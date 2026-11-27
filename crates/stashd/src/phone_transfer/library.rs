//! The phone's library, sent with its songs ([`stash_core::phone::LIBRARY`], beside them in the transfer folder): its
//! likes, its playlists (its own, its Stash Mixes, and the playlists, albums, mixes and radios it synced from Spotify,
//! YouTube Music or SoundCloud) and its plays, as "Export for Stash on the web" writes them, with the send they go with.
//! Saved albums come in as playlists.
//!
//! It is read once for a send, when the send is offered (or taken without being offered), and summed up for the
//! window's first-run screen (`GET /api/phone/library`). The parts the listener chooses (`library` in
//! `POST /api/phone/take`) come in before the send's songs, in one import, once for the send. A song not sent as a file
//! comes in as a song that streams; a file sent lands on its song, as every file a take brings in does. Likes can be
//! chosen by where they were made (in Stash, on Spotify, YouTube Music or SoundCloud). What was left out is kept for
//! the phone (by its name: a playlist's id is that phone's), and each later send of its with a library brings in all
//! the rest by itself, so it stays live: likes and lists the phone gains come in, likes and the phone's own playlists
//! gain, and a playlist synced from a service (or a Stash Mix) is made what the phone has it as now. With some likes
//! left out, the songs taken do not bring the likes their list marks either (the list does not say where they were
//! made).
//!
//! A library file missing, unreadable, too big or of another send changes nothing: the send is its songs, as before.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::identity::{isrcs_agree, words_key};
use stash_core::import::web_library::{self, FromPhone};
use stash_core::import::{Incoming, Playlist, Song};
use stash_core::phone::PhoneFile;

use super::{Board, Choice, Ctx, Folder, List, Offer, Offered, as_song, in_library};
use crate::serve::now_ms;

/// The largest library file read: the most the reader takes.
const CAP: u64 = 64 * 1024 * 1024;
/// Reads of a library file the phone did not answer before its send is taken as one without.
const READS: u32 = 3;
/// What was chosen to bring in from each phone's library ([`Parts`]), by the phone's name (its playlists' ids are
/// that phone's): kept, and brought in from each later send of that phone's too. `{"Pixel 6 Pro": {…}}`.
pub(super) const CHOICE_SETTING: &str = "phone.library_choice";
/// The sends whose library came in, newest last, as `[phone, sent_at_ms]` (the last [`BROUGHT_KEPT`]): another try at
/// one of them, from that phone or another (or another Stash on it), does not bring it in again.
pub(super) const BROUGHT_SETTING: &str = "phone.library_taken";
const BROUGHT_KEPT: usize = 50;
/// How many songs are asked of the library at a time, with the library let go between: the window is answered
/// meanwhile.
const ASK_AT_ONCE: usize = 500;
/// The most albums told of a send, and covers of a playlist.
const ALBUMS: usize = 40;
const COVERS: usize = 4;

/// A send's library as read: once for the send, from the phone it came from.
pub(super) struct Read {
    phone: String,
    sent_at_ms: i64,
    got: Got,
}

enum Got {
    Library(Arc<FromPhone>),
    /// None came with the send, or none that can be used: the send is its songs.
    Without,
    /// The phone did not answer as it was read, this many times: read again at the next look.
    Unanswered(u32),
}

/// The library that came with the send made at `sent_at_ms` on `phone`, read from the phone the first time it is
/// asked for, and not again for that send. No lock is held while the phone is read (it can take a while). `listing`
/// is the folder's files as the look listed them. What came of it is logged once.
pub(super) fn of_send(
    ctx: &Ctx,
    phone: &str,
    folder: &dyn Folder,
    listing: &[PhoneFile],
    sent_at_ms: i64,
) -> Option<Arc<FromPhone>> {
    let tried = match ctx
        .board
        .library
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.phone == phone && r.sent_at_ms == sent_at_ms)
        .map(|r| &r.got)
    {
        Some(Got::Library(library)) => return Some(Arc::clone(library)),
        Some(Got::Without) => return None,
        Some(Got::Unanswered(n)) => *n,
        None => 0,
    };
    let (got, said) = read(folder, listing, sent_at_ms, tried);
    (ctx.log)(&said);
    let library = match &got {
        Got::Library(library) => Some(Arc::clone(library)),
        _ => None,
    };
    // One send's library kept for each phone: this one's in place of any before it.
    let mut kept = ctx.board.library.lock().unwrap();
    kept.retain(|r| r.phone != phone);
    kept.push(Read {
        phone: phone.to_string(),
        sent_at_ms,
        got,
    });
    library
}

