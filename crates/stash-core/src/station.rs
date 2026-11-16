//! A station: songs like one song or one artist, added a batch at a time. Last.fm says which artists are like the
//! seed (and, for a song, which of their songs), the caller finds those artists' songs (the library first, then an
//! add-on), and the station picks from them: a share for the seed's own artist, at most two by any other artist in
//! ten, never the same artist twice in a row when that can be helped, and never one song twice.
//!
//! The station only decides. It asks no one and holds no lock: the caller asks Last.fm what [`Station::wants`]
//! names and hands the answer to [`Station::take`], finds the songs of the artists [`Station::to_fill`] names and
//! hands them to [`Station::fill`], then takes a [`Station::batch`]. Before each batch:
//!
//! ```text
//! loop {
//!     while let Some(ask) = station.wants() { station.take(&ask, answer_to(&ask), &blocked) }
//!     let artists = station.to_fill();
//!     if artists.is_empty() { break }
//!     for artist in artists { station.fill(&artist.key, songs_of(&artist)) }
//! }
//! let items = station.batch(&queued, last_artist);
//! ```

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::identity::{fold, words_key};
use crate::matching::{same_title, title_key};
use crate::player::QueueItem;
use crate::shuffle::{apart, artist_key};
use crate::similar::{Answer, Ask};

/// The most songs in a batch.
pub const BATCH: usize = 10;
/// A station grows when this many songs or fewer are left after the one playing.
pub const GROW_WHEN_LEFT: usize = 3;
/// The most new artists opened at once; a batch opens at most two rounds of them.
pub const NEW_ARTISTS: usize = 4;
/// The most songs in a batch by one artist other than the seed's.
pub const PER_BATCH: usize = 2;
/// The most songs kept in hand for one artist.
pub const POOL: usize = 6;
/// The most songs kept in hand for the seed's own artist.
pub const SEED_POOL: usize = 12;
/// A song's similar tracks that name fewer artists than this (the seed's own not counted) are not enough: the
/// artists like the seed's are asked for too.
pub const FEW: usize = 4;
/// Rounds of opening new artists before one batch.
const ROUNDS: usize = 2;
/// The most Last.fm questions in a station's life.
const QUESTIONS: usize = 2;

/// What a station starts from, already as Last.fm is asked: the first artist the credit names, and a title without
/// "(Official Audio)" and the like. Shown to the window as `{"title", "artist"}` or `{"artist"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Seed {
    /// Songs like this song.
    Song { title: String, artist: String },
    /// Songs by artists like this one.
    Artist {
        #[serde(rename = "artist")]
        name: String,
    },
}

impl Seed {
    /// The seed's artist.
    pub fn artist(&self) -> &str {
        match self {
            Seed::Song { artist, .. } => artist.as_str(),
            Seed::Artist { name } => name.as_str(),
        }
    }

    /// The seed song's title; none for an artist.
    pub fn title(&self) -> Option<&str> {
        match self {
            Seed::Song { title, .. } => Some(title.as_str()),
            Seed::Artist { .. } => None,
        }
    }
}

/// An artist whose songs a station plays: one Last.fm named as like the seed, or the seed's own.
#[derive(Clone, Debug, PartialEq)]
pub struct Neighbor {
    /// The name as Last.fm writes it (the seed's own: as the seed has it), to search for.
    pub name: String,
    /// The artist as songs are grouped: the first one a credit names, folded (`shuffle::artist_key`).
    pub key: String,
    /// The songs by this artist Last.fm named, best first, as Last.fm writes them: these are wanted first.
    pub titles: Vec<String>,
}

/// One station's taste, the songs it has in hand, and what it has given.
#[derive(Clone, Debug)]
pub struct Station {
    seed: Seed,
    /// The most songs by the seed's artist in a batch.
    seed_per_batch: usize,
    /// The seed's artist as songs are grouped.
    seed_key: String,
    /// The songs by the seed's artist Last.fm named.
    seed_titles: Vec<String>,
    /// The listener chose not to play the seed's artist: none of their songs come.
    seed_blocked: bool,
    /// The seed's artist was handed out to be filled.
    seed_opened: bool,
    /// The questions taken, answered or not.
    asked: Vec<Ask>,
    /// How many artists the song question named that the station can use (not the seed's own).
    song_named: usize,
    /// Artists named and not opened yet, in Last.fm's order.
    neighbors: VecDeque<Neighbor>,
    /// Every artist named so far that the station can use, and the seed's: none is named twice.
    known: HashSet<String>,
    /// The artists opened, in the order they were: the seed's first.
    opened: Vec<String>,
    /// Each opened artist's songs not given yet.
    pools: HashMap<String, VecDeque<QueueItem>>,
    /// The identities of every song given, and of the seed song.
    had: HashSet<String>,
    /// Rounds of opening since the last batch.
    rounds: usize,
    spent: bool,
}

