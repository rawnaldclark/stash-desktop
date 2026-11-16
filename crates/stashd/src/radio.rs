//! Radio, and autoplay that knows what is like what. Last.fm says which artists and songs are like the one a station
//! starts from (a song, an artist, or the song playing; for autoplay, the last song of the queue); the songs come from
//! the listener's own library first, then from their add-ons, one search an artist. A station grows as it plays, and
//! ends when another queue starts, when it is stopped, or when it has nothing more like it.
//!
//! Last.fm is asked through Stash's own proxy, which holds the key: a build carries only the proxy's address (see
//! `stash_core::similar`), and a build without one has no radio (autoplay then picks from the library, as it always
//! did). Each question is asked at most once in two weeks (the answers are kept in the library), at most twice in a
//! station's life, never offline, and never more than [`DAILY_CAP`] times a day. One thread asks, so no question is
//! asked twice at once, and the library is never held while the network is asked.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::addons::Track;
use stash_core::lists::Room;
use stash_core::matching::{names_other_version, same_title};
use stash_core::player::{Player, QueueItem};
use stash_core::shuffle::artist_key;
use stash_core::similar::{self, Answer, Ask, Proxy, Trouble};
use stash_core::station::{self, Neighbor, Seed, Station};
use tiny_http::{Method, Request};

use crate::serve::{
    Reply, State, addon_item, body_json, can_stream, found_track, now_ms, offline, refuse,
    reply_json,
};

/// The setting that switches similar music from Last.fm off ("0").
pub(crate) const SETTING: &str = "similar.lastfm";
/// How many questions were sent today: "day:count", the day counted in days since 1970.
const DAY_SETTING: &str = "similar.day";
/// The most questions sent to Last.fm in a day: past it, stations pick from the library until the next day.
pub(crate) const DAILY_CAP: u32 = 100;
/// Songs heard this recently are not picked again.
const RESTED_MS: i64 = 3 * 3600 * 1000;
/// Answers kept longer than this are let go when the core starts, and no more than this many are kept.
const KEPT_FOR_S: i64 = 60 * 86_400;
const KEPT_ROWS: usize = 5_000;
/// How many songs the stations of this run added are remembered, so a later station does not bring one back (an
/// add-on's song leaves no play in the library).
const RING: usize = 1_000;
/// How far back in the queue songs already there are looked at.
const LOOKED_BACK: usize = 500;
/// How often answers kept too long are let go of while Stash runs (and when the core starts).
const PRUNE_EVERY: Duration = Duration::from_secs(24 * 3600);
/// How many times one fill opens more artists when a batch comes back empty, before the station ends.
const ATTEMPTS: usize = 3;
/// How many tracks one add-on search asks for.
const SEARCHED: u32 = 20;
/// Searches made at once.
const AT_ONCE: usize = 4;
/// How long a trouble an add-on had (after the last song it gave) keeps a station from searching it.
const TROUBLE_FOR_MS: i64 = 10 * 60 * 1000;

const NOT_IN_BUILD: &str = "Radio is not in this build of Stash";
const SWITCHED_OFF: &str = "Similar music from Last.fm is switched off (Settings › Sound)";
const NOTHING_PLAYS: &str = "Nothing is playing";

// Why a station picks from the library instead.
const NOTHING_LIKE: &str = "Last.fm found nothing like it";
const OFFLINE: &str = "Offline";
const UNREACHED: &str = "Last.fm could not be reached";
const ENOUGH: &str = "Last.fm was asked enough for today";
const SAYS_TOO_LITTLE: &str = "Its title and artist say too little to ask about";
const NOTHING_HERE: &str = "Nothing by the artists Last.fm named can play here";
/// The station was stopped, replaced or switched off while its songs were found (never shown: its songs are dropped).
const STOPPED: &str = "Stopped";

/// Whether a station was started by the listener (a radio) or by the queue's end (autoplay).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Radio,
    Autoplay,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Radio => "radio",
            Kind::Autoplay => "autoplay",
        }
    }
}

/// The station of the queue playing.
struct Live {
    /// Which station this is: songs found for another one (stopped, replaced, switched off) are dropped.
    token: u64,
    kind: Kind,
    /// Whether Last.fm chose its songs (false: the library did, by the artists heard, likes and plays).
    lastfm: bool,
    /// Why the library chose them, when it did.
    why: Option<&'static str>,
    seed: Option<Seed>,
    label: String,
    /// The seed's page on Last.fm.
    link: Option<String>,
    /// The queue it adds to (see `PlayerState::queue_id`), and for autoplay, the song it adds after while that is
    /// still the last.
    queue_id: u64,
    after: Option<u64>,
    /// What the library picks from when Last.fm cannot help: these songs' and artists' (the seed's, or for autoplay
    /// the last songs heard).
    fallback: (Vec<i64>, Vec<String>),
    /// The station; taken out while the radio thread finds its next songs, and none for a fill from the library.
    station: Option<Station>,
    /// The queue numbers of the songs it added.
    added: HashSet<u64>,
    /// Songs are wanted: the radio thread finds them.
    wanted: bool,
    /// No song was added yet.
    finding: bool,
    /// It has nothing more to add.
    spent: bool,
}