/// Reads the library file among `listing`, read `tried` times before: what came of it, and what to log.
fn read(folder: &dyn Folder, listing: &[PhoneFile], sent_at_ms: i64, tried: u32) -> (Got, String) {
    let Some(file) = listing.iter().find(|f| f.name == stash_core::phone::LIBRARY) else {
        return (Got::Without, "no library came with the send: its songs only".into());
    };
    if file.size > CAP {
        let mb = file.size / 1_000_000;
        return (Got::Without, format!("the library that came with the send is too big ({mb} MB): its songs only"));
    }
    let bytes = match folder.bytes(file, CAP) {
        Ok(bytes) => bytes,
        Err(why) if tried + 1 < READS => {
            let said = format!("the library that came with the send was not read, read again at the next look: {why}");
            return (Got::Unanswered(tried + 1), said);
        }
        Err(why) => {
            return (Got::Without, format!("the library that came with the send was not read: its songs only: {why}"));
        }
    };
    match web_library::read_from_phone(&bytes, now_ms()) {
        Err(e) => (Got::Without, format!("the library that came with the send was not read ({e}): its songs only")),
        Ok(library) if library.sent_at_ms != Some(sent_at_ms) => (
            Got::Without,
            format!(
                "the library on the phone is another send's (sent at {:?}): its songs only",
                library.sent_at_ms
            ),
        ),
        Ok(library) => {
            let said = format!(
                "the phone's library came with the send: {} likes, {} playlists, {} plays",
                library.library.likes.len(),
                library.library.playlists.len(),
                library.library.plays.len()
            );
            (Got::Library(Arc::new(library)), said)
        }
    }
}

/// The send made at `sent_at_ms` is over (its list gone from the phone): its library is let go.
pub(super) fn let_go(board: &Board, sent_at_ms: i64) {
    board.library.lock().unwrap().retain(|r| r.sent_at_ms != sent_at_ms);
}

/// Which likes `POST /api/phone/take` asks to bring in: all, none, or those made on some services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Likes {
    All,
    None,
    /// Those made on any of these (of [`web_library::LIKED_ON`], in that order); a like that does not say where it was
    /// made counts as made in Stash.
    On(Vec<&'static str>),
}

/// What `POST /api/phone/take` asks to bring in from the phone's library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Asked {
    pub likes: Likes,
    pub plays: bool,
    /// The playlists chosen, by their ids in the phone's file.
    pub playlists: Vec<String>,
    /// Of each part, only the songs that are files here (sent as files with the send, or files here already): nothing
    /// comes in that only streams.
    pub only_here: bool,
}

/// What was left out of a phone's library when it was brought in, kept for the phone (by its name): each later send's
/// library comes in but for these, so likes made on a service since and lists the phone has gained come in too.
/// `{likes_left_out, all_likes_left_out, plays, playlists_left_out, only_here}`, every one of them, or no choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LeftOut {
    /// The services whose likes were left out (of those the library had likes from then).
    pub likes_left_out: Vec<String>,
    /// Every like left out, wherever it was made.
    pub all_likes_left_out: bool,
    /// Whether the plays come in.
    pub plays: bool,
    /// The playlists left out (of those the library had then), by their ids.
    pub playlists_left_out: Vec<String>,
    /// As [`Asked::only_here`].
    pub only_here: bool,
}