impl Station {
    /// A station from `seed`, giving at most `seed_per_batch` songs by the seed's artist in a batch (3 for a radio,
    /// 2 for autoplay, whose seed's artist was just heard).
    pub fn new(seed: Seed, seed_per_batch: usize) -> Station {
        let seed_key = artist_key(seed.artist());
        let mut had = HashSet::new();
        if let Seed::Song { title, artist } = &seed {
            had.insert(words_identity(title, artist));
            had.insert(song_identity(title, artist));
        }
        let known = HashSet::from([seed_key.clone()]);
        Station {
            seed,
            seed_per_batch,
            seed_key,
            seed_titles: Vec::new(),
            seed_blocked: false,
            seed_opened: false,
            asked: Vec::new(),
            song_named: 0,
            neighbors: VecDeque::new(),
            known,
            opened: Vec::new(),
            pools: HashMap::new(),
            had,
            rounds: 0,
            spent: false,
        }
    }

    /// What the station started from.
    pub fn seed(&self) -> &Seed {
        &self.seed
    }

    /// The seed's artist as songs are grouped: the key [`Station::to_fill`] gives it.
    pub fn seed_key(&self) -> &str {
        &self.seed_key
    }

    /// The station's name: "Teardrop radio", "Massive Attack radio".
    pub fn label(&self) -> String {
        match &self.seed {
            Seed::Song { title, .. } => format!("{title} radio"),
            Seed::Artist { name } => format!("{name} radio"),
        }
    }

    /// The seed's page on Last.fm.
    pub fn link(&self) -> String {
        self.song_ask().unwrap_or_else(|| self.artist_ask()).link()
    }

    /// The next question for Last.fm, if there is one; never more than two in a station's life.
    ///
    /// A song's station asks for the song's similar tracks first. It asks for the artist's similar artists only
    /// when that first answer named fewer than [`FEW`] artists it can use (none, when Last.fm did not know the song
    /// or could not be asked), or when the station has used the artists it named: fewer are left to open than one
    /// round takes, and the songs in hand cannot fill a batch. An artist's station asks for similar artists only.
    pub fn wants(&self) -> Option<Ask> {
        if self.spent || self.asked.len() >= QUESTIONS {
            return None;
        }
        let artist = self.artist_ask();
        let Some(song) = self.song_ask() else {
            return (!self.asked.contains(&artist)).then_some(artist);
        };
        if !self.asked.contains(&song) {
            return Some(song);
        }
        if self.asked.contains(&artist) {
            return None;
        }
        let few = self.song_named < FEW;
        let used = self.neighbors.len() < NEW_ARTISTS && self.capacity() < BATCH;
        (few || used).then_some(artist)
    }

    /// Takes Last.fm's answer to `ask`; `None` when it could not be asked (offline, Last.fm busy or out of reach,
    /// the day's questions used). Either way the question counts. A question that is not this station's, or one
    /// already taken, is passed over.
    ///
    /// The artists named are kept in Last.fm's order, grouped as songs are (so "The xx" and "the xx" are one, and
    /// "A feat. B" is A), each with the titles Last.fm named by them. Left out: the seed's own artist (whose titles
    /// become the seed's wanted songs, without the seed song itself), artists the listener chose not to play, names
    /// that fold to nothing, and artists named before. `blocked_artist` is asked with the name as Last.fm writes it
    /// and with its folded first artist.
    pub fn take(
        &mut self,
        ask: &Ask,
        answer: Option<&Answer>,
        blocked_artist: &dyn Fn(&str) -> bool,
    ) {
        let from_song = self.song_ask().as_ref() == Some(ask);
        if self.asked.contains(ask) || !(from_song || *ask == self.artist_ask()) {
            return;
        }
        self.asked.push(ask.clone());
        self.seed_blocked = self.seed_blocked
            || blocked_artist(self.seed.artist())
            || blocked_artist(self.seed_key.as_str());
        let Some(Answer::Found(likes)) = answer else {
            return;
        };
        // The artists this answer names, and where each one is in `neighbors`.
        let mut named: HashMap<String, usize> = HashMap::new();
        for like in likes {
            let name = like.artist.trim();
            let key = artist_key(name);
            let title = like.title.as_deref().filter(|_| from_song);
            if key.is_empty() {
                continue;
            }
            if key == self.seed_key {
                if let Some(title) = title {
                    self.want_seed_title(title);
                }
                continue;
            }
            if let Some(&at) = named.get(&key) {
                let neighbor = &mut self.neighbors[at];
                if let Some(title) = title {
                    add_title(&mut neighbor.titles, title);
                }
                // "A" names the artist better than "A feat. B" does.
                if fold(&neighbor.name) != key && fold(name) == key {
                    neighbor.name = name.to_string();
                }
                continue;
            }
            if self.known.contains(&key) || blocked_artist(name) || blocked_artist(key.as_str()) {
                continue;
            }
            self.known.insert(key.clone());
            let mut titles = Vec::new();
            if let Some(title) = title {
                add_title(&mut titles, title);
            }
            named.insert(key.clone(), self.neighbors.len());
            self.neighbors.push_back(Neighbor {
                name: name.to_string(),
                key,
                titles,
            });
        }
        if from_song {
            self.song_named = named.len();
        }
    }