/// Last.fm left alone a while after trouble: until when, how many troubles in a row, and the questions that failed.
#[derive(Default)]
struct Gate {
    closed_until: Option<Instant>,
    trips: u32,
    failed: HashMap<String, u32>,
}

pub(crate) struct Radio {
    proxy: Option<Proxy>,
    live: Mutex<Option<Live>>,
    bell: Mutex<bool>,
    rung: Condvar,
    gate: Mutex<Gate>,
    /// What the stations of this run added (one identity a song, `station::song_identity`), newest last.
    ring: Mutex<VecDeque<String>>,
    /// The last station's token.
    tokens: AtomicU64,
    /// The song autoplay last acted on, by its queue number (0: none): it acts once a song, and not at all after a
    /// station was stopped on it.
    pub(crate) autoplay_after: AtomicU64,
}

impl Radio {
    /// Radio through the proxy at `proxy`, or none (`None`, or an address that is not usable).
    pub(crate) fn new(proxy: Option<&str>) -> Self {
        let proxy = proxy
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .and_then(|p| match Proxy::new(p) {
                Ok(proxy) => Some(proxy),
                Err(_) => {
                    // Never the address itself: it is not to be shown anywhere.
                    eprintln!("the Last.fm proxy address was not usable: radio is left out");
                    None
                }
            });
        Self {
            proxy,
            live: Mutex::new(None),
            bell: Mutex::new(false),
            rung: Condvar::new(),
            gate: Mutex::new(Gate::default()),
            ring: Mutex::new(VecDeque::new()),
            tokens: AtomicU64::new(0),
            autoplay_after: AtomicU64::new(0),
        }
    }

    /// Whether this build has radio (it knows the proxy).
    pub(crate) fn available(&self) -> bool {
        self.proxy.is_some()
    }

    /// Whether similar music from Last.fm is on: in this build, and not switched off.
    pub(crate) fn on(&self, library: &Library) -> bool {
        self.available() && library.setting(SETTING).ok().flatten().as_deref() != Some("0")
    }

    fn ring_bell(&self) {
        *self.bell.lock().unwrap() = true;
        self.rung.notify_all();
    }

    /// Starts a station from `seed` for the queue `queue_id`: the radio thread finds its first songs. Nothing starts
    /// when that queue was replaced meanwhile, nor autoplay while a station still grows it.
    pub(crate) fn begin(
        &self,
        player: &Player,
        kind: Kind,
        seed: Seed,
        fallback: (Vec<i64>, Vec<String>),
        queue_id: u64,
        after: Option<u64>,
    ) {
        let station = Station::new(seed.clone(), if kind == Kind::Radio { 3 } else { 2 });
        {
            let mut live = self.live.lock().unwrap();
            if !self.may_begin(&live, player, kind, queue_id) {
                return;
            }
            *live = Some(Live {
                token: self.tokens.fetch_add(1, Ordering::Relaxed) + 1,
                kind,
                lastfm: true,
                why: None,
                label: station.label(),
                link: Some(station.link()),
                seed: Some(seed),
                queue_id,
                after,
                fallback,
                station: Some(station),
                added: HashSet::new(),
                wanted: true,
                finding: true,
                spent: false,
            });
        }
        self.ring_bell();
    }

    /// Autoplay added `qids` from the library to the queue `queue_id`: the window shows where they begin, and Stop takes
    /// them out.
    pub(crate) fn note_fill(&self, player: &Player, qids: Vec<u64>, queue_id: u64) {
        if qids.is_empty() {
            return;
        }
        let mut live = self.live.lock().unwrap();
        if !self.may_begin(&live, player, Kind::Autoplay, queue_id) {
            return;
        }
        *live = Some(Live {
            token: self.tokens.fetch_add(1, Ordering::Relaxed) + 1,
            kind: Kind::Autoplay,
            lastfm: false,
            why: None,
            seed: None,
            label: String::new(),
            link: None,
            queue_id,
            after: None,
            fallback: (Vec::new(), Vec::new()),
            station: None,
            added: qids.into_iter().collect(),
            wanted: false,
            finding: false,
            spent: true,
        });
    }

    /// Whether a station of `kind` may begin on the queue `queue_id`: it is still the player's (a radio the listener
    /// just started replaced it, else), and for autoplay, no station still grows it. Asked with the station's lock
    /// held, so two cannot both begin.
    fn may_begin(&self, live: &Option<Live>, player: &Player, kind: Kind, queue_id: u64) -> bool {
        player.state().queue_id == queue_id
            && !(kind == Kind::Autoplay
                && live
                    .as_ref()
                    .is_some_and(|l| l.queue_id == queue_id && !l.spent))
    }