impl LeftOut {
    /// What `asked` leaves out of `from`: the services `from`'s likes were made on that it does not ask for, and the
    /// playlists `from` has that it does not choose.
    fn of(asked: &Asked, from: &FromPhone) -> LeftOut {
        let (likes_left_out, all_likes_left_out) = match &asked.likes {
            Likes::All => (Vec::new(), false),
            Likes::None => (Vec::new(), true),
            Likes::On(on) => {
                let left: Vec<String> = web_library::LIKED_ON
                    .into_iter()
                    .filter(|service| !on.contains(service))
                    .filter(|service| from.liked_on.iter().any(|made| made_on(made).contains(service)))
                    .map(String::from)
                    .collect();
                (left, false)
            }
        };
        let chosen: HashSet<&str> = asked.playlists.iter().map(String::as_str).collect();
        LeftOut {
            likes_left_out,
            all_likes_left_out,
            plays: asked.plays,
            playlists_left_out: from
                .library
                .playlists
                .iter()
                .map(|p| p.external_id.clone())
                .filter(|id| !chosen.contains(id.as_str()))
                .collect(),
            only_here: asked.only_here,
        }
    }

    /// Whether a like made on `made` comes in: one of the services it was made on was not left out.
    fn like(&self, made: &[&'static str]) -> bool {
        !self.all_likes_left_out
            && made_on(made)
                .iter()
                .any(|service| !self.likes_left_out.iter().any(|left| left == service))
    }

    /// Whether the likes the list marks on a send's songs come in with them: only when no like is left out, as the
    /// list does not say where a like was made.
    fn list_likes(&self) -> bool {
        !self.all_likes_left_out && self.likes_left_out.is_empty()
    }
}

/// Where a like was made, as the phone says (`made`): one that does not say was made in Stash.
fn made_on<'a>(made: &'a [&'static str]) -> &'a [&'static str] {
    if made.is_empty() { &["stash"] } else { made }
}

/// What `POST /api/phone/take` asks of the phone's library (its `library`), when it asks: `likes` true, false, or the
/// services the likes to bring in were made on; plays and only_here true or false (false when left out); playlists by
/// their ids.
pub(super) fn asked(body: &Value) -> Result<Option<Asked>, &'static str> {
    const LIKES: &str = "likes are true, false, or the services they were made on (stash, spotify, youtube, soundcloud)";
    let asked = &body["library"];
    if asked.is_null() {
        return Ok(None);
    }
    let asked = asked
        .as_object()
        .ok_or("library says what to bring in: likes, plays, playlists and only_here")?;
    let yes = |name: &str| match asked.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(v) => v.as_bool().ok_or("plays and only_here are true or false"),
    };
    let likes = match asked.get("likes") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Likes::None,
        Some(Value::Bool(true)) => Likes::All,
        Some(Value::Array(services)) => {
            let mut on: Vec<&'static str> = Vec::new();
            for service in services {
                let known = web_library::LIKED_ON
                    .into_iter()
                    .find(|k| Some(*k) == service.as_str())
                    .ok_or(LIKES)?;
                on.push(known);
            }
            Likes::On(web_library::LIKED_ON.into_iter().filter(|s| on.contains(s)).collect())
        }
        Some(_) => return Err(LIKES),
    };
    let mut playlists: Vec<String> = match asked.get("playlists") {
        None | Some(Value::Null) => Vec::new(),
        Some(ids) => ids
            .as_array()
            .ok_or("playlists are the playlists' ids")?
            .iter()
            .filter_map(|id| id.as_str().map(String::from))
            .collect(),
    };
    playlists.sort();
    playlists.dedup();
    Ok(Some(Asked {
        likes,
        plays: yes("plays")?,
        playlists,
        only_here: yes("only_here")?,
    }))
}

/// What `asked` leaves out of the library `offer` came with, when it can be brought in: the send came with one, and
/// every playlist chosen is in it (one that is not was gone from the phone since the window read the library).
pub(super) fn can_bring(offer: &Offer, asked: &Asked) -> Result<LeftOut, &'static str> {
    let from = offer
        .library
        .as_ref()
        .ok_or("the phone sent no library with these songs")?;
    let ids: HashSet<&str> = from.library.playlists.iter().map(|p| p.external_id.as_str()).collect();
    if asked.playlists.iter().any(|id| !ids.contains(id.as_str())) {
        return Err("the playlists on the phone changed: look them over and take them again");
    }
    Ok(LeftOut::of(asked, from))
}