    /// Whether Last.fm named anything the station can use: an artist like the seed's, or songs by the seed's artist.
    /// When it did not, the caller plays from the library instead.
    pub fn found(&self) -> bool {
        self.known.len() > 1 || !self.seed_titles.is_empty()
    }

    /// The artists whose songs the next batch needs, for the caller to find and hand to [`Station::fill`]: the
    /// seed's own artist first (once), then, when the songs in hand cannot fill a batch, up to [`NEW_ARTISTS`]
    /// artists not opened yet, in Last.fm's order. At most two rounds before each batch, and each artist is handed
    /// out once in a station's life. Empty when nothing more is needed or allowed before the batch.
    pub fn to_fill(&mut self) -> Vec<Neighbor> {
        if self.spent || self.rounds >= ROUNDS {
            return Vec::new();
        }
        let mut open = Vec::new();
        if !self.seed_opened {
            self.seed_opened = true;
            if self.seed_per_batch > 0 && !self.seed_blocked {
                self.opened.push(self.seed_key.clone());
                self.pools.insert(self.seed_key.clone(), VecDeque::new());
                open.push(Neighbor {
                    name: self.seed.artist().to_string(),
                    key: self.seed_key.clone(),
                    titles: self.seed_titles.clone(),
                });
            }
        }
        if self.capacity() < BATCH {
            for _ in 0..NEW_ARTISTS {
                let Some(neighbor) = self.neighbors.pop_front() else {
                    break;
                };
                self.opened.push(neighbor.key.clone());
                self.pools.insert(neighbor.key.clone(), VecDeque::new());
                open.push(neighbor);
            }
        }
        if !open.is_empty() {
            self.rounds += 1;
        }
        open
    }

    /// How many songs to find for an artist: [`SEED_POOL`] for the seed's own, [`POOL`] for any other.
    pub fn pool_size(&self, key: &str) -> usize {
        if key == self.seed_key {
            SEED_POOL
        } else {
            POOL
        }
    }

    /// Hands in songs for the artist `key` (one [`Station::to_fill`] gave), best first, already filtered by the
    /// caller. Songs the station has given or holds already (by any of their identities), and the seed song itself,
    /// are dropped; at most [`Station::pool_size`] are kept in hand. Songs for an artist not handed out are ignored.
    pub fn fill(&mut self, key: &str, songs: Vec<QueueItem>) {
        let room = match self.pools.get(key) {
            Some(pool) => self.pool_size(key).saturating_sub(pool.len()),
            None => return,
        };
        let mut held: HashSet<String> =
            self.pools.values().flatten().flat_map(identities).collect();
        let mut kept = Vec::new();
        for song in songs {
            if kept.len() >= room {
                break;
            }
            let ids = identities(&song);
            if self.is_seed_song(&song)
                || ids
                    .iter()
                    .any(|id| self.had.contains(id) || held.contains(id))
            {
                continue;
            }
            held.extend(ids);
            kept.push(song);
        }
        if let Some(pool) = self.pools.get_mut(key) {
            pool.extend(kept);
        }
    }

    /// The next songs to add, up to [`BATCH`]: at most the seed's share by the seed's artist and [`PER_BATCH`] by
    /// any other, the seed's artist first and then the others in Last.fm's order. None whose identity is in `queued`
    /// (the identities, as [`identities`] gives them, of what the queue holds and what stations queued lately) or
    /// was given before. They come in an order where the same artist is not twice in a row when that can be helped,
    /// starting after `last_artist` (the credit of the song they follow). What is given is never given again.
    ///
    /// The station is spent when every artist named is used and no song is left in hand (or a batch comes back empty
    /// with no artist left to open), and no question is left for Last.fm that could name more. An empty batch with
    /// artists still to open is not the end: the caller may open more and ask again.
    pub fn batch(&mut self, queued: &HashSet<String>, last_artist: Option<&str>) -> Vec<QueueItem> {
        self.rounds = 0;
        if self.spent {
            return Vec::new();
        }
        let mut picked: Vec<(QueueItem, String)> = Vec::new();
        for key in &self.opened {
            let share = self.share(key);
            let Some(pool) = self.pools.get_mut(key) else {
                continue;
            };
            let mut given = 0;
            while given < share && picked.len() < BATCH {
                let Some(item) = pool.pop_front() else {
                    break;
                };
                let ids = identities(&item);
                if ids
                    .iter()
                    .any(|id| self.had.contains(id) || queued.contains(id))
                {
                    continue;
                }
                self.had.extend(ids);
                let artist = artist_key(&item.artist);
                picked.push((item, artist));
                given += 1;
            }
        }
        let items = apart(picked, last_artist.map(artist_key));
        let used = self.neighbors.is_empty() && (self.capacity() == 0 || items.is_empty());
        if used && self.wants().is_none() {
            self.spent = true;
        }
        items
    }

    /// Whether the station has nothing more to give: it no longer grows.
    pub fn spent(&self) -> bool {
        self.spent
    }