    /// Whether the station `token` is still the one playing (not stopped, replaced or switched off).
    fn current(&self, token: u64) -> bool {
        self.live
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|l| l.token == token)
    }

    /// Whether a station still grows the queue `queue_id`: autoplay leaves it to the station then.
    pub(crate) fn carries_on(&self, queue_id: u64) -> bool {
        self.live
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|l| l.queue_id == queue_id && !l.spent)
    }

    /// Looks at the player (about once a second): a station whose queue was replaced ends; one with few songs left
    /// after the one playing grows.
    pub(crate) fn look(&self, player: &Player) {
        let state = player.state();
        let mut live = self.live.lock().unwrap();
        let Some(l) = live.as_mut() else { return };
        if state.queue_id != l.queue_id {
            *live = None;
            return;
        }
        if l.spent || l.wanted || l.station.is_none() || state.repeat != "off" {
            return;
        }
        let Some(index) = state.index else { return };
        if state.queue_len.saturating_sub(index + 1) <= station::GROW_WHEN_LEFT {
            l.wanted = true;
            drop(live);
            self.ring_bell();
        }
    }

    /// Stops the station: its songs after the one playing are taken out (songs the listener added stay), and nothing
    /// follows the song playing. Answers how many songs went.
    pub(crate) fn stop(&self, player: &Player) -> usize {
        let Some(live) = self.live.lock().unwrap().take() else {
            return 0;
        };
        let state = player.state();
        if let Some(track) = &state.track {
            self.autoplay_after.store(track.qid, Ordering::Relaxed);
        }
        if state.queue_id != live.queue_id {
            return 0;
        }
        let from = state.index.map_or(0, |i| i + 1);
        let ahead: HashSet<u64> = player
            .queue_at(from, state.queue_len.saturating_sub(from))
            .iter()
            .map(|t| t.qid)
            .filter(|qid| live.added.contains(qid))
            .collect();
        if ahead.is_empty() {
            return 0;
        }
        player.retain(|t| !ahead.contains(&t.qid))
    }

    /// Ends a station Last.fm chose (similar music was switched off). Its songs stay in the queue.
    fn end_lastfm(&self) {
        let mut live = self.live.lock().unwrap();
        if live.as_ref().is_some_and(|l| l.lastfm) {
            *live = None;
        }
    }

    /// The station of the queue playing, for the window (`station` in `/api/player`), or null.
    pub(crate) fn summary(&self, player: &Player) -> Value {
        let state = player.state();
        let (mut said, added) = {
            let live = self.live.lock().unwrap();
            let Some(l) = live.as_ref().filter(|l| l.queue_id == state.queue_id) else {
                return Value::Null;
            };
            let seed = match &l.seed {
                Some(Seed::Song { title, artist }) => json!({ "title": title, "artist": artist }),
                Some(Seed::Artist { name }) => json!({ "artist": name }),
                None => Value::Null,
            };
            (
                json!({
                    "kind": l.kind.word(),
                    "label": l.label,
                    "seed": seed,
                    "via": if l.lastfm { "lastfm" } else { "library" },
                    "why": l.why,
                    "finding": l.finding && !l.spent,
                    // How many songs it added: none, and nothing follows, means it found none.
                    "added": l.added.len(),
                }),
                l.added.clone(),
            )
        };
        // Where its songs still to come begin: the window draws its line there.
        let from = state.index.map_or(0, |i| i + 1);
        let from_qid = player
            .queue_at(from, 2_000)
            .iter()
            .map(|t| t.qid)
            .find(|qid| added.contains(qid));
        said["from_qid"] = json!(from_qid);
        said
    }

    /// What Last.fm says to `ask`: kept from before when it is fresh (any age offline, or while Last.fm is left
    /// alone), else asked. Why not, when there is no answer.
    fn answer(
        &self,
        state: &State,
        ask: &Ask,
        off: bool,
        token: u64,
    ) -> std::result::Result<Answer, &'static str> {
        if !self.current(token) {
            return Err(STOPPED);
        }
        if !askable(ask) {
            return Err(SAYS_TOO_LITTLE);
        }
        let key = ask.key();
        let now_s = now_ms() / 1000;
        let kept = state
            .library
            .lock()
            .unwrap()
            .kept_similar(&key)
            .ok()
            .flatten();
        let closed = self
            .gate
            .lock()
            .unwrap()
            .closed_until
            .is_some_and(|until| Instant::now() < until);
        if let Some((answer, at)) = &kept
            && (off || closed || now_s - at < similar::FRESH_S)
        {
            return Ok(answer.clone());
        }
        let stale = || kept.as_ref().map(|(answer, _)| answer.clone());
        if off {
            return Err(OFFLINE);
        }
        if closed
            || self
                .gate
                .lock()
                .unwrap()
                .failed
                .get(&key)
                .is_some_and(|n| *n >= 2)
        {
            return stale().ok_or(UNREACHED);
        }
        match self.may_ask(state) {
            Ok(()) => {}
            Err(STOPPED) => return Err(STOPPED),
            Err(words) => return stale().ok_or(words),
        }
        let Some(proxy) = &self.proxy else {
            return Err(UNREACHED);
        };
        match proxy.ask(ask) {
            Ok(answer) => {
                {
                    let mut gate = self.gate.lock().unwrap();
                    gate.trips = 0;
                    gate.closed_until = None;
                }
                // Kept only while similar music is still on: switched off meanwhile, what was kept was deleted,
                // and this is not kept either.
                let mut library = state.library.lock().unwrap();
                if !self.on(&library) {
                    return Err(STOPPED);
                }
                let _ = library.keep_similar(&key, &answer, now_s);
                Ok(answer)
            }
            Err(trouble) => {
                let mut gate = self.gate.lock().unwrap();
                *gate.failed.entry(key).or_default() += 1;
                let wait = match trouble {
                    Trouble::Busy { retry_after_s } => {
                        Duration::from_secs(retry_after_s.unwrap_or(60).clamp(1, 3600))
                    }
                    Trouble::Away(words) => {
                        gate.trips += 1;
                        eprintln!("radio: {words}");
                        Duration::from_secs((60u64 << (gate.trips - 1).min(6)).min(3600))
                    }
                };
                gate.closed_until = Some(Instant::now() + wait);
                stale().ok_or(UNREACHED)
            }
        }
    }

    /// Whether a question may go to Last.fm now, and counts it: not when similar music was switched off (asked under
    /// the same hold of the library that switches it off), nor past today's questions.
    fn may_ask(&self, state: &State) -> std::result::Result<(), &'static str> {
        let day = now_ms() / 86_400_000;
        let mut library = state.library.lock().unwrap();
        if !self.on(&library) {
            return Err(STOPPED);
        }
        let sent = library
            .setting(DAY_SETTING)
            .ok()
            .flatten()
            .and_then(|kept| {
                let (on, n) = kept.split_once(':')?;
                Some((on.parse::<i64>().ok()?, n.parse::<u32>().ok()?))
            })
            .filter(|(on, _)| *on == day)
            .map_or(0, |(_, n)| n);
        if sent >= DAILY_CAP {
            return Err(ENOUGH);
        }
        let _ = library.set_setting(DAY_SETTING, &format!("{day}:{}", sent + 1));
        Ok(())
    }

    /// The station's next songs: Last.fm's answers (asked when the station wants them, between rounds of finding
    /// songs), then each artist's songs from the library, then from an add-on. Why the library must pick instead, when
    /// Last.fm could not help. Nothing when the station `token` stops being the one playing meanwhile.
    fn fill(
        &self,
        state: &State,
        player: &Player,
        station: &mut Station,
        token: u64,
    ) -> (Vec<QueueItem>, Option<&'static str>) {
        let off = offline(state);
        // Offline, or with no add-on to stream from, only songs on this computer can play.
        let here_only = !can_stream(state);
        let mut asked_why = None;

        // What the queue holds already, and what the stations of this run added, is not added again: such songs
        // take no room in the station's hands.
        let now = player.state();
        let around = player.queue_at(now.queue_len.saturating_sub(LOOKED_BACK), LOOKED_BACK);
        let mut queued: HashSet<String> = around.iter().flat_map(station::identities).collect();
        queued.extend(self.ring.lock().unwrap().iter().cloned());
        let last_artist = around.last().map(|t| t.artist.clone());
        let room = Room {
            not: around.iter().filter_map(|t| t.id).collect(),
            rested_since_ms: now_ms() - RESTED_MS,
            here_only,
        };
        let fresh = |item: &QueueItem| {
            station::identities(item)
                .iter()
                .all(|id| !queued.contains(id))
        };

        let mut attempts = 0;
        let batch = loop {
            loop {
                while let Some(ask) = station.wants() {
                    let answer = match self.answer(state, &ask, off, token) {
                        Ok(answer) => Some(answer),
                        Err(STOPPED) => return (Vec::new(), None),
                        Err(words) => {
                            asked_why = Some(words);
                            None
                        }
                    };
                    if matches!(answer, Some(Answer::NotFound)) {
                        asked_why = Some(NOTHING_LIKE);
                    }
                    let library = state.library.lock().unwrap();
                    station.take(&ask, answer.as_ref(), &|name| {
                        library.artist_blocked(name).unwrap_or(false)
                    });
                }
                let need = station.to_fill();
                if need.is_empty() {
                    break;
                }
                if !self.current(token) {
                    return (Vec::new(), None);
                }
                // The library's own songs first: they cost nothing.
                let mut short = Vec::new();
                for neighbor in need {
                    // An add-on is searched when the library has fewer than half the artist's pool.
                    let pool = station.pool_size(&neighbor.key);
                    let few = pool / 2;
                    let items: Vec<QueueItem> = {
                        let library = state.library.lock().unwrap();
                        library
                            .songs_by_artist(
                                &neighbor.name,
                                &neighbor.key,
                                &neighbor.titles,
                                &room,
                                pool * 2,
                            )
                            .and_then(|ids| library.queue_items(&ids))
                            .unwrap_or_default()
                    }
                    .into_iter()
                    .filter(|item| fresh(item))
                    .take(pool)
                    .collect();
                    if items.len() < few && !here_only {
                        short.push((neighbor, items, pool));
                    } else {
                        station.fill(&neighbor.key, items);
                    }
                }
                // Then an add-on's, one search an artist.
                for (neighbor, items) in self.search(state, short, &queued) {
                    station.fill(&neighbor.key, items);
                }
            }
            let batch = station.batch(&queued, last_artist.as_deref());
            // Gone offline (or the add-ons gone) since its songs were found: only those here.
            let batch = if here_only {
                only_here(state, batch)
            } else {
                batch
            };
            attempts += 1;
            if !batch.is_empty() || station.spent() || attempts >= ATTEMPTS {
                break batch;
            }
        };
        let why = if !batch.is_empty() {
            None
        } else if station.found() {
            Some(NOTHING_HERE)
        } else {
            Some(asked_why.unwrap_or(NOTHING_LIKE))
        };
        (batch, why.or(asked_why))
    }

    /// The artists' songs, with what one add-on (the first that searches and plays, and is not in trouble) finds of
    /// each, up to its pool; a few searches at a time.
    fn search(
        &self,
        state: &State,
        short: Vec<(Neighbor, Vec<QueueItem>, usize)>,
        queued: &HashSet<String>,
    ) -> Vec<(Neighbor, Vec<QueueItem>)> {
        if short.is_empty() {
            return Vec::new();
        }
        let searcher = state
            .addons
            .read()
            .unwrap()
            .iter()
            .find(|loaded| {
                loaded
                    .usable()
                    .is_some_and(|a| a.offers_resource("search") && a.offers_resource("stream"))
                    && !state.lacks.in_trouble(&loaded.stored.id, TROUBLE_FOR_MS)
            })
            .cloned();
        let Some(loaded) = searcher else {
            return short.into_iter().map(|(n, items, _)| (n, items)).collect();
        };
        let addon_id = loaded.stored.id.clone();
        let mut out = Vec::new();
        let mut rest = short.into_iter().peekable();
        while rest.peek().is_some() {
            let group: Vec<_> = rest.by_ref().take(AT_ONCE).collect();
            let found: Vec<_> = std::thread::scope(|scope| {
                let asks: Vec<_> = group
                    .into_iter()
                    .map(|(neighbor, items, pool)| {
                        let loaded = &loaded;
                        scope.spawn(move || {
                            let tracks =
                                loaded.usable().map(|a| a.search(&neighbor.name, SEARCHED));
                            (neighbor, items, pool, tracks)
                        })
                    })
                    .collect();
                asks.into_iter().filter_map(|ask| ask.join().ok()).collect()
            });
            for (neighbor, mut items, pool, tracks) in found {
                match tracks {
                    Some(Ok(tracks)) => {
                        state.lacks.heard(&addon_id);
                        let library = state.library.lock().unwrap();
                        add_found(
                            &library, &addon_id, &neighbor, &tracks, queued, &mut items, pool,
                        );
                    }
                    Some(Err(e)) => state.lacks.troubled(&addon_id, &e),
                    None => {}
                }
                out.push((neighbor, items));
            }
        }
        out
    }

    /// Adds what the radio thread found to the queue it was found for, and keeps the station for its next songs.
    /// When Last.fm could not help with a station's first songs, the library picks them.
    fn settle(
        &self,
        state: &State,
        player: &Player,
        job: Job,
        station: Station,
        found: Vec<QueueItem>,
        why: Option<&'static str>,
    ) {
        // Its first songs, when Last.fm named nothing it could use (it was not asked, or knew nothing like it): the
        // library's picks after the seed, not the seed's artist alone.
        let (items, lastfm) = if job.first && (found.is_empty() || !station.found()) {
            (
                library_picks(state, player, &job.fallback.0, &job.fallback.1),
                false,
            )
        } else {
            (found, true)
        };
        let spent = !lastfm || items.is_empty() || station.spent();
        let words: Vec<String> = items
            .iter()
            .map(|item| station::song_identity(&item.title, &item.artist))
            .collect();
        let qids = {
            // Stopped, switched off or replaced while they were found: they are not added. They are added with the
            // station's lock held, so a Stop comes before (and they are not added) or after (and takes them out).
            let mut live = self.live.lock().unwrap();
            let Some(l) = live.as_mut().filter(|l| l.token == job.token) else {
                return;
            };
            // The queue changed while they were found (another queue, or songs added after the last one).
            let Some(qids) = player.enqueue_onto(items, job.queue_id, job.after) else {
                *live = None;
                return;
            };
            l.added.extend(qids.iter().copied());
            if !qids.is_empty() {
                l.after = None;
            }
            l.station = Some(station);
            l.wanted = false;
            l.spent = spent;
            if !qids.is_empty() || spent {
                l.finding = false;
            }
            if !lastfm {
                l.lastfm = false;
                l.why = why.or(Some(NOTHING_LIKE));
            }
            qids
        };
        {
            let mut ring = self.ring.lock().unwrap();
            ring.extend(words);
            while ring.len() > RING {
                ring.pop_front();
            }
        }
        // The queue ran out while they were found: play goes on with the first of them.
        let now = player.state();
        if let (Some(&first), true, Some(index)) = (qids.first(), now.resting, now.index)
            && player
                .queue_at(index + 1, 1)
                .first()
                .is_some_and(|t| t.qid == first)
        {
            player.jump(index + 1);
        }
    }
}