/// What was left out for each phone, by its name. One kept in another shape (an earlier one's) is no choice.
fn kept_all(library: &Library) -> HashMap<String, LeftOut> {
    let all: HashMap<String, Value> = library
        .setting(CHOICE_SETTING)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    all.into_iter()
        .filter_map(|(phone, left)| Some((phone, serde_json::from_value(left).ok()?)))
        .collect()
}

/// `left_out` kept for `phone`: the send chosen from now, and each later send of the phone's that comes with a library,
/// bring in all but that. Another phone's stay as they are.
pub(super) fn keep(library: &Mutex<Library>, phone: &str, left_out: &LeftOut) {
    let mut library = library.lock().unwrap();
    let mut all = kept_all(&library);
    all.insert(phone.to_string(), left_out.clone());
    let _ = library.set_setting(CHOICE_SETTING, &json!(all).to_string());
}

/// What was left out of `phone`'s library, as kept.
fn kept(library: &Library, phone: &str) -> Option<LeftOut> {
    kept_all(library).remove(phone)
}

/// Whether a choice of what to bring in from `phone`'s library is kept (`library_choice_kept` in `GET /api/phone`):
/// its sends' libraries then come in by themselves, as that choice says.
pub(super) fn choice_kept(library: &Mutex<Library>, phone: &str) -> bool {
    kept(&library.lock().unwrap(), phone).is_some()
}