    /// The question about the seed song, for a song's station.
    fn song_ask(&self) -> Option<Ask> {
        match &self.seed {
            Seed::Song { title, artist } => Some(Ask::Song {
                artist: artist.clone(),
                title: title.clone(),
            }),
            Seed::Artist { .. } => None,
        }
    }

    /// The question about the seed's artist.
    fn artist_ask(&self) -> Ask {
        Ask::Artist {
            artist: self.seed.artist().to_string(),
        }
    }

    /// Keeps a title Last.fm named by the seed's artist, unless it is the seed song itself.
    fn want_seed_title(&mut self, title: &str) {
        if let Seed::Song { title: seed, .. } = &self.seed
            && same_title(title, seed)
        {
            return;
        }
        add_title(&mut self.seed_titles, title);
    }

    /// Whether `item` is the seed song itself, in this release or another ("Teardrop - 2011 Remaster").
    fn is_seed_song(&self, item: &QueueItem) -> bool {
        match &self.seed {
            Seed::Song { title, .. } => {
                artist_key(&item.artist) == self.seed_key && same_title(&item.title, title)
            }
            Seed::Artist { .. } => false,
        }
    }

    /// The most songs by the artist `key` in a batch.
    fn share(&self, key: &str) -> usize {
        if key == self.seed_key {
            self.seed_per_batch
        } else {
            PER_BATCH
        }
    }

    /// How many songs the opened artists can give to the next batch, each within its share.
    fn capacity(&self) -> usize {
        self.opened
            .iter()
            .map(|key| {
                let fresh = self.pools.get(key).map_or(0, |pool| {
                    pool.iter().filter(|item| self.fresh(item)).count()
                });
                fresh.min(self.share(key))
            })
            .sum()
    }

    /// Whether `item` has not been given (by any of its identities).
    fn fresh(&self, item: &QueueItem) -> bool {
        identities(item).iter().all(|id| !self.had.contains(id))
    }
}

/// The ways a song in the queue is known, so it comes once however it is known: "id:N" for a song of the library,
/// "addon:<add-on>:<track>" for an add-on's track, "words:<title|artist, folded>", so a library song and an add-on's
/// track with the same words are one song, and always [`song_identity`]'s, so "Angel - 2006 Remaster" by "Massive
/// Attack, Horace Andy" is "Angel" by Massive Attack.
pub fn identities(item: &QueueItem) -> Vec<String> {
    let mut ids = Vec::with_capacity(4);
    if let Some(id) = item.id {
        ids.push(format!("id:{id}"));
    }
    if let Some(addon) = &item.addon {
        ids.push(format!("addon:{}:{}", addon.addon, addon.track));
    }
    ids.push(words_identity(&item.title, &item.artist));
    ids.push(song_identity(&item.title, &item.artist));
    ids
}

/// A song as it is heard: its title without a release label ("- 2011 Remaster", "(Mono)"), by the first artist its
/// credit names. One per song, so this is what a run's stations remember of what they added.
pub fn song_identity(title: &str, artist: &str) -> String {
    format!("song:{}|{}", artist_key(artist), title_key(title))
}

/// A song's identity by its words.
fn words_identity(title: &str, artist: &str) -> String {
    format!("words:{}", words_key(title, artist))
}

