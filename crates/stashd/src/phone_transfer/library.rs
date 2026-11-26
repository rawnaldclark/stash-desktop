//! The phone's library, sent with its songs ([`stash_core::phone::LIBRARY`], beside them in the transfer folder): its
//! likes, its playlists (its own, and those it synced from Spotify or YouTube Music) and its plays, as "Export for
//! Stash on the web" writes them, with the send they go with.
//!
//! It is read once for a send, when the send is offered (or taken without being offered), and summed up for the
//! window's first-run screen (`GET /api/phone/library`). The parts the listener chooses (`library` in
//! `POST /api/phone/take`) come in before the send's songs, in one import, once for the send. A song not sent as a file
//! comes in as a song that streams; a file sent lands on its song, as every file a take brings in does. What was
//! chosen is kept, and each later send with a library brings the same parts in by itself: a playlist not chosen never
//! comes in uninvited.
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
/// What was chosen to bring in from a phone's library ([`Parts`]): kept, and brought in from each later send's too.
pub(super) const CHOICE_SETTING: &str = "phone.library_choice";
/// The last send whose library came in (its `sent_at_ms`): another try at that send does not bring it in again.
pub(super) const BROUGHT_SETTING: &str = "phone.library_taken";
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

/// What the listener chose to bring in from a phone's library.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Parts {
    #[serde(default)]
    pub likes: bool,
    #[serde(default)]
    pub plays: bool,
    /// The playlists chosen, by their ids in the phone's file.
    #[serde(default)]
    pub playlists: Vec<String>,
    /// Of each part, only the songs that are files here (sent as files with the send, or files here already): nothing
    /// comes in that only streams.
    #[serde(default)]
    pub only_here: bool,
}

impl Parts {
    /// Whether anything is to come in.
    fn any(&self) -> bool {
        self.likes || self.plays || !self.playlists.is_empty()
    }
}

/// What `POST /api/phone/take` asks of the phone's library (its `library`), when it asks: likes, plays and only_here
/// true or false (false when left out), playlists by their ids.
pub(super) fn asked(body: &Value) -> Result<Option<Parts>, &'static str> {
    let asked = &body["library"];
    if asked.is_null() {
        return Ok(None);
    }
    let asked = asked
        .as_object()
        .ok_or("library says what to bring in: likes, plays, playlists and only_here")?;
    let yes = |name: &str| match asked.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(v) => v.as_bool().ok_or("likes, plays and only_here are true or false"),
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
    Ok(Some(Parts {
        likes: yes("likes")?,
        plays: yes("plays")?,
        playlists,
        only_here: yes("only_here")?,
    }))
}

/// Whether `parts` can be brought in from the library `offer` came with: it came with one, and every playlist chosen is
/// in it (one that is not was gone from the phone since the window read the library).
pub(super) fn can_bring(offer: &Offer, parts: &Parts) -> Result<(), &'static str> {
    let from = offer
        .library
        .as_ref()
        .ok_or("the phone sent no library with these songs")?;
    let ids: HashSet<&str> = from.library.playlists.iter().map(|p| p.external_id.as_str()).collect();
    if parts.playlists.iter().any(|id| !ids.contains(id.as_str())) {
        return Err("the playlists on the phone changed: look them over and take them again");
    }
    Ok(())
}

/// `parts` kept: brought in from the send chosen from now, and from each later send that comes with a library.
pub(super) fn keep(library: &Mutex<Library>, parts: &Parts) {
    let _ = library
        .lock()
        .unwrap()
        .set_setting(CHOICE_SETTING, &json!(parts).to_string());
}

/// What was chosen to bring in, as kept.
fn kept(library: &Library) -> Option<Parts> {
    let text = library.setting(CHOICE_SETTING).ok().flatten()?;
    serde_json::from_str(&text).ok()
}