/// The sends whose library came in, newest last.
fn brought(library: &Library) -> Vec<(String, i64)> {
    library
        .setting(BROUGHT_SETTING)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The send made at `sent_at_ms` on `phone` has had its library brought in: kept with the last [`BROUGHT_KEPT`].
pub(super) fn mark_brought(library: &mut Library, phone: &str, sent_at_ms: i64) -> stash_core::Result<()> {
    let mut sends = brought(library);
    sends.retain(|(p, at)| !(p == phone && *at == sent_at_ms));
    sends.push((phone.to_string(), sent_at_ms));
    let over = sends.len().saturating_sub(BROUGHT_KEPT);
    sends.drain(..over);
    library.set_setting(BROUGHT_SETTING, &json!(sends).to_string())
}

/// What a take does with the phone's library: what was left out of it for the phone (no choice yet: the likes the list
/// marks come in, as ever), and the library to bring the rest in from while it is still to come in from this send.
pub(super) struct ForTake {
    pub kept: Option<LeftOut>,
    pub due: Option<Arc<FromPhone>>,
}

impl ForTake {
    /// Whether the likes the list marks on its songs come in with them: unless some of the phone's likes are left out.
    pub fn likes(&self) -> bool {
        self.kept.as_ref().is_none_or(LeftOut::list_likes)
    }
}

/// What a take of the send made at `sent_at_ms` on `phone` does with the phone's library ([`ForTake`]): the library
/// is read from the phone only while it is still to come in (once for a send).
pub(super) fn for_take(
    ctx: &Ctx,
    phone: &str,
    folder: &dyn Folder,
    listing: &[PhoneFile],
    sent_at_ms: i64,
) -> ForTake {
    let (kept, done) = {
        let library = ctx.library.lock().unwrap();
        let done = brought(&library).iter().any(|(p, at)| p == phone && *at == sent_at_ms);
        (kept(&library, phone), done)
    };
    let due = (!done && kept.is_some())
        .then(|| of_send(ctx, phone, folder, listing, sent_at_ms))
        .flatten();
    ForTake { kept, due }
}

/// Brings in `phone`'s library (`from`), but for what was left out of it (`left_out`), before the songs of `list` chosen
/// (`songs`) are taken, in one import: its likes made where they were not left out, its plays when they come in, and
/// every playlist not left out (one the phone gained since comes in). Likes and its own playlists only gain; a
/// playlist synced from a service, or a Stash Mix, is made what the phone has it as now (their songs change), here
/// already or not. With `only_here`, of each only the songs coming in now as files or files here already; a playlist
/// left with none of its songs is not made (but one made what the phone has, here already, is emptied). Then the send
/// is marked as brought in, so another try at it does not bring it in again. Trouble in the library here as
/// [`in_library`] says it.
pub(super) fn bring_in(
    ctx: &Ctx,
    phone: &str,
    list: &List,
    songs: &Choice,
    left_out: &LeftOut,
    from: &FromPhone,
) -> Result<(), String> {
    let mut incoming = chosen_parts(from, left_out);
    let mirrored: HashSet<String> = incoming
        .playlists
        .iter()
        .filter(|p| {
            from.from.contains_key(&p.external_id) || from.kind.get(&p.external_id) == Some(&"stash_mix")
        })
        .map(|p| p.external_id.clone())
        .collect();
    if left_out.only_here {
        let coming: Vec<Song> = list
            .songs
            .iter()
            .filter(|s| songs.includes(&s.file))
            .map(as_song)
            .collect();
        let downloaded = Downloaded::of(ctx.library, &Coming::of(&coming), songs_in(&incoming));
        let here: HashSet<String> = ctx
            .library
            .lock()
            .unwrap()
            .playlists()
            .map(|rows| rows.into_iter().filter_map(|p| p.external_id).collect())
            .unwrap_or_default();
        only_downloaded(&mut incoming, &downloaded, |id| mirrored.contains(id) && here.contains(id));
    }
    let summary = in_library(ctx.library, |library| library.import_mirroring(&incoming, &mirrored))?;
    in_library(ctx.library, |library| mark_brought(library, phone, list.sent_at_ms))?;
    (ctx.log)(&format!(
        "the phone's library came in: {} likes new here, {} playlists new and {} added to, {} plays new here, {} songs new here",
        summary.likes_added,
        summary.playlists_added,
        summary.playlists_merged,
        summary.plays_added,
        summary.songs_added
    ));
    (ctx.edited)();
    Ok(())
}

/// What comes in of `from` but for what was left out: its likes made where they were not left out, its plays when
/// they come in, and its playlists not left out.
fn chosen_parts(from: &FromPhone, left_out: &LeftOut) -> Incoming {
    let library = &from.library;
    let made = |at: usize| from.liked_on.get(at).map(Vec::as_slice).unwrap_or_default();
    Incoming {
        source: library.source,
        generator: library.generator.clone(),
        likes: library
            .likes
            .iter()
            .enumerate()
            .filter(|(at, _)| left_out.like(made(*at)))
            .map(|(_, like)| like.clone())
            .collect(),
        playlists: library
            .playlists
            .iter()
            .filter(|p| !left_out.playlists_left_out.contains(&p.external_id))
            .cloned()
            .collect(),
        plays: if left_out.plays { library.plays.clone() } else { Vec::new() },
        songs: Vec::new(),
        blocked: Vec::new(),
    }
}

/// Every song `incoming` names: in its likes, its playlists and its plays.
fn songs_in(incoming: &Incoming) -> impl Iterator<Item = &Song> {
    incoming
        .likes
        .iter()
        .map(|(song, _)| song)
        .chain(incoming.playlists.iter().flat_map(|p| &p.items))
        .chain(incoming.plays.iter().map(|(song, _)| song))
}

/// `incoming` with only its songs that are downloaded; a playlist left with none of its songs goes, unless it is to be
/// emptied (`emptied`, by its id).
fn only_downloaded(incoming: &mut Incoming, downloaded: &Downloaded, emptied: impl Fn(&str) -> bool) {
    incoming.likes.retain(|(song, _)| downloaded.is(song));
    incoming.plays.retain(|(song, _)| downloaded.is(song));
    incoming.playlists.retain_mut(|p| {
        let had = p.items.len();
        p.items.retain(|song| downloaded.is(song));
        had == 0 || !p.items.is_empty() || emptied(&p.external_id)
    });
}

/// The songs coming in as files with a send, to tell a song of the library among them as the import tells one song
/// from another: by its ISRC, its YouTube or Spotify id, or its words (unless both have an ISRC and they differ).
struct Coming {
    isrcs: HashSet<String>,
    ids: HashSet<(String, String)>,
    words: HashMap<String, Vec<Option<String>>>,
}

impl Coming {
    fn of(songs: &[Song]) -> Coming {
        let mut coming = Coming {
            isrcs: HashSet::new(),
            ids: HashSet::new(),
            words: HashMap::new(),
        };
        for song in songs {
            if let Some(isrc) = &song.isrc {
                coming.isrcs.insert(isrc.clone());
            }
            coming.ids.extend(song.refs.iter().filter(|(source, _)| known_id(source)).cloned());
            coming
                .words
                .entry(words_key(&song.title, &song.artist))
                .or_default()
                .push(song.isrc.clone());
        }
        coming
    }

    fn has(&self, song: &Song) -> bool {
        song.isrc.as_ref().is_some_and(|isrc| self.isrcs.contains(isrc))
            || song
                .refs
                .iter()
                .any(|(source, id)| known_id(source) && self.ids.contains(&(source.clone(), id.clone())))
            || self
                .words
                .get(&words_key(&song.title, &song.artist))
                .is_some_and(|isrcs| isrcs.iter().any(|theirs| isrcs_agree(theirs.as_deref(), song.isrc.as_deref())))
    }
}

/// The ids that name one recording wherever they are met: a YouTube video's, a Spotify track's.
fn known_id(source: &str) -> bool {
    matches!(source, "youtube" | "spotify")
}

/// Whether each song of a library is downloaded: among the songs coming in as files, or a file here already (as a take
/// would find it). By the key the import knows a song by, each asked once.
struct Downloaded(HashMap<String, bool>);

impl Downloaded {
    fn of<'a>(library: &Mutex<Library>, coming: &Coming, songs: impl Iterator<Item = &'a Song>) -> Downloaded {
        let mut known: HashMap<String, bool> = HashMap::new();
        let (mut keys, mut ask) = (Vec::new(), Vec::new());
        for song in songs {
            let key = key_of(song);
            if known.contains_key(&key) {
                continue;
            }
            let sent = coming.has(song);
            known.insert(key.clone(), sent);
            if !sent {
                keys.push(key);
                ask.push(song.clone());
            }
        }
        for (key, here) in keys.into_iter().zip(here_of(library, &ask)) {
            if here {
                known.insert(key, true);
            }
        }
        Downloaded(known)
    }

    fn is(&self, song: &Song) -> bool {
        self.0.get(&key_of(song)).copied().unwrap_or(false)
    }
}