/// A station's songs being found, and the queue they are for.
struct Job {
    token: u64,
    queue_id: u64,
    after: Option<u64>,
    first: bool,
    fallback: (Vec<i64>, Vec<String>),
}

/// Whether Last.fm can be asked about `ask`: not a song whose title or artist says too little ("Unknown Artist", a
/// phone's recording, a file name).
fn askable(ask: &Ask) -> bool {
    ask.askable()
}

/// What a station from `item` asks Last.fm about: the song as it is asked for (a row from YouTube is the song its
/// title names), without the words video sites add, by the first artist its credit names.
pub(crate) fn song_seed(library: &Library, item: &QueueItem) -> Seed {
    let (title, artist) = match item.id.and_then(|id| library.describe(id).ok().flatten()) {
        Some(described) => (described.wanted.title, described.wanted.artist),
        None => (item.title.clone(), item.artist.clone()),
    };
    let artist = similar::clean_artist(&artist);
    let first = library
        .credit_parts(&artist)
        .ok()
        .and_then(|parts| parts.into_iter().next())
        .map_or(artist, |part| part.name);
    Seed::Song {
        title: similar::clean_title(&title),
        artist: first,
    }
}

/// Whether Last.fm can be asked about a song seed.
pub(crate) fn seed_askable(seed: &Seed) -> bool {
    match seed {
        Seed::Song { title, artist } => similar::lookable(title, artist),
        Seed::Artist { name } => Ask::Artist {
            artist: name.clone(),
        }
        .askable(),
    }
}