/// The parts kept to bring in, and the library that came with the send made at `sent_at_ms`, while they are still to
/// come in from that send (once for a send): the library is read from the phone only then.
pub(super) fn due(
    ctx: &Ctx,
    phone: &str,
    folder: &dyn Folder,
    listing: &[PhoneFile],
    sent_at_ms: i64,
) -> Option<(Parts, Arc<FromPhone>)> {
    let (parts, brought) = {
        let library = ctx.library.lock().unwrap();
        (kept(&library), library.setting(BROUGHT_SETTING).ok().flatten())
    };
    if brought.as_deref() == Some(sent_at_ms.to_string().as_str()) {
        return None;
    }
    let parts = parts.filter(Parts::any)?;
    Some((parts, of_send(ctx, phone, folder, listing, sent_at_ms)?))
}

/// Brings in the `parts` chosen of the phone's library (`from`) before the songs of `list` chosen (`songs`) are taken,
/// in one import: its likes and plays when chosen, and the playlists chosen that it holds. With `only_here`, of each
/// only the songs coming in now as files or files here already; a playlist left with none of its songs is not made.
/// Then the send is marked as brought in, so another try at it does not bring it in again. Trouble in the library here
/// as [`in_library`] says it.
pub(super) fn bring_in(
    ctx: &Ctx,
    list: &List,
    songs: &Choice,
    parts: &Parts,
    from: &FromPhone,
) -> Result<(), String> {
    let mut incoming = chosen_parts(&from.library, parts);
    if parts.only_here {
        let coming: Vec<Song> = list
            .songs
            .iter()
            .filter(|s| songs.includes(&s.file))
            .map(as_song)
            .collect();
        let downloaded = Downloaded::of(ctx.library, &Coming::of(&coming), songs_in(&incoming));
        only_downloaded(&mut incoming, &downloaded);
    }
    let summary = in_library(ctx.library, |library| library.import(&incoming))?;
    in_library(ctx.library, |library| {
        library.set_setting(BROUGHT_SETTING, &list.sent_at_ms.to_string())
    })?;
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

/// The parts of `library` chosen: its likes and plays when chosen, and the playlists chosen that it holds.
fn chosen_parts(library: &Incoming, parts: &Parts) -> Incoming {
    let playlists: HashSet<&str> = parts.playlists.iter().map(String::as_str).collect();
    Incoming {
        source: library.source,
        generator: library.generator.clone(),
        likes: if parts.likes { library.likes.clone() } else { Vec::new() },
        playlists: library
            .playlists
            .iter()
            .filter(|p| playlists.contains(p.external_id.as_str()))
            .cloned()
            .collect(),
        plays: if parts.plays { library.plays.clone() } else { Vec::new() },
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

/// `incoming` with only its songs that are downloaded; a playlist left with none of its songs goes.
fn only_downloaded(incoming: &mut Incoming, downloaded: &Downloaded) {
    incoming.likes.retain(|(song, _)| downloaded.is(song));
    incoming.plays.retain(|(song, _)| downloaded.is(song));
    incoming.playlists.retain_mut(|p| {
        let had = p.items.len();
        p.items.retain(|song| downloaded.is(song));
        had == 0 || !p.items.is_empty()
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

/// What the window's first-run screen draws of the send offered and the library it came with
/// (`GET /api/phone/library`): the send's songs to come in (those not here already), the library's likes and
/// playlists with how many of their songs are downloaded (sent as files now, or files here already), its plays, the
/// albums the send's songs are from, and what was chosen before (`choice`: null the first time).
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
    // Its own playlists first, then those it synced, each in the file's order.
    let (own, synced): (Vec<&Playlist>, Vec<&Playlist>) = incoming
        .playlists
        .iter()
        .partition(|p| !from.from.contains_key(&p.external_id));
    let playlists: Vec<Value> = own
        .into_iter()
        .chain(synced)
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
                "songs": p.items.len(),
                "downloaded": p.items.iter().filter(|s| downloaded.is(s)).count(),
                "covers": covers,
            })
        })
        .collect();
    let choice = kept(&library.lock().unwrap());
    json!({
        "sent_at_ms": offer.sent_at_ms,
        "phone": offer.phone,
        "downloads": { "songs": songs, "bytes": bytes, "here": had },
        "likes": {
            "songs": incoming.likes.len(),
            "downloaded": incoming.likes.iter().filter(|(s, _)| downloaded.is(s)).count(),
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