/// The key one import knows a song by: its ISRC, else its words.
fn key_of(song: &Song) -> String {
    match &song.isrc {
        Some(isrc) => format!("isrc:{isrc}"),
        None => words_key(&song.title, &song.artist),
    }
}

/// For each of `songs`, whether it is a file here already ([`Library::songs_here`]), asked [`ASK_AT_ONCE`] at a time
/// with the library let go between. One the library could not say of is not.
fn here_of(library: &Mutex<Library>, songs: &[Song]) -> Vec<bool> {
    songs
        .chunks(ASK_AT_ONCE)
        .flat_map(|chunk| {
            library
                .lock()
                .unwrap()
                .songs_here(chunk)
                .unwrap_or_else(|_| vec![false; chunk.len()])
        })
        .collect()
}

/// Where a playlist comes in the first-run screen's order: the phone's own lists, then its Stash Mixes; then those of
/// each service ([`web_library::FROM`], in that order), its playlists, then albums, mixes and radios. Sorted stably, so
/// each kind keeps the file's order.
fn place(from: Option<&str>, kind: Option<&str>) -> (usize, usize) {
    let source = match (from, kind) {
        (_, Some("stash_mix")) => 1,
        (None, _) => 0,
        (Some(service), _) => {
            2 + web_library::FROM
                .iter()
                .position(|s| *s == service)
                .unwrap_or(web_library::FROM.len())
        }
    };
    let kind = match kind {
        None | Some("playlist") => 0,
        Some("album") => 1,
        Some("mix") => 2,
        Some("radio") => 3,
        _ => 4,
    };
    (source, kind)
}