/// An add-on's tracks by the artist `neighbor` names, as queue items, added to `items` up to `pool`: the titles
/// Last.fm named first, then in the add-on's order. Not another version of a song (live, a remix, a cover…) unless
/// Last.fm named that very title, none chosen not to be played, and one copy of each song.
fn add_found(
    library: &Library,
    addon_id: &str,
    neighbor: &Neighbor,
    tracks: &[Track],
    queued: &HashSet<String>,
    items: &mut Vec<QueueItem>,
    pool: usize,
) {
    let wanted = |title: &str| neighbor.titles.iter().position(|t| same_title(t, title));
    let mut found: Vec<(usize, usize, QueueItem)> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| artist_key(&t.artist) == neighbor.key)
        .filter(|(_, t)| wanted(&t.title).is_some() || !names_other_version(&t.title))
        .filter(|(_, t)| !library.words_blocked(&t.title, &t.artist).unwrap_or(true))
        .filter_map(|(place, t)| {
            let item = addon_item(addon_id, &found_track(t)?)?;
            Some((wanted(&t.title).unwrap_or(usize::MAX), place, item))
        })
        .collect();
    found.sort_by_key(|(wanted, place, _)| (*wanted, *place));
    // One copy of a song ("Angel" and "Angel - 2006 Remaster" are one), and none the queue holds already.
    let mut seen: HashSet<String> = items
        .iter()
        .map(|i| station::song_identity(&i.title, &i.artist))
        .collect();
    for (_, _, item) in found {
        if items.len() >= pool {
            break;
        }
        if station::identities(&item)
            .iter()
            .any(|id| queued.contains(id))
        {
            continue;
        }
        if seen.insert(station::song_identity(&item.title, &item.artist)) {
            items.push(item);
        }
    }
}