/// Adds `title` to `titles`, unless it is blank or already there (as folded words).
fn add_title(titles: &mut Vec<String>, title: &str) {
    let title = title.trim();
    let folded = fold(title);
    if !folded.is_empty() && !titles.iter().any(|t| fold(t) == folded) {
        titles.push(title.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::AddonTrack;
    use crate::similar::Like;

    /// A song of the library.
    fn row(id: i64, title: &str, artist: &str) -> QueueItem {
        QueueItem {
            id: Some(id),
            title: title.to_string(),
            artist: artist.to_string(),
            album: None,
            addon: None,
            art: None,
            duration_ms: None,
            qid: 0,
            locator: format!("C:/Music/{id}.flac"),
        }
    }

    /// A song an add-on has.
    fn track(id: &str, title: &str, artist: &str) -> QueueItem {
        QueueItem {
            id: None,
            title: title.to_string(),
            artist: artist.to_string(),
            album: None,
            addon: Some(AddonTrack {
                addon: "demo".to_string(),
                track: id.to_string(),
            }),
            art: None,
            duration_ms: None,
            qid: 0,
            locator: String::new(),
        }
    }

    fn like(artist: &str, title: Option<&str>) -> Like {
        Like {
            artist: artist.to_string(),
            title: title.map(str::to_string),
            score: 0.5,
        }
    }

    /// Last.fm's answer naming these artists.
    fn artists(names: &[&str]) -> Answer {
        Answer::Found(names.iter().map(|name| like(name, None)).collect())
    }

    /// Last.fm's answer naming these songs, as (artist, title).
    fn songs(named: &[(&str, &str)]) -> Answer {
        Answer::Found(
            named
                .iter()
                .map(|(artist, title)| like(artist, Some(title)))
                .collect(),
        )
    }

    fn no_one(_: &str) -> bool {
        false
    }

    fn teardrop() -> Seed {
        Seed::Song {
            title: "Teardrop".to_string(),
            artist: "Massive Attack".to_string(),
        }
    }

    fn song_ask() -> Ask {
        Ask::Song {
            artist: "Massive Attack".to_string(),
            title: "Teardrop".to_string(),
        }
    }

    fn artist_ask() -> Ask {
        Ask::Artist {
            artist: "Massive Attack".to_string(),
        }
    }

    /// "Artist 1" to "Artist n".
    fn numbered(n: usize) -> Vec<String> {
        (1..=n).map(|k| format!("Artist {k}")).collect()
    }

    /// An artist's station that took Last.fm's answer naming `names`.
    fn artist_station(seed: &str, names: &[String], seed_per_batch: usize) -> Station {
        let mut station = Station::new(
            Seed::Artist {
                name: seed.to_string(),
            },
            seed_per_batch,
        );
        let ask = station.wants().expect("an artist's station asks");
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        station.take(&ask, Some(&artists(&names)), &no_one);
        station
    }

    /// Fills every artist the station opens before a batch, as the caller does: a full pool of songs each, every
    /// other one an add-on's, each handed in with a copy known the other way (the same words, another identity).
    fn fill_all(station: &mut Station, next: &mut i64) {
        loop {
            let open = station.to_fill();
            if open.is_empty() {
                break;
            }
            for artist in open {
                let mut list = Vec::new();
                for _ in 0..station.pool_size(&artist.key) {
                    *next += 1;
                    let n = *next;
                    let title = format!("Song {n}");
                    let as_row = row(n, &title, &artist.name);
                    let as_track = track(&format!("t{n}"), &title, &artist.name);
                    if n % 2 == 0 {
                        list.extend([as_row, as_track]);
                    } else {
                        list.extend([as_track, as_row]);
                    }
                }
                station.fill(&artist.key, list);
            }
        }
    }

    fn keys(open: &[Neighbor]) -> Vec<String> {
        open.iter().map(|n| n.key.clone()).collect()
    }

    fn names(open: &[Neighbor]) -> Vec<String> {
        open.iter().map(|n| n.name.clone()).collect()
    }

    fn titles(items: &[QueueItem]) -> Vec<String> {
        let mut titles: Vec<String> = items.iter().map(|i| i.title.clone()).collect();
        titles.sort();
        titles
    }

    #[test]
    fn a_station_keeps_its_shares_and_spreads_artists() {
        for seed_share in [3, 2] {
            let mut station = artist_station("Massive Attack", &numbered(10), seed_share);
            let mut next = 0;
            let mut seen = HashSet::new();
            let mut last = Some("Massive Attack".to_string());
            for round in 0..5 {
                fill_all(&mut station, &mut next);
                let batch = station.batch(&HashSet::new(), last.as_deref());
                assert_eq!(batch.len(), BATCH, "batch {round}");
                let mut counts: HashMap<String, usize> = HashMap::new();
                for item in &batch {
                    *counts.entry(artist_key(&item.artist)).or_default() += 1;
                }
                for (artist, n) in &counts {
                    let share = if artist == "massive attack" {
                        seed_share
                    } else {
                        PER_BATCH
                    };
                    assert!(*n <= share, "{n} by {artist} in batch {round}");
                }
                if round == 0 {
                    assert_eq!(counts.get("massive attack"), Some(&seed_share));
                }
                // Never the same artist twice in a row, nor right after the one heard last.
                let mut before = last.as_deref().map(artist_key);
                for item in &batch {
                    let now = artist_key(&item.artist);
                    assert_ne!(before.as_ref(), Some(&now), "batch {round}");
                    before = Some(now);
                    // No song twice, whether it came as a row or as an add-on's track.
                    for id in identities(item) {
                        assert!(seen.insert(id.clone()), "{id} came twice");
                    }
                }
                last = batch.last().map(|item| item.artist.clone());
            }
            assert!(!station.spent());

            // On until every artist is used: each song once, then spent.
            let mut given = 50;
            for _ in 0..20 {
                fill_all(&mut station, &mut next);
                let batch = station.batch(&HashSet::new(), last.as_deref());
                for item in &batch {
                    for id in identities(item) {
                        assert!(seen.insert(id.clone()), "{id} came twice");
                    }
                }
                given += batch.len();
                last = batch.last().map(|item| item.artist.clone());
                if station.spent() {
                    break;
                }
            }
            assert!(station.spent());
            assert_eq!(given, SEED_POOL + 10 * POOL);
            assert!(station.batch(&HashSet::new(), None).is_empty());
            assert!(station.to_fill().is_empty());
        }
    }

    #[test]
    fn a_song_comes_once_whatever_it_is_known_by() {
        let five: Vec<String> = ["B", "C", "D", "E", "F"].map(String::from).to_vec();
        let mut station = artist_station("A", &five, 3);
        assert_eq!(keys(&station.to_fill()), ["a", "b", "c", "d", "e"]);
        station.fill(
            "a",
            vec![
                row(1, "One", "A"),
                row(1, "One again", "A"),
                row(2, "Two", "A"),
            ],
        );
        station.fill(
            "b",
            vec![
                row(10, "Twin", "B"),
                track("t1", "Twin", "B"),
                row(11, "Solo", "B"),
            ],
        );
        station.fill("c", vec![row(20, "Cee", "C"), track("t2", "Queued", "C")]);
        // Songs for an artist not handed out are ignored.
        station.fill("f", vec![row(30, "Eff", "F")]);
        let queued = HashSet::from(["addon:demo:t2".to_string()]);
        let first = station.batch(&queued, None);
        assert_eq!(titles(&first), ["Cee", "One", "Solo", "Twin", "Two"]);
        let twin = first.iter().find(|item| item.title == "Twin");
        assert_eq!(twin.and_then(|item| item.id), Some(10));

        // The same song from an add-on, handed in after the library's was given, does not come.
        station.fill(
            "b",
            vec![
                track("t1", "Twin", "B"),
                track("t3", "Twin", "b"),
                track("t4", "New", "B"),
            ],
        );
        let second = station.batch(&HashSet::new(), Some("B"));
        assert_eq!(titles(&second), ["New"]);
    }

    #[test]
    fn the_seed_song_itself_never_comes() {
        let mut station = Station::new(teardrop(), 3);
        station.take(&song_ask(), Some(&Answer::NotFound), &no_one);
        let four: Vec<&str> = vec!["Portishead", "Tricky", "Burial", "Unkle"];
        station.take(&artist_ask(), Some(&artists(&four)), &no_one);
        station.to_fill();
        station.fill(
            "massive attack",
            vec![
                row(1, "Teardrop", "Massive Attack"),
                track("t1", "Teardrop - 2011 Remaster", "Massive Attack"),
                row(2, "Angel", "Massive Attack"),
            ],
        );
        let batch = station.batch(&HashSet::new(), Some("Massive Attack"));
        assert_eq!(titles(&batch), ["Angel"]);
    }

    #[test]
    fn a_batch_opens_new_artists_only_as_it_needs_them() {
        let mut station = artist_station("Seed", &numbered(20), 3);
        let first = station.to_fill();
        assert_eq!(
            keys(&first),
            ["seed", "artist 1", "artist 2", "artist 3", "artist 4"]
        );
        let seed_songs = (1..=12)
            .map(|n| row(n, &format!("Seed {n}"), "Seed"))
            .collect();
        station.fill("seed", seed_songs);
        for (n, artist) in first.iter().skip(1).enumerate() {
            station.fill(&artist.key, vec![row(100 + n as i64, "Only", &artist.name)]);
        }
        // Three by the seed's artist and four others cannot fill ten: four more are opened.
        let second = station.to_fill();
        assert_eq!(
            keys(&second),
            ["artist 5", "artist 6", "artist 7", "artist 8"]
        );
        // No third round before one batch, however short it is.
        assert!(station.to_fill().is_empty());
        assert_eq!(station.batch(&HashSet::new(), None).len(), 7);

        // The next batch opens the next four, once each.
        let third = station.to_fill();
        assert_eq!(
            keys(&third),
            ["artist 9", "artist 10", "artist 11", "artist 12"]
        );
        for (n, artist) in third.iter().enumerate() {
            let list = (0..POOL as i64)
                .map(|k| row(200 + 10 * n as i64 + k, &format!("Song {k}"), &artist.name))
                .collect();
            station.fill(&artist.key, list);
        }
        // Three by the seed's artist and eight others can: nothing more is opened.
        assert!(station.to_fill().is_empty());
        assert_eq!(station.batch(&HashSet::new(), None).len(), BATCH);
    }

    #[test]
    fn a_station_is_spent_when_it_has_nothing_left_to_give() {
        // Every artist used and nothing left in hand.
        let mut station = artist_station("A", &["B".to_string()], 3);
        station.to_fill();
        station.fill("a", vec![row(1, "One", "A")]);
        station.fill("b", vec![row(2, "Two", "B")]);
        assert!(!station.spent());
        assert_eq!(station.batch(&HashSet::new(), None).len(), 2);
        assert!(station.spent());
        assert!(station.batch(&HashSet::new(), None).is_empty());
        assert!(station.to_fill().is_empty());

        // A batch that comes back empty with artists still to open is not the end: more are opened, and once none
        // is left, an empty batch is.
        let mut station = artist_station("A", &numbered(9), 3);
        while !station.to_fill().is_empty() {}
        assert!(station.batch(&HashSet::new(), None).is_empty());
        assert!(!station.spent());
        assert_eq!(station.to_fill().len(), 1);
        assert!(station.batch(&HashSet::new(), None).is_empty());
        assert!(station.spent());

        // Not while a question is left that could name more artists.
        let mut station = Station::new(teardrop(), 3);
        let five = songs(&[
            ("Portishead", "Roads"),
            ("Tricky", "Overcome"),
            ("Burial", "Archangel"),
            ("Unkle", "Rabbit in Your Headlights"),
            ("Morcheeba", "Trigger Hippie"),
        ]);
        station.take(&song_ask(), Some(&five), &no_one);
        let mut id = 0;
        loop {
            let open = station.to_fill();
            if open.is_empty() {
                break;
            }
            for artist in open {
                id += 1;
                station.fill(&artist.key, vec![row(id, "Only", &artist.name)]);
            }
        }
        assert_eq!(station.batch(&HashSet::new(), None).len(), 6);
        assert!(!station.spent());
        assert_eq!(station.wants(), Some(artist_ask()));
        station.take(&artist_ask(), Some(&Answer::NotFound), &no_one);
        assert!(station.batch(&HashSet::new(), None).is_empty());
        assert!(station.spent());
        assert_eq!(station.wants(), None);
    }

    #[test]
    fn a_song_station_asks_for_similar_artists_only_when_it_must() {
        let five = songs(&[
            ("Portishead", "Roads"),
            ("Tricky", "Overcome"),
            ("Burial", "Archangel"),
            ("Unkle", "Rabbit in Your Headlights"),
            ("Morcheeba", "Trigger Hippie"),
        ]);

        // Last.fm does not know the song: the artist is asked next, and then nothing more.
        let mut station = Station::new(teardrop(), 3);
        assert_eq!(station.wants(), Some(song_ask()));
        station.take(&song_ask(), Some(&Answer::NotFound), &no_one);
        assert_eq!(station.wants(), Some(artist_ask()));
        station.take(
            &artist_ask(),
            Some(&artists(&["Portishead", "Tricky"])),
            &no_one,
        );
        assert_eq!(station.wants(), None);
        assert!(station.found());

        // Too few artists besides the seed's own: songs by Massive Attack do not count.
        let mut station = Station::new(teardrop(), 3);
        let few = songs(&[
            ("Massive Attack", "Angel"),
            ("Portishead", "Roads"),
            ("Portishead", "Glory Box"),
            ("Tricky", "Overcome"),
            ("Massive Attack", "Unfinished Sympathy"),
            ("Burial", "Archangel"),
        ]);
        station.take(&song_ask(), Some(&few), &no_one);
        assert_eq!(station.wants(), Some(artist_ask()));

        // Enough: no second question until the station has used the artists named.
        let mut station = Station::new(teardrop(), 3);
        station.take(&song_ask(), Some(&five), &no_one);
        let mut next = 0;
        for round in 0..3 {
            assert_eq!(station.wants(), None, "before batch {round}");
            fill_all(&mut station, &mut next);
            assert_eq!(station.batch(&HashSet::new(), None).len(), BATCH);
        }
        // One artist is left to open and the songs in hand cannot fill a batch.
        assert_eq!(station.wants(), Some(artist_ask()));
        let more = artists(&["Portishead", "Zero 7"]);
        station.take(&artist_ask(), Some(&more), &no_one);
        assert_eq!(station.wants(), None);
        assert_eq!(names(&station.to_fill()), ["Morcheeba", "Zero 7"]);
        assert_eq!(station.wants(), None);
    }

    #[test]
    fn a_question_that_could_not_be_asked_counts_and_leads_to_the_next() {
        let mut station = Station::new(teardrop(), 3);
        let ask = station.wants().expect("the song is asked first");
        station.take(&ask, None, &no_one);
        assert_eq!(station.wants(), Some(artist_ask()));
        // The same question taken again counts once.
        station.take(&ask, Some(&artists(&["Portishead"])), &no_one);
        assert_eq!(station.wants(), Some(artist_ask()));
        station.take(&artist_ask(), None, &no_one);
        assert_eq!(station.wants(), None);
        assert!(!station.found());
        // Only the seed's own artist is left to fill, with nothing wanted first.
        let seed = Neighbor {
            name: "Massive Attack".to_string(),
            key: "massive attack".to_string(),
            titles: Vec::new(),
        };
        assert_eq!(station.to_fill(), [seed]);
    }

    #[test]
    fn an_artist_station_asks_once() {
        let mut station = Station::new(
            Seed::Artist {
                name: "Massive Attack".to_string(),
            },
            3,
        );
        // A question that is not this station's is passed over.
        station.take(&song_ask(), Some(&artists(&["Portishead"])), &no_one);
        assert!(!station.found());
        assert_eq!(station.wants(), Some(artist_ask()));
        station.take(&artist_ask(), Some(&artists(&["Portishead"])), &no_one);
        assert!(station.found());
        assert_eq!(station.wants(), None);
        // Not even once every artist named is used.
        while !station.to_fill().is_empty() {}
        assert_eq!(station.wants(), None);

        let mut station = Station::new(
            Seed::Artist {
                name: "Massive Attack".to_string(),
            },
            3,
        );
        station.take(&artist_ask(), Some(&Answer::NotFound), &no_one);
        assert_eq!(station.wants(), None);
        assert!(!station.found());
    }

    #[test]
    fn last_fms_titles_reach_the_artists_and_names_are_grouped() {
        let mut station = Station::new(teardrop(), 3);
        let named = songs(&[
            ("The xx", "Intro"),
            ("Massive Attack", "Teardrop - 2011 Remaster"),
            ("Massive Attack", "Angel"),
            ("A feat. B", "Duet"),
            ("the xx", "Islands"),
            ("Massive Attack feat. Elizabeth Fraser", "Black Milk"),
            ("A", "Solo"),
            ("THE XX", "intro"),
        ]);
        station.take(&song_ask(), Some(&named), &no_one);
        let open = station.to_fill();
        fn neighbor(name: &str, key: &str, titles: &[&str]) -> Neighbor {
            Neighbor {
                name: name.to_string(),
                key: key.to_string(),
                titles: titles.iter().map(|t| t.to_string()).collect(),
            }
        }
        assert_eq!(
            open,
            [
                neighbor("Massive Attack", "massive attack", &["Angel", "Black Milk"]),
                neighbor("The xx", "the xx", &["Intro", "Islands"]),
                neighbor("A", "a", &["Duet", "Solo"]),
            ]
        );
        // Two artists besides the seed's own are too few.
        assert_eq!(station.wants(), Some(artist_ask()));
        // An artist named again by the next answer is not named twice.
        let more = artists(&["the xx", "A", "Zero 7"]);
        station.take(&artist_ask(), Some(&more), &no_one);
        assert_eq!(names(&station.to_fill()), ["Zero 7"]);
    }

    #[test]
    fn artists_chosen_not_to_play_are_never_opened() {
        let blocked = |name: &str| matches!(fold(name).as_str(), "tricky" | "burial");
        let mut station = Station::new(teardrop(), 3);
        let named = songs(&[
            ("Tricky", "Overcome"),
            ("Portishead", "Roads"),
            ("Burial feat. Someone", "Archangel"),
            ("TRICKY", "Hell Is Round the Corner"),
            ("Unkle", "Rabbit in Your Headlights"),
            ("???", "Nothing"),
            ("Zero 7", "Destiny"),
        ]);
        station.take(&song_ask(), Some(&named), &blocked);
        assert_eq!(
            names(&station.to_fill()),
            ["Massive Attack", "Portishead", "Unkle", "Zero 7"]
        );
        // Three left once those are dropped: too few, so the artist is asked as well.
        assert_eq!(station.wants(), Some(artist_ask()));
        station.take(
            &artist_ask(),
            Some(&artists(&["Burial", "Morcheeba"])),
            &blocked,
        );
        assert_eq!(names(&station.to_fill()), ["Morcheeba"]);

        // The seed's own artist, chosen not to play, is never opened.
        let mut station = Station::new(teardrop(), 3);
        let seed_blocked = |name: &str| fold(name) == "massive attack";
        let named = songs(&[("Portishead", "Roads"), ("Massive Attack", "Angel")]);
        station.take(&song_ask(), Some(&named), &seed_blocked);
        assert_eq!(names(&station.to_fill()), ["Portishead"]);
    }

    #[test]
    fn a_song_is_known_by_its_row_its_add_on_track_and_its_words() {
        assert_eq!(
            identities(&row(7, "Teardrop", "Massive Attack")),
            [
                "id:7",
                "words:teardrop|massive attack",
                "song:massive attack|teardrop"
            ]
        );
        assert_eq!(
            identities(&track(
                "t1",
                "Teardrop (feat. Elizabeth Fraser)",
                "MASSIVE ATTACK"
            )),
            [
                "addon:demo:t1",
                "words:teardrop|massive attack",
                "song:massive attack|teardrop"
            ]
        );
        let mut both = track("t1", "Teardrop", "Massive Attack");
        both.id = Some(7);
        assert_eq!(
            identities(&both),
            [
                "id:7",
                "addon:demo:t1",
                "words:teardrop|massive attack",
                "song:massive attack|teardrop"
            ]
        );
        // A release label and a longer credit are the same song.
        assert!(
            identities(&row(
                8,
                "Teardrop - 2006 Remaster",
                "Massive Attack, Elizabeth Fraser"
            ))
            .contains(&"song:massive attack|teardrop".to_string())
        );
    }

    #[test]
    fn a_station_is_named_after_its_seed() {
        let song = Station::new(teardrop(), 3);
        assert_eq!(song.label(), "Teardrop radio");
        assert_eq!(song.link(), song_ask().link());
        assert_eq!(song.seed_key(), "massive attack");
        let artist = Station::new(
            Seed::Artist {
                name: "Massive Attack".to_string(),
            },
            2,
        );
        assert_eq!(artist.label(), "Massive Attack radio");
        assert_eq!(artist.link(), artist_ask().link());
        assert_eq!(
            serde_json::to_value(teardrop()).expect("a seed is written"),
            serde_json::json!({"title": "Teardrop", "artist": "Massive Attack"})
        );
        assert_eq!(
            serde_json::to_value(artist.seed()).expect("a seed is written"),
            serde_json::json!({"artist": "Massive Attack"})
        );
    }
}