/// What the window's first-run screen draws of the send offered and the library it came with
/// (`GET /api/phone/library`): the send's songs to come in (those not here already), the library's likes (with those
/// made in Stash and on each service: `by`, empty for an older file) and playlists (in [`place`]'s order, each with
/// where it came from and what kind it is) with how many of their songs are downloaded (sent as files now, or files
/// here already), its plays, the albums the send's songs are from, and what was left out before for this phone
/// (`choice`: null the first time).
pub(super) fn summary(library: &Mutex<Library>, offer: &Offer, from: &FromPhone) -> Value {
    // The send's songs asked again: songs may have come into the library since they were offered.
    let (mut songs, mut bytes, mut had) = (0, 0u64, 0);
    for (song, here) in offer.songs.iter().zip(here_of(library, &offer.asked)) {
        if here {
            had += 1;
        } else {
            songs += 1;
            bytes += song.bytes;
        }
    }
    let incoming = &from.library;
    let downloaded = Downloaded::of(
        library,
        &Coming::of(&offer.asked),
        incoming
            .likes
            .iter()
            .map(|(song, _)| song)
            .chain(incoming.playlists.iter().flat_map(|p| &p.items)),
    );
    let made = |at: usize| from.liked_on.get(at).map(Vec::as_slice).unwrap_or_default();
    let by: Vec<Value> = web_library::LIKED_ON
        .iter()
        .filter_map(|service| {
            let of: Vec<&Song> = incoming
                .likes
                .iter()
                .enumerate()
                .filter(|(at, _)| made(*at).contains(service))
                .map(|(_, (song, _))| song)
                .collect();
            (!of.is_empty()).then(|| {
                json!({
                    "service": service,
                    "songs": of.len(),
                    "downloaded": of.iter().filter(|s| downloaded.is(s)).count(),
                })
            })
        })
        .collect();
    let mut ordered: Vec<&Playlist> = incoming.playlists.iter().collect();
    ordered.sort_by_key(|p| {
        place(
            from.from.get(&p.external_id).copied(),
            from.kind.get(&p.external_id).copied(),
        )
    });
    let playlists: Vec<Value> = ordered
        .into_iter()
        .map(|p| {
            let mut covers: Vec<&str> = Vec::new();
            for url in p.items.iter().filter_map(|s| s.art_url.as_deref()) {
                if covers.len() == COVERS {
                    break;
                }
                if !covers.contains(&url) {
                    covers.push(url);
                }
            }
            json!({
                "id": p.external_id,
                "name": p.name,
                "from": from.from.get(&p.external_id),
                "kind": from.kind.get(&p.external_id),
                "songs": p.items.len(),
                "downloaded": p.items.iter().filter(|s| downloaded.is(s)).count(),
                "covers": covers,
            })
        })
        .collect();
    let choice = kept(&library.lock().unwrap(), &offer.phone);
    json!({
        "sent_at_ms": offer.sent_at_ms,
        "phone": offer.phone,
        "downloads": { "songs": songs, "bytes": bytes, "here": had },
        "likes": {
            "songs": incoming.likes.len(),
            "downloaded": incoming.likes.iter().filter(|(s, _)| downloaded.is(s)).count(),
            "by": by,
        },
        "plays": incoming.plays.len(),
        "playlists": playlists,
        "albums": albums(&offer.songs),
        "choice": choice,
    })
}

/// The albums the send's songs are from, most songs first (those with as many in the order they come), up to
/// [`ALBUMS`]: each its title, its artist (the album's, else the song's), a cover, and how many of the send's songs.
fn albums(songs: &[Offered]) -> Vec<Value> {
    let mut albums: Vec<(&str, &str, Option<&str>, usize)> = Vec::new();
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    for song in songs {
        let Some(title) = song.album.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
            continue;
        };
        let artist = song
            .album_artist
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .unwrap_or(&song.artist);
        let i = *at
            .entry((title.to_lowercase(), artist.to_lowercase()))
            .or_insert_with(|| {
                albums.push((title, artist, None, 0));
                albums.len() - 1
            });
        let album = &mut albums[i];
        album.3 += 1;
        if album.2.is_none() {
            album.2 = song.art_url.as_deref();
        }
    }
    albums.sort_by_key(|album| std::cmp::Reverse(album.3));
    albums
        .into_iter()
        .take(ALBUMS)
        .map(|(title, artist, art_url, songs)| {
            json!({ "title": title, "artist": artist, "art_url": art_url, "songs": songs })
        })
        .collect()
}