/// The library's own picks after `seeds` (and their artists): today's autoplay (`Library::autoplay_picks`).
fn library_picks(
    state: &State,
    player: &Player,
    seeds: &[i64],
    seed_artists: &[String],
) -> Vec<QueueItem> {
    let now = player.state();
    let around = player.queue_at(now.queue_len.saturating_sub(LOOKED_BACK), LOOKED_BACK);
    let queued: HashSet<i64> = around.iter().filter_map(|t| t.id).collect();
    let files_only = !crate::serve::can_stream(state);
    let library = state.library.lock().unwrap();
    library
        .autoplay_picks(
            seeds,
            seed_artists,
            &queued,
            now_ms() - RESTED_MS,
            files_only,
            station::BATCH,
        )
        .and_then(|ids| library.queue_items(&ids))
        .unwrap_or_default()
}

/// Lets go of answers kept too long (when the core starts).
pub(crate) fn prune(library: &mut Library) {
    let _ = library.prune_similar(now_ms() / 1000 - KEPT_FOR_S, KEPT_ROWS);
}

/// The one thread that asks Last.fm and the add-ons for the stations' songs.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let Ok(player) = &state.player else { return };
    let radio = &state.radio;
    if !radio.available() {
        return;
    }
    let mut pruned = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if pruned.elapsed() >= PRUNE_EVERY {
            pruned = Instant::now();
            prune(&mut state.library.lock().unwrap());
        }
        {
            let bell = radio.bell.lock().unwrap();
            let (mut rung, _) = radio
                .rung
                .wait_timeout_while(bell, Duration::from_secs(1), |rung| !*rung)
                .unwrap();
            *rung = false;
        }
        // The station that wants songs, taken out while they are found.
        let taken = {
            let mut live = radio.live.lock().unwrap();
            match live.as_mut() {
                Some(l) if l.wanted && l.station.is_some() => l.station.take().map(|station| {
                    let job = Job {
                        token: l.token,
                        queue_id: l.queue_id,
                        after: l.after,
                        first: l.finding,
                        fallback: l.fallback.clone(),
                    };
                    (job, station)
                }),
                _ => None,
            }
        };
        let Some((job, mut station)) = taken else {
            continue;
        };
        let (found, why) = radio.fill(state, player, &mut station, job.token);
        radio.settle(state, player, job, station, found, why);
    }
}

/// `/api/settings/similar` and `/api/radio/*`.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    let radio = &state.radio;
    let library = || state.library.lock().unwrap();
    Ok(Some(match (request.method().clone(), path) {
        (Method::Get, "/api/settings/similar") => {
            let on = radio.on(&library());
            reply_json(200, json!({ "available": radio.available(), "on": on }))
        }
        (Method::Post, "/api/settings/similar") => {
            let on = body_json(request)?
                .get("on")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if !radio.available() {
                return Ok(Some(refuse(404, NOT_IN_BUILD)));
            }
            let forgot = {
                let mut library = library();
                library.set_setting(SETTING, if on { "1" } else { "0" })?;
                if on { 0 } else { library.forget_similar()? }
            };
            if !on {
                radio.end_lastfm();
            }
            reply_json(
                200,
                json!({ "available": true, "on": on, "forgot": forgot }),
            )
        }
        (Method::Post, "/api/radio/start") => start(state, request)?,
        (Method::Post, "/api/radio/stop") => match &state.player {
            Ok(player) => reply_json(200, json!({ "removed": radio.stop(player) })),
            Err(why) => refuse(409, why),
        },
        (Method::Post, "/api/radio/lastfm") => {
            let link = radio
                .live
                .lock()
                .unwrap()
                .as_ref()
                .filter(|l| l.lastfm)
                .and_then(|l| l.link.clone());
            match link {
                None => refuse(409, "No radio is playing"),
                // In the window, the shell opens it in the browser; in a page served to a browser, the page does.
                Some(link) => match &state.opener {
                    Some(open) if open(&link) => reply_json(200, json!({ "opened": true })),
                    _ => reply_json(200, json!({ "url": link })),
                },
            }
        }
        _ => return Ok(None),
    }))
}

/// `POST /api/radio/start`: `{song: {id} | {addon, track}}` plays that song now, `{playing: true}` keeps the song
/// playing and replaces what follows it, `{artist}` plays the artist's song the listener likes or plays most (else one
/// an add-on has). A station then grows the queue.
fn start(state: &State, request: &mut Request) -> Result<Reply> {
    let radio = &state.radio;
    if !radio.available() {
        return Ok(refuse(404, NOT_IN_BUILD));
    }
    if !radio.on(&state.library.lock().unwrap()) {
        return Ok(refuse(409, SWITCHED_OFF));
    }
    let player = match &state.player {
        Ok(player) => player,
        Err(why) => return Ok(refuse(409, why)),
    };
    let body = body_json(request)?;
    let off = offline(state);

    let playing = body.get("playing").and_then(Value::as_bool) == Some(true);
    let now = player.state();
    if playing && !now.resting {
        let Some(track) = now.track else {
            return Ok(refuse(409, NOTHING_PLAYS));
        };
        let seed = song_seed(&state.library.lock().unwrap(), &track);
        let label = Station::new(seed.clone(), 3).label();
        if !player.play_after_current(Vec::new(), Some(label)) {
            return Ok(refuse(409, NOTHING_PLAYS));
        }
        let fallback = (track.id.into_iter().collect(), vec![track.artist.clone()]);
        radio.begin(
            player,
            Kind::Radio,
            seed,
            fallback,
            player.state().queue_id,
            None,
        );
        return Ok(reply_json(200, json!({ "station": radio.summary(player) })));
    }

    let (item, seed) = if playing {
        // The player rests on it (after a restart, or a queue that ran out): it plays again from its start.
        let Some(item) = now.track else {
            return Ok(refuse(409, NOTHING_PLAYS));
        };
        if off && !here(state, &item) {
            return Ok(refuse(
                409,
                "Stash is offline, and this song is not on this computer",
            ));
        }
        let seed = song_seed(&state.library.lock().unwrap(), &item);
        (item, seed)
    } else if let Some(song) = body.get("song") {
        let item = match song.get("id").and_then(Value::as_i64) {
            Some(id) => {
                let library = state.library.lock().unwrap();
                let Some(item) = library.queue_items(&[id])?.pop() else {
                    return Ok(refuse(404, "That song is not in your library"));
                };
                if off && !library.here_of(&[id])?.contains(&id) {
                    return Ok(refuse(
                        409,
                        "Stash is offline, and this song is not on this computer",
                    ));
                }
                item
            }
            None => {
                let Some(item) = song
                    .get("addon")
                    .and_then(Value::as_str)
                    .zip(song.get("track"))
                    .and_then(|(addon, track)| addon_item(addon, track))
                else {
                    return Ok(refuse(400, "song is {id} or {addon, track}"));
                };
                if off {
                    return Ok(refuse(
                        409,
                        "Stash is offline, and this song is not on this computer",
                    ));
                }
                item
            }
        };
        let seed = song_seed(&state.library.lock().unwrap(), &item);
        (item, seed)
    } else if let Some(name) = body
        .get("artist")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        if state.library.lock().unwrap().artist_blocked(name)? {
            return Ok(refuse(409, &format!("You chose not to play {name}")));
        }
        let Some(item) = artist_song(state, name, off) else {
            return Ok(refuse(409, &format!("Nothing by {name} can play here")));
        };
        (
            item,
            Seed::Artist {
                name: name.to_string(),
            },
        )
    } else {
        return Ok(refuse(400, "say song, artist or playing"));
    };

    let label = Station::new(seed.clone(), 3).label();
    let fallback = (item.id.into_iter().collect(), vec![item.artist.clone()]);
    player.play_from(vec![item], 0, Some(label))?;
    radio.begin(
        player,
        Kind::Radio,
        seed,
        fallback,
        player.state().queue_id,
        None,
    );
    Ok(reply_json(200, json!({ "station": radio.summary(player) })))
}

/// Of `items`, those on this computer (a file of the library, or a song kept as it was played).
fn only_here(state: &State, items: Vec<QueueItem>) -> Vec<QueueItem> {
    let ids: Vec<i64> = items.iter().filter_map(|item| item.id).collect();
    let here = state
        .library
        .lock()
        .unwrap()
        .here_of(&ids)
        .unwrap_or_default();
    items
        .into_iter()
        .filter(|item| item.id.is_some_and(|id| here.contains(&id)))
        .collect()
}

/// Whether `item` is on this computer: a file of the library, or a song kept as it was played.
fn here(state: &State, item: &QueueItem) -> bool {
    item.id.is_some_and(|id| {
        state
            .library
            .lock()
            .unwrap()
            .here_of(&[id])
            .is_ok_and(|here| here.contains(&id))
    })
}

/// The song an artist's radio begins with: the library's the listener likes or plays most, else (online) the first an
/// add-on finds by them.
fn artist_song(state: &State, name: &str, off: bool) -> Option<QueueItem> {
    let key = artist_key(name);
    let room = Room {
        not: HashSet::new(),
        // Any song, however recently heard.
        rested_since_ms: i64::MAX,
        here_only: !can_stream(state),
    };
    {
        let library = state.library.lock().unwrap();
        if let Some(item) = library
            .songs_by_artist(name, &key, &[], &room, 1)
            .and_then(|ids| library.queue_items(&ids))
            .ok()
            .and_then(|mut items| items.pop())
        {
            return Some(item);
        }
    }
    if off {
        return None;
    }
    let neighbor = Neighbor {
        name: name.to_string(),
        key,
        titles: Vec::new(),
    };
    state
        .radio
        .search(state, vec![(neighbor, Vec::new(), 1)], &HashSet::new())
        .into_iter()
        .next()
        .and_then(|(_, mut items)| items.pop())
}
