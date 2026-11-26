//! Songs from Stash for Android over USB. "Send to computer" on the phone leaves its downloads in
//! `Download/Stash transfer/` with `stash-transfer.json` saying which song each file is; with the phone plugged in
//! ("File transfer"), the listener chooses which of them to take (each said to be here already or not, from what the
//! list says, before any is read), and this takes them into the library as downloaded songs, each deleted from the
//! phone once taken. Then the folder is deleted from the phone, or, with songs not chosen left in it, just its list:
//! the phone then clears its copies. Settings can have every send taken whole as it comes, without asking.
//!
//! Each song is found in the library as an import finds it (its ISRC, its words), or added; its file goes into the
//! downloads folder where Stash would have saved it, and its download is recorded as saved. A song the library has as a
//! file already is left as it is, and not read from the phone. A song liked on the phone is liked here too.
//!
//! A phone's File transfer can stop mid-song and not start again by itself: a song that sends nothing for a while has
//! its read called off, and the phone is opened again and the song taken from the start (see
//! [`stash_core::phone::STALL`]). What each transfer did goes into `phone.log` beside the library.
//!
//! - `GET /api/phone`: what is happening: nothing, a send waiting to be chosen from, songs being taken from a phone
//!   (how many of how many), or what the last transfer did; and the phones plugged in now (with "File transfer"
//!   chosen), each with whether it has songs sent.
//! - `GET /api/phone/songs`: the send waiting to be chosen from: each song as the phone describes it (its format and
//!   quality, its tags, the playlists it is in), its size, and whether it is here already.
//! - `POST /api/phone/take` `{sent_at_ms, files}` (or `all: true`, or `files: [], none: true`): takes those songs.
//! - `GET`/`POST /api/settings/phone` `{take_all}`: every send taken whole as it comes, or offered (the default).
//! - `POST /api/phone/look`: looks at the phones now, and tries again a send that had songs not taken; answers the
//!   look's number, which `answered` in `GET /api/phone` reaches once it is done.
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use stash_core::Library;
use stash_core::downloads::{self, Song};
use stash_core::import;
use stash_core::lists::SavedDownload;
use stash_core::phone::{PhoneFile, STALL, STALLED, Watch};
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, now_ms, refuse, reply_json};

/// How often the phones are looked at, and how often again a phone with no transfer on it.
const LOOK_EVERY: Duration = Duration::from_secs(2);
const PHONE_AGAIN: Duration = Duration::from_secs(3);
/// The largest list read.
const LIST_CAP: u64 = 16 * 1024 * 1024;
/// A transfer taken whole, but its folder could not be deleted from the phone: it is not taken twice.
const TAKEN_SETTING: &str = "phone.taken";
/// A transfer is tried this many times in a row with nothing coming in before it is left (while Stash runs; plugging
/// the phone in again after a restart, or Settings' "Look for my phone", tries again). A try that brings songs in
/// starts the count again, however often the phone has to be plugged back in.
const TRIES: u32 = 3;
/// How often one song may stop sending before it is left on the phone (for another send) and the rest go on.
const STALLS_PER_SONG: u32 = 2;
/// The waits before each try at opening a phone again after a read was called off: it can take a moment to answer.
#[cfg(not(test))]
const AGAIN_AFTER: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(6),
];
#[cfg(test)]
const AGAIN_AFTER: [Duration; 3] = [Duration::from_millis(1); 3];

/// One song in the phone's list.
#[derive(Debug, Clone, Deserialize)]
struct Sent {
    file: String,
    title: String,
    artist: String,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    isrc: Option<String>,
    #[serde(default)]
    youtube_id: Option<String>,
    #[serde(default)]
    spotify_uri: Option<String>,
    #[serde(default)]
    art_url: Option<String>,
    #[serde(default)]
    liked: bool,
    // What the phone says of its file and where the song is, for choosing (none is needed to take it).
    #[serde(default)]
    album_artist: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    bits: Option<u32>,
    #[serde(default)]
    sample_rate: Option<u32>,
    #[serde(default)]
    playlists: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct List {
    stash_transfer: u32,
    sent_at_ms: i64,
    songs: Vec<Sent>,
}

/// What the window is told.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Now {
    /// "idle" (no phone), "connected" (a phone with nothing sent), "preparing" (the phone is copying its songs out:
    /// `ready` so far), "ready" (a send waits to be chosen from: `total` songs, `here` of them here already),
    /// "taking", "stalled" (the phone stopped sending mid-song: it is being asked again), "done", or "stopped"
    /// (unplugged while taking, or not answering again after stalling).
    state: &'static str,
    phone: Option<String>,
    ready: usize,
    done: usize,
    total: usize,
    bytes_done: u64,
    bytes_total: u64,
    /// The song being taken ("Title · Artist").
    song: Option<String>,
    /// Songs that are files here now, songs that were files here already, songs whose file is not kept here (WebM,
    /// which cannot be tagged: the song is in the library, to stream or save from a source), songs not taken.
    taken: usize,
    had: usize,
    not_kept: usize,
    failed: usize,
    /// Of the songs offered, how many are here already.
    here: usize,
    /// Songs left on the phone because they were not chosen.
    left: usize,
    /// The phone's folder was deleted.
    cleared: bool,
    at_ms: i64,
    error: Option<String>,
}

/// A send's songs still on the phone, offered to choose from (`GET /api/phone/songs`).
#[derive(Debug, Clone, Serialize)]
struct Offer {
    phone: String,
    sent_at_ms: i64,
    songs: Vec<Offered>,
    /// Each song as taking it would find it in the library, for asking again whether it is here.
    #[serde(skip)]
    asked: Vec<import::Song>,
}

/// One song offered: what the phone says of it, its size on the phone, and whether it is here already.
#[derive(Debug, Clone, Serialize)]
struct Offered {
    file: String,
    title: String,
    artist: String,
    album: Option<String>,
    album_artist: Option<String>,
    format: Option<String>,
    bits: Option<u32>,
    sample_rate: Option<u32>,
    duration_ms: Option<u64>,
    art_url: Option<String>,
    liked: bool,
    playlists: Vec<String>,
    bytes: u64,
    here: bool,
}

/// Which songs of a send to take: every one, or those chosen (by their file names; none, to take nothing and let the
/// phone clear its copies).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Choice {
    All,
    These(HashSet<String>),
}

impl Choice {
    fn includes(&self, file: &str) -> bool {
        match self {
            Choice::All => true,
            Choice::These(files) => files.contains(file),
        }
    }

    /// As kept in the library's settings, with the send it is for.
    fn kept(&self, sent_at_ms: i64) -> String {
        match self {
            Choice::All => json!({ "sent_at_ms": sent_at_ms, "all": true }),
            Choice::These(files) => {
                let mut files: Vec<&String> = files.iter().collect();
                files.sort();
                json!({ "sent_at_ms": sent_at_ms, "files": files })
            }
        }
        .to_string()
    }

    fn from_kept(text: &str) -> Option<(i64, Choice)> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let at = v["sent_at_ms"].as_i64()?;
        if v["all"].as_bool() == Some(true) {
            return Some((at, Choice::All));
        }
        let files: HashSet<String> = v["files"]
            .as_array()?
            .iter()
            .filter_map(|f| f.as_str().map(String::from))
            .collect();
        Some((at, Choice::These(files)))
    }
}

/// The send last chosen from, and what was chosen: it outlives a restart, so a transfer cut short carries on with the
/// same songs.
const CHOSEN_SETTING: &str = "phone.chosen";
/// "1": every send is taken whole, as it comes, without asking.
const TAKE_ALL_SETTING: &str = "phone.take_all";
/// A folder of songs with no list that has not changed for this long is not a phone copying: it is copies left there
/// (songs not chosen, from a phone that has not cleared them yet).
const COPYING_QUIET: Duration = Duration::from_secs(30);

/// What the window is told, written by the transfer as it goes (and, for a song stopping, by the thread watching
/// its read).
struct Board {
    now: Mutex<Option<Now>>,
    /// The send waiting to be chosen from, until it is.
    offer: Mutex<Option<Offer>>,
    /// What was chosen from each send (by its `sent_at_ms`), until it is taken.
    chosen: Mutex<Option<HashMap<i64, Choice>>>,
    #[cfg(test)]
    states: Mutex<Vec<&'static str>>,
}

impl Board {
    const fn new() -> Self {
        Board {
            now: Mutex::new(None),
            offer: Mutex::new(None),
            chosen: Mutex::new(None),
            #[cfg(test)]
            states: Mutex::new(Vec::new()),
        }
    }

    /// What was chosen from the send `sent_at_ms`: while Stash runs, else what the library kept from before.
    fn choice(&self, library: &Mutex<Library>, sent_at_ms: i64) -> Option<Choice> {
        if let Some(choice) = self.chosen.lock().unwrap().as_ref().and_then(|c| c.get(&sent_at_ms)) {
            return Some(choice.clone());
        }
        let kept = library.lock().unwrap().setting(CHOSEN_SETTING).ok().flatten()?;
        let (at, choice) = Choice::from_kept(&kept)?;
        (at == sent_at_ms).then_some(choice)
    }

    fn choose(&self, library: &Mutex<Library>, sent_at_ms: i64, choice: Choice) {
        let _ = library
            .lock()
            .unwrap()
            .set_setting(CHOSEN_SETTING, &choice.kept(sent_at_ms));
        self.chosen
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(sent_at_ms, choice);
        *self.offer.lock().unwrap() = None;
    }

    /// The send `sent_at_ms` is taken: its choice is let go.
    fn chosen_taken(&self, library: &Mutex<Library>, sent_at_ms: i64) {
        if let Some(chosen) = self.chosen.lock().unwrap().as_mut() {
            chosen.remove(&sent_at_ms);
        }
        let mut library = library.lock().unwrap();
        let kept = library.setting(CHOSEN_SETTING).ok().flatten();
        if kept.as_deref().and_then(Choice::from_kept).is_some_and(|(at, _)| at == sent_at_ms) {
            let _ = library.set_setting(CHOSEN_SETTING, "");
        }
    }

    fn set(&self, now: Now) {
        #[cfg(test)]
        self.states.lock().unwrap().push(now.state);
        *self.now.lock().unwrap() = Some(now);
    }

    /// How much of the transfer has come, as a song comes in.
    fn bytes_done(&self, bytes: u64) {
        if let Some(now) = self.now.lock().unwrap().as_mut() {
            now.bytes_done = bytes;
        }
    }

    /// The song being taken has stopped coming: its read is being called off and the phone asked again.
    fn stalled(&self) {
        #[cfg(test)]
        self.states.lock().unwrap().push("stalled");
        if let Some(now) = self.now.lock().unwrap().as_mut() {
            now.state = "stalled";
            now.at_ms = now_ms();
        }
    }
}

static BOARD: Board = Board::new();
/// The phones plugged in at the last look: their names, and whether each had songs sent (None: not looked inside yet).
static SEEN: Mutex<Vec<(String, Option<bool>)>> = Mutex::new(Vec::new());
/// Looks asked for (Settings' "Look for my phone"), and the last one answered: the window waits for its number.
static ASKED: AtomicU64 = AtomicU64::new(0);
static ANSWERED: AtomicU64 = AtomicU64::new(0);

/// What taking songs needs of a phone's transfer folder: the phone's own, or one in memory in the tests.
trait Folder {
    fn files(&self) -> Vec<PhoneFile>;
    fn bytes(&self, file: &PhoneFile, cap: u64) -> Result<Vec<u8>, String>;
    /// Copies `file` to `to`; fails with [`STALLED`] when the phone stopped sending.
    fn copy(&self, file: &PhoneFile, to: &Path, watch: &Watch) -> Result<u64, String>;
    fn remove_file(&self, file: &PhoneFile) -> Result<(), String>;
    fn remove(&self) -> Result<(), String>;
}

#[cfg(windows)]
impl Folder for stash_core::phone::Transfer {
    fn files(&self) -> Vec<PhoneFile> {
        self.files.clone()
    }
    fn bytes(&self, file: &PhoneFile, cap: u64) -> Result<Vec<u8>, String> {
        stash_core::phone::Transfer::bytes(self, file, cap)
    }
    fn copy(&self, file: &PhoneFile, to: &Path, watch: &Watch) -> Result<u64, String> {
        stash_core::phone::Transfer::copy(self, file, to, watch)
    }
    fn remove_file(&self, file: &PhoneFile) -> Result<(), String> {
        stash_core::phone::Transfer::remove_file(self, file)
    }
    fn remove(&self) -> Result<(), String> {
        stash_core::phone::Transfer::remove(self)
    }
}

/// The phone a transfer is taken from: its name, opened again after a read was called off, and whether Windows still
/// has it.
trait Plugged {
    fn name(&self) -> &str;
    /// Its transfer folder opened again, when it answers and the list is still there.
    fn open_again(&self) -> Option<Box<dyn Folder>>;
    fn still_there(&self) -> bool;
}

#[cfg(windows)]
struct Usb<'a>(&'a stash_core::phone::Phone);

#[cfg(windows)]
impl Plugged for Usb<'_> {
    fn name(&self) -> &str {
        &self.0.name
    }
    fn open_again(&self) -> Option<Box<dyn Folder>> {
        match stash_core::phone::open_transfer(&self.0.id) {
            Ok(Some(t)) if t.files.iter().any(|f| f.name == stash_core::phone::LIST) => {
                Some(Box::new(t))
            }
            _ => None,
        }
    }
    fn still_there(&self) -> bool {
        stash_core::phone::phones()
            .map(|p| p.iter().any(|p| p.id == self.0.id))
            .unwrap_or(true)
    }
}

/// What taking songs needs of the core: the library, the board the window reads, telling the windows the library
/// changed, and the log.
struct Ctx<'a> {
    library: &'a Mutex<Library>,
    board: &'a Board,
    edited: &'a dyn Fn(),
    log: &'a dyn Fn(&str),
}

/// A line in `phone.log` beside the library (the last megabyte or so kept, the one before in `phone.old.log`): what
/// each transfer did, and why one stopped or stalled.
fn log(state: &State, line: &str) {
    eprintln!("phone: {line}");
    let Some(dir) = state.data_dir() else {
        return;
    };
    let path = dir.join("phone.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&path, dir.join("phone.old.log"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{} {line}", now_ms());
    }
}

/// The song as an import knows it: its ids checked as the Android import checks them.
fn as_song(s: &Sent) -> import::Song {
    let made_of = |id: &str, length: usize, also: &[u8]| {
        id.len() == length
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || also.contains(&b))
    };
    let mut refs = Vec::new();
    if let Some(y) = s.youtube_id.as_deref().filter(|id| made_of(id, 11, b"_-")) {
        refs.push(("youtube".to_string(), y.to_string()));
    }
    let spotify = s.spotify_uri.as_deref().and_then(|uri| {
        let uri = uri.trim();
        let id = uri
            .strip_prefix("spotify:track:")
            .or_else(|| uri.strip_prefix("https://open.spotify.com/track/"))?;
        Some(id.split(['?', '/']).next().unwrap_or_default().to_string())
    });
    if let Some(sp) = spotify.filter(|id| made_of(id, 22, b"")) {
        refs.push(("spotify".to_string(), sp));
    }
    let short = |t: &str, n: usize| t.trim().chars().take(n).collect::<String>();
    import::Song {
        title: short(&s.title, 300),
        artist: short(&s.artist, 300),
        album: s
            .album
            .as_deref()
            .map(|a| short(a, 300))
            .filter(|a| !a.is_empty()),
        duration_ms: s.duration_ms.filter(|d| *d > 0),
        isrc: s
            .isrc
            .as_deref()
            .map(str::trim)
            .filter(|i| i.len() == 12)
            .map(str::to_uppercase),
        explicit: None,
        refs,
        art_url: s
            .art_url
            .clone()
            .filter(|u| u.starts_with("https://") && u.len() <= 2_048),
        added_at_ms: None,
    }
}

/// What became of one song.
#[derive(Debug, PartialEq, Eq)]
enum Took {
    /// Its file is a download here now.
    Taken,
    /// The library had it as a file already.
    Had,
    /// Its file is WebM, which Stash does not keep (as its own downloads do not): the song is in the library only.
    NotKept,
}

/// A WebM (Matroska) file starts so: YouTube's Opus, which Android keeps as it came.
const WEBM: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

/// Takes one song: its row, its file put where Stash saves songs, its download recorded. A song the library has as a
/// file already is not read from the phone; any other is read once, whole, and what kind of file it is is told from
/// the copy here.
fn take_one(
    ctx: &Ctx,
    folder: &dyn Folder,
    file: &PhoneFile,
    sent: &Sent,
    watch: &Watch,
) -> Result<Took, String> {
    let song = as_song(sent);
    let (id, wanted, root) = {
        let mut library = ctx.library.lock().unwrap();
        let id = *library
            .adopt_songs(std::slice::from_ref(&song))
            .map_err(|e| e.to_string())?
            .first()
            .ok_or("the song was not kept")?;
        if sent.liked {
            let _ = library.set_liked(id, true, now_ms());
        }
        let wanted = library.wants_file(id).map_err(|e| e.to_string())?;
        (id, wanted, crate::saving::folder(&library))
    };
    if !wanted {
        return Ok(Took::Had);
    }
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let part = root.join(format!(".stash-phone-{id}.part"));
    // Nothing is left at `part` when the copy fails.
    folder.copy(file, &part, watch)?;
    let mut head = [0u8; 64];
    let n = std::fs::File::open(&part)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .unwrap_or(0);
    if head[..n].starts_with(&WEBM) {
        let _ = std::fs::remove_file(&part);
        return Ok(Took::NotKept);
    }
    let kind = downloads::kind_of(&head[..n]).unwrap_or("audio");
    let number = {
        let mut library = ctx.library.lock().unwrap();
        if !library
            .download_arriving(id, now_ms())
            .map_err(|e| e.to_string())?
        {
            drop(library);
            let _ = std::fs::remove_file(&part);
            return Ok(Took::Had);
        }
        library.track_number(id).ok().flatten()
    };
    let failed = |why: String| {
        let _ = std::fs::remove_file(&part);
        let _ = ctx.library.lock().unwrap().download_failed(id, &why);
        why
    };
    let target = downloads::path_for(
        &root,
        &Song {
            title: song.title.clone(),
            artist: song.artist.clone(),
            album: song.album.clone(),
            track_no: number,
            isrc: song.isrc.clone(),
        },
        kind,
    );
    let placed = target
        .parent()
        .map(std::fs::create_dir_all)
        .unwrap_or(Ok(()))
        .and_then(|_| std::fs::rename(&part, &target));
    if let Err(e) = placed {
        return Err(failed(e.to_string()));
    }
    let read = downloads::read(&target).map_err(|e| {
        crate::saving::remove_saved(&target, &root);
        failed(format!("{e:?}"))
    })?;
    let saved = SavedDownload {
        addon: "phone".into(),
        tier: None,
        at_ms: now_ms(),
        fetched: false,
        cover_inside: false,
    };
    let kept = ctx
        .library
        .lock()
        .unwrap()
        .download_saved(id, &read, &saved)
        .map_err(|e| e.to_string());
    match kept {
        Ok(true) => Ok(Took::Taken),
        Ok(false) => {
            crate::saving::remove_saved(&target, &root);
            Ok(Took::Had)
        }
        Err(e) => {
            crate::saving::remove_saved(&target, &root);
            Err(failed(e))
        }
    }
}

/// A failed song counts as done, for the count shown.
fn continue_failed(ctx: &Ctx, now: &mut Now, file: &PhoneFile) {
    now.done += 1;
    now.bytes_done += file.size;
    ctx.board.set(now.clone());
}

/// What Windows says when the phone is no longer there to answer: unreachable, not connected, not ready, the device
/// closed, a general failure of the link.
fn lost(why: &str) -> bool {
    [
        "0x80070141",
        "0x8007048f",
        "0x80070015",
        "0x802a0002",
        "0x8007001f",
        "0x80070079",
    ]
    .iter()
    .any(|code| why.contains(code))
}

/// This many songs failing one after another: the phone is taken as gone.
const LOST_AFTER: usize = 3;

/// Takes a phone's transfer, if it has a whole one (its list is there) not taken before, and while it has not been
/// tried [`TRIES`] times in a row with nothing coming in.
fn take(
    ctx: &Ctx,
    phone: &dyn Plugged,
    folder: Box<dyn Folder>,
    tried: &mut HashMap<i64, u32>,
    stop: &AtomicBool,
) {
    let Some(list_file) = folder
        .files()
        .into_iter()
        .find(|f| f.name == stash_core::phone::LIST)
    else {
        return;
    };
    let bytes = match folder.bytes(&list_file, LIST_CAP) {
        Ok(bytes) => bytes,
        Err(why) => {
            // Listed but not answering (its USB connection dropped): said once, as a transfer cut short. A send of this
            // phone's waiting to be chosen from is left as it is: nothing was being taken, and the same send is offered
            // again (with what the window chose from it) once the phone answers.
            let waiting = ctx
                .board
                .offer
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|o| o.phone == phone.name());
            if lost(&why) && !waiting {
                let mut now = ctx.board.now.lock().unwrap();
                let said = now.as_ref().is_some_and(|n| {
                    n.state == "stopped" && n.phone.as_deref() == Some(phone.name())
                });
                if !said {
                    *now = Some(Now {
                        state: "stopped",
                        phone: Some(phone.name().to_string()),
                        at_ms: now_ms(),
                        ..Default::default()
                    });
                }
            }
            return;
        }
    };
    let Ok(list) = serde_json::from_slice::<List>(&bytes) else {
        return;
    };
    if list.stash_transfer != 1 {
        return;
    }
    let taken_before = ctx
        .library
        .lock()
        .unwrap()
        .setting(TAKEN_SETTING)
        .ok()
        .flatten();
    if taken_before.as_deref() == Some(list.sent_at_ms.to_string().as_str()) {
        return;
    }
    // Nothing chosen from this send yet: it is offered to choose from, unless every send is to be taken whole.
    let Some(choice) = ctx
        .board
        .choice(ctx.library, list.sent_at_ms)
        .or_else(|| take_all(ctx.library).then_some(Choice::All))
    else {
        offer(ctx, phone, folder.as_ref(), &list);
        return;
    };
    let tries = tried.get(&list.sent_at_ms).copied().unwrap_or(0);
    if tries >= TRIES {
        return;
    }
    tried.insert(list.sent_at_ms, tries + 1);
    let now = take_songs(ctx, phone, folder, &list, &choice, stop);
    if now.taken + now.had + now.not_kept > 0 {
        tried.remove(&list.sent_at_ms);
    }
}

/// What the window is told while a send waits to be chosen from.
fn ready_now(offer: &Offer, at_ms: i64) -> Now {
    Now {
        state: "ready",
        phone: Some(offer.phone.clone()),
        total: offer.songs.len(),
        here: offer.songs.iter().filter(|s| s.here).count(),
        bytes_total: offer.songs.iter().map(|s| s.bytes).sum(),
        at_ms,
        ..Default::default()
    }
}

/// Offers `list`'s songs still on the phone to choose from, each said to be here already or not (found as taking it
/// would find it, changing nothing). Worked out once for a send, and again when what is on the phone changes. One
/// send is offered at a time: another phone's waits until that one is chosen from or unplugged.
fn offer(ctx: &Ctx, phone: &dyn Plugged, folder: &dyn Folder, list: &List) {
    let sizes: HashMap<String, u64> = folder.files().into_iter().map(|f| (f.name, f.size)).collect();
    let on_phone: Vec<&Sent> = list
        .songs
        .iter()
        .filter(|s| sizes.contains_key(&s.file))
        .collect();
    {
        let offered = ctx.board.offer.lock().unwrap();
        if let Some(o) = offered.as_ref() {
            if o.phone != phone.name() {
                return;
            }
            if o.sent_at_ms == list.sent_at_ms && o.songs.len() == on_phone.len() {
                // Offered already; said again when something else was said over it: on this phone, or what another
                // phone did (its transfer done or stopped), but not over another phone's send being chosen or taken.
                let mut now = ctx.board.now.lock().unwrap();
                let shown = now.as_ref().is_some_and(|n| n.state == "ready");
                if !shown && !busy_elsewhere(now.as_ref(), phone.name()) {
                    let at_ms = now.as_ref().map_or_else(now_ms, |n| n.at_ms);
                    *now = Some(ready_now(o, at_ms));
                }
                return;
            }
        }
    }
    let asked: Vec<import::Song> = on_phone.iter().map(|s| as_song(s)).collect();
    let here = ctx
        .library
        .lock()
        .unwrap()
        .songs_here(&asked)
        .unwrap_or_else(|_| vec![false; asked.len()]);
    let songs: Vec<Offered> = on_phone
        .iter()
        .zip(here)
        .map(|(s, here)| Offered {
            file: s.file.clone(),
            title: s.title.trim().to_string(),
            artist: s.artist.trim().to_string(),
            album: s.album.clone().filter(|a| !a.trim().is_empty()),
            album_artist: s.album_artist.clone().filter(|a| !a.trim().is_empty()),
            format: s.format.clone(),
            bits: s.bits,
            sample_rate: s.sample_rate,
            duration_ms: s.duration_ms,
            art_url: s.art_url.clone().filter(|u| u.starts_with("https://")),
            liked: s.liked,
            playlists: s.playlists.clone(),
            bytes: sizes[&s.file],
            here,
        })
        .collect();
    let offer = Offer {
        phone: phone.name().to_string(),
        sent_at_ms: list.sent_at_ms,
        songs,
        asked,
    };
    // The same send offered again (fewer of its songs on the phone) keeps its time, so a card closed on it stays
    // closed.
    let at_ms = ctx
        .board
        .now
        .lock()
        .unwrap()
        .as_ref()
        .filter(|n| n.state == "ready" && n.phone.as_deref() == Some(phone.name()))
        .map_or_else(now_ms, |n| n.at_ms);
    let now = ready_now(&offer, at_ms);
    (ctx.log)(&format!(
        "offering {} songs, {} MB ({} here already)",
        now.total,
        now.bytes_total / 1_000_000,
        now.here
    ));
    *ctx.board.offer.lock().unwrap() = Some(offer);
    // Not said over another phone's send being chosen from or taken: it is said once that one is done (above).
    if !busy_elsewhere(ctx.board.now.lock().unwrap().as_ref(), phone.name()) {
        ctx.board.set(now);
    }
}

/// Whether `now` is another phone's send being chosen from or taken, which another phone's news is not said over.
fn busy_elsewhere(now: Option<&Now>, phone: &str) -> bool {
    now.is_some_and(|n| n.phone.as_deref() != Some(phone) && matches!(n.state, "ready" | "taking" | "stalled"))
}

/// The phone's transfer folder opened again after a read was called off, tried a few times a little apart. None
/// when it does not answer (it is then to be unplugged, or its File transfer turned off and on).
fn open_again(phone: &dyn Plugged, stop: &AtomicBool) -> Option<Box<dyn Folder>> {
    for wait in AGAIN_AFTER {
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100).min(wait));
        }
        if let Some(folder) = phone.open_again() {
            return Some(folder);
        }
    }
    None
}

/// Takes the songs of `list` chosen and still on the phone, one at a time, each deleted from the phone once taken. A
/// song that stops coming has its read called off: the phone is opened again and the song taken again from its start
/// (a song that stops [`STALLS_PER_SONG`] times is left, and the rest go on). With every chosen song taken, the copies
/// of songs not chosen that are here already go too ([`drop_here_already`]), then the phone's folder, or, when songs not
/// chosen are left in it, just its list: the phone then knows the computer is done. What the transfer came to.
fn take_songs(
    ctx: &Ctx,
    phone: &dyn Plugged,
    folder: Box<dyn Folder>,
    list: &List,
    choice: &Choice,
    stop: &AtomicBool,
) -> Now {
    let index = |folder: &dyn Folder| -> HashMap<String, PhoneFile> {
        folder
            .files()
            .into_iter()
            .map(|f| (f.name.clone(), f))
            .collect()
    };
    let mut folder = Some(folder);
    let mut files = index(folder.as_deref().unwrap());
    // Chosen songs whose file is gone were taken by an earlier try (each file is deleted from the phone once taken).
    let wanted: Vec<&Sent> = list.songs.iter().filter(|s| choice.includes(&s.file)).collect();
    let total = wanted.len();
    let here: Vec<&Sent> = wanted
        .iter()
        .copied()
        .filter(|s| files.contains_key(&s.file))
        .collect();
    let mut now = Now {
        state: "taking",
        phone: Some(phone.name().to_string()),
        total,
        done: total - here.len(),
        bytes_total: here.iter().map(|s| files[&s.file].size).sum(),
        at_ms: now_ms(),
        ..Default::default()
    };
    ctx.board.set(now.clone());
    (ctx.log)(&format!(
        "taking {} songs, {} MB ({} taken before)",
        here.len(),
        now.bytes_total / 1_000_000,
        now.done
    ));
    let started = Instant::now();
    let (mut i, mut in_a_row, mut stalls) = (0, 0, 0);
    while i < here.len() {
        if stop.load(Ordering::Relaxed) {
            return now;
        }
        let sent = here[i];
        let Some(file) = files.get(&sent.file).cloned() else {
            // Gone from the phone since it was opened again: taken before.
            now.done += 1;
            i += 1;
            continue;
        };
        now.state = "taking";
        now.song = Some(format!("{} · {}", sent.title.trim(), sent.artist.trim()));
        ctx.board.set(now.clone());
        let (base, size, board) = (now.bytes_done, file.size, ctx.board);
        let progress = |n: u64| board.bytes_done(base + n.min(size));
        let stalled = || board.stalled();
        let watch = Watch {
            stall_after: STALL,
            progress: &progress,
            stalled: &stalled,
        };
        let current = folder.as_deref().expect("a folder while taking");
        let at = Instant::now();
        match take_one(ctx, current, &file, sent, &watch) {
            Ok(took) => {
                match took {
                    Took::Taken => now.taken += 1,
                    Took::Had => now.had += 1,
                    Took::NotKept => now.not_kept += 1,
                }
                // Off the phone: what is left there is what is still to come (the phone counts it).
                let _ = current.remove_file(&file);
                (in_a_row, stalls) = (0, 0);
                now.done += 1;
                now.bytes_done = base + size;
                i += 1;
                ctx.board.set(now.clone());
                // The library pages read the new files as they come.
                if now.done.is_multiple_of(10) {
                    (ctx.edited)();
                }
            }
            Err(why) if why == STALLED => {
                stalls += 1;
                (ctx.log)(&format!(
                    "{} ({} MB) stopped sending after {:.0} s (time {stalls}); opening the phone again",
                    sent.file,
                    size / 1_000_000,
                    at.elapsed().as_secs_f64()
                ));
                now.state = "stalled";
                now.bytes_done = base;
                ctx.board.set(now.clone());
                if stalls > STALLS_PER_SONG {
                    (ctx.log)(&format!("{} left on the phone: it stops every time", sent.file));
                    now.failed += 1;
                    continue_failed(ctx, &mut now, &file);
                    (i, stalls) = (i + 1, 0);
                    continue;
                }
                // The phone closed before it is opened again.
                drop(folder.take());
                match open_again(phone, stop) {
                    Some(again) => {
                        files = index(again.as_ref());
                        folder = Some(again);
                        (ctx.log)("the phone answered again");
                    }
                    None => {
                        (ctx.log)(&format!(
                            "the phone did not answer again after {} songs",
                            now.done
                        ));
                        now.state = "stopped";
                        now.song = None;
                        now.at_ms = now_ms();
                        ctx.board.set(now.clone());
                        (ctx.edited)();
                        return now;
                    }
                }
            }
            Err(why) => {
                // The phone gone (unplugged, File transfer turned off, its USB connection dropped: Windows can list
                // a phone for a while after it stops answering): stop, and say so; plugged in again, the rest is
                // taken. Songs failing one after another are taken as the phone gone too.
                in_a_row += 1;
                let gone = lost(&why) || in_a_row >= LOST_AFTER || !phone.still_there();
                if gone {
                    now.state = "stopped";
                    now.song = None;
                    now.at_ms = now_ms();
                    (ctx.log)(&format!("lost the phone after {} songs: {why}", now.done));
                    ctx.board.set(now.clone());
                    (ctx.edited)();
                    return now;
                }
                now.failed += 1;
                (ctx.log)(&format!("{} was not taken: {why}", sent.file));
                continue_failed(ctx, &mut now, &file);
                i += 1;
            }
        }
    }
    // Everything not taken stays on the phone, to be tried again.
    if now.failed == 0 {
        // Before the list goes: the phone counts a copy gone while it is there as on this computer.
        if let Some(current) = folder.as_deref() {
            now.had += drop_here_already(ctx, current, list, choice, &mut files);
        }
        now.left = list
            .songs
            .iter()
            .filter(|s| !choice.includes(&s.file) && files.contains_key(&s.file))
            .count();
        let list_file = files.get(stash_core::phone::LIST).cloned();
        let ended = match (folder.as_deref(), now.left) {
            (Some(folder), 0) => folder.remove().map(|()| now.cleared = true),
            (Some(folder), _) => list_file.map_or(Ok(()), |list| folder.remove_file(&list)),
            (None, _) => Ok(()),
        };
        if let Err(e) = ended {
            (ctx.log)(&format!("the transfer stayed on the phone as it was: {e}"));
            let _ = ctx
                .library
                .lock()
                .unwrap()
                .set_setting(TAKEN_SETTING, &list.sent_at_ms.to_string());
        }
        ctx.board.chosen_taken(ctx.library, list.sent_at_ms);
    }
    now.state = "done";
    now.song = None;
    now.at_ms = now_ms();
    (ctx.log)(&format!(
        "{} taken, {} here already, {} not kept (WebM), {} not taken, {} left on the phone (not chosen), in {:.0} s",
        now.taken,
        now.had,
        now.not_kept,
        now.failed,
        now.left,
        started.elapsed().as_secs_f64()
    ));
    ctx.board.set(now.clone());
    (ctx.edited)();
    now
}

/// Deletes from the phone the copies of songs not chosen that this computer has already, as each song taken is: the
/// phone sees them go and sends them no more, so a send carries only songs this computer has not got. How many went.
fn drop_here_already(
    ctx: &Ctx,
    folder: &dyn Folder,
    list: &List,
    choice: &Choice,
    files: &mut HashMap<String, PhoneFile>,
) -> usize {
    let unchosen: Vec<&Sent> = list
        .songs
        .iter()
        .filter(|s| !choice.includes(&s.file) && files.contains_key(&s.file))
        .collect();
    if unchosen.is_empty() {
        return 0;
    }
    let asked: Vec<import::Song> = unchosen.iter().map(|s| as_song(s)).collect();
    let Ok(here) = ctx.library.lock().unwrap().songs_here(&asked) else {
        return 0;
    };
    let mut gone = 0;
    for (sent, here) in unchosen.into_iter().zip(here) {
        if !here {
            continue;
        }
        let Some(file) = files.get(&sent.file) else {
            continue;
        };
        if folder.remove_file(file).is_ok() {
            files.remove(&sent.file);
            gone += 1;
        }
    }
    if gone > 0 {
        (ctx.log)(&format!(
            "{gone} songs not chosen are here already: their copies went from the phone"
        ));
    }
    gone
}

/// A folder on this computer standing in for a phone's transfer folder (`STASH_PHONE_FOLDER`): to try the transfer
/// and its window with no phone. While it is set, no phone is looked at.
struct LocalFolder(std::path::PathBuf);

impl Folder for LocalFolder {
    fn files(&self) -> Vec<PhoneFile> {
        std::fs::read_dir(&self.0)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                    .map(|e| PhoneFile {
                        object: e.path().to_string_lossy().into_owned(),
                        name: e.file_name().to_string_lossy().into_owned(),
                        size: e.metadata().map_or(0, |m| m.len()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn bytes(&self, file: &PhoneFile, cap: u64) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        std::fs::File::open(&file.object)
            .and_then(|f| std::io::Read::read_to_end(&mut std::io::Read::take(f, cap), &mut bytes))
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
    fn copy(&self, file: &PhoneFile, to: &Path, watch: &Watch) -> Result<u64, String> {
        let copied = (|| -> std::io::Result<u64> {
            let mut from = std::fs::File::open(&file.object)?;
            let mut into = std::fs::File::create(to)?;
            let (mut block, mut n) = (vec![0u8; 1 << 20], 0u64);
            loop {
                let got = std::io::Read::read(&mut from, &mut block)?;
                if got == 0 {
                    return Ok(n);
                }
                into.write_all(&block[..got])?;
                n += got as u64;
                (watch.progress)(n);
            }
        })();
        copied.map_err(|e| {
            let _ = std::fs::remove_file(to);
            e.to_string()
        })
    }
    fn remove_file(&self, file: &PhoneFile) -> Result<(), String> {
        std::fs::remove_file(&file.object).map_err(|e| e.to_string())
    }
    fn remove(&self) -> Result<(), String> {
        std::fs::remove_dir_all(&self.0).map_err(|e| e.to_string())
    }
}

impl Plugged for LocalFolder {
    fn name(&self) -> &str {
        "Test folder"
    }
    fn open_again(&self) -> Option<Box<dyn Folder>> {
        self.0
            .join(stash_core::phone::LIST)
            .is_file()
            .then(|| Box::new(LocalFolder(self.0.clone())) as Box<dyn Folder>)
    }
    fn still_there(&self) -> bool {
        self.0.is_dir()
    }
}

/// [`run`] with a folder for a phone (`STASH_PHONE_FOLDER`).
fn run_folder(state: &State, stop: &AtomicBool, dir: std::path::PathBuf) {
    let phone = LocalFolder(dir);
    let mut tried: HashMap<i64, u32> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        let asked = ASKED.load(Ordering::Relaxed);
        if asked > ANSWERED.load(Ordering::Relaxed) {
            tried.clear();
        }
        let has = phone.0.join(stash_core::phone::LIST).is_file();
        *SEEN.lock().unwrap() = vec![(phone.name().to_string(), Some(has))];
        if has {
            let edited = || state.edited();
            let note = |line: &str| log(state, line);
            let ctx = Ctx {
                library: &state.library,
                board: &BOARD,
                edited: &edited,
                log: &note,
            };
            take(&ctx, &phone, Box::new(LocalFolder(phone.0.clone())), &mut tried, stop);
        } else {
            *BOARD.offer.lock().unwrap() = None;
        }
        ANSWERED.fetch_max(asked, Ordering::Relaxed);
        let until = Instant::now() + LOOK_EVERY;
        while Instant::now() < until && ASKED.load(Ordering::Relaxed) <= ANSWERED.load(Ordering::Relaxed) {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// Looks for a phone with a transfer on it, every few seconds, and takes it.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    if let Some(dir) = std::env::var_os("STASH_PHONE_FOLDER") {
        log(state, "STASH_PHONE_FOLDER is set: a folder stands in for a phone, and no phone is looked at");
        return run_folder(state, stop, dir.into());
    }
    let mut looked: HashMap<String, Instant> = HashMap::new();
    let mut sent: HashMap<String, bool> = HashMap::new();
    let mut tried: HashMap<i64, u32> = HashMap::new();
    // For each phone, the songs in its folder while it has no list, and when they last grew in number.
    let mut grew: HashMap<String, (usize, Option<Instant>)> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        let asked = ASKED.load(Ordering::Relaxed);
        if asked > ANSWERED.load(Ordering::Relaxed) {
            looked.clear();
            tried.clear();
        }
        if let Ok(phones) = stash_core::phone::phones() {
            // A phone unplugged before anything was taken: nothing to say, and nothing to choose from.
            {
                let mut now = BOARD.now.lock().unwrap();
                let gone = now.as_ref().is_some_and(|n| {
                    matches!(n.state, "connected" | "preparing" | "ready")
                        && !phones.iter().any(|p| Some(&p.name) == n.phone.as_ref())
                });
                if gone {
                    *now = None;
                }
            }
            {
                let mut offer = BOARD.offer.lock().unwrap();
                if offer.as_ref().is_some_and(|o| !phones.iter().any(|p| p.name == o.phone)) {
                    *offer = None;
                }
            }
            looked.retain(|id, _| phones.iter().any(|p| &p.id == id));
            sent.retain(|id, _| phones.iter().any(|p| &p.id == id));
            grew.retain(|id, _| phones.iter().any(|p| &p.id == id));
            *SEEN.lock().unwrap() = phones
                .iter()
                .map(|p| (p.name.clone(), sent.get(&p.id).copied()))
                .collect();
            for phone in phones {
                if looked
                    .get(&phone.id)
                    .is_some_and(|at| at.elapsed() < PHONE_AGAIN)
                {
                    continue;
                }
                looked.insert(phone.id.clone(), Instant::now());
                #[cfg(windows)]
                {
                    let transfer = stash_core::phone::open_transfer(&phone.id).ok().flatten();
                    let has = transfer
                        .as_ref()
                        .is_some_and(|t| t.files.iter().any(|f| f.name == stash_core::phone::LIST));
                    sent.insert(phone.id.clone(), has);
                    seen_inside(&phone.name, has);
                    // Songs coming into the folder with no list yet: the phone is still copying them out. Songs
                    // there that are not growing in number are copies left from a send taken already.
                    let ready = transfer.as_ref().map_or(0, |t| t.files.len());
                    if has {
                        grew.remove(&phone.id);
                    } else {
                        let (before, at) = grew.get(&phone.id).copied().unwrap_or((ready, None));
                        let at = if ready > before { Some(Instant::now()) } else { at };
                        grew.insert(phone.id.clone(), (ready, at));
                        let copying =
                            ready > 0 && at.is_some_and(|t| t.elapsed() < COPYING_QUIET);
                        tell(
                            &phone.name,
                            if copying { "preparing" } else { "connected" },
                            if copying { ready } else { 0 },
                        );
                    }
                    if let Some(transfer) = transfer.filter(|_| has) {
                        let edited = || state.edited();
                        let note = |line: &str| log(state, line);
                        let ctx = Ctx {
                            library: &state.library,
                            board: &BOARD,
                            edited: &edited,
                            log: &note,
                        };
                        take(&ctx, &Usb(&phone), Box::new(transfer), &mut tried, stop);
                        // Taken and deleted from the phone: it has no songs sent now.
                        let cleared = BOARD.now.lock().unwrap().as_ref().is_some_and(|n| {
                            n.cleared
                                && n.state == "done"
                                && n.phone.as_deref() == Some(phone.name.as_str())
                        });
                        sent.insert(phone.id.clone(), has && !cleared);
                        seen_inside(&phone.name, has && !cleared);
                    }
                }
            }
        }
        ANSWERED.fetch_max(asked, Ordering::Relaxed);
        let until = Instant::now() + LOOK_EVERY;
        while Instant::now() < until
            && ASKED.load(Ordering::Relaxed) <= ANSWERED.load(Ordering::Relaxed)
        {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// Says a phone is plugged in with nothing sent ("connected") or is copying its songs out ("preparing", `ready` so
/// far): its send, if one was offered, is gone. What a transfer said last (taking, done, stopped) stays until the
/// phone has something new to say: a phone still plugged in after its songs were taken is not "connected" again over
/// the result. Another phone's news stays over a phone just plugged in, and over one copying while that news is a
/// send to choose from or one being taken.
#[cfg(windows)]
fn tell(phone: &str, what: &'static str, ready: usize) {
    if told(&mut BOARD.now.lock().unwrap(), phone, what, ready) {
        let mut offer = BOARD.offer.lock().unwrap();
        if offer.as_ref().is_some_and(|o| o.phone == phone) {
            *offer = None;
        }
    }
}

/// [`tell`] on `now`: whether it was said.
fn told(now: &mut Option<Now>, phone: &str, what: &'static str, ready: usize) -> bool {
    let same = now.as_ref().filter(|n| n.phone.as_deref() == Some(phone));
    let other = now.as_ref().filter(|n| n.phone.as_deref() != Some(phone));
    if what == "connected"
        && (other.is_some()
            || same.is_some_and(|n| matches!(n.state, "taking" | "stalled" | "done" | "stopped")))
    {
        return false;
    }
    if other.is_some_and(|n| matches!(n.state, "ready" | "taking" | "stalled")) {
        return false;
    }
    // The same news keeps its time, so a card closed on it stays closed.
    let at_ms = same
        .filter(|n| n.state == what)
        .map_or_else(now_ms, |n| n.at_ms);
    *now = Some(Now {
        state: what,
        phone: Some(phone.to_string()),
        ready,
        at_ms,
        ..Default::default()
    });
    true
}

/// Whether every send is taken whole as it comes (Settings), not offered to choose from.
fn take_all(library: &Mutex<Library>) -> bool {
    library.lock().unwrap().setting(TAKE_ALL_SETTING).ok().flatten().as_deref() == Some("1")
}

/// What `POST /api/phone/take` asks for: the send (`sent_at_ms`), and `all: true`, its songs by their `files`, or
/// `none: true` (nothing taken: the phone clears its copies).
fn chosen(body: &serde_json::Value) -> Result<(i64, Choice), &'static str> {
    let sent_at_ms = body["sent_at_ms"].as_i64().ok_or("which send is this for?")?;
    if body["all"].as_bool() == Some(true) {
        return Ok((sent_at_ms, Choice::All));
    }
    let files: HashSet<String> = body["files"]
        .as_array()
        .ok_or("which songs?")?
        .iter()
        .filter_map(|f| f.as_str().map(String::from))
        .collect();
    if files.is_empty() && body["none"].as_bool() != Some(true) {
        return Err("no songs chosen");
    }
    Ok((sent_at_ms, Choice::These(files)))
}

/// Takes from the send offered now what was chosen from it (only songs it offered): how many songs that is. The window
/// is told at once that they are being taken.
fn choose_from(
    library: &Mutex<Library>,
    board: &Board,
    sent_at_ms: i64,
    choice: Choice,
) -> Result<usize, &'static str> {
    let offer = board
        .offer
        .lock()
        .unwrap()
        .clone()
        .filter(|o| o.sent_at_ms == sent_at_ms)
        .ok_or("those songs are no longer waiting on the phone")?;
    let choice = match choice {
        Choice::All => Choice::All,
        Choice::These(files) => {
            Choice::These(offer.songs.iter().filter(|s| files.contains(&s.file)).map(|s| s.file.clone()).collect())
        }
    };
    let taking = match &choice {
        Choice::All => offer.songs.len(),
        Choice::These(files) => files.len(),
    };
    board.choose(library, sent_at_ms, choice);
    board.set(Now {
        state: "taking",
        phone: Some(offer.phone),
        total: taking,
        at_ms: now_ms(),
        ..Default::default()
    });
    Ok(taking)
}

#[cfg(windows)]
fn seen_inside(name: &str, has: bool) {
    for (n, h) in SEEN.lock().unwrap().iter_mut() {
        if n == name {
            *h = Some(has);
        }
    }
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    match (request.method(), path) {
        (Method::Get, "/api/phone/songs") => {
            let Some(mut offer) = BOARD.offer.lock().unwrap().clone() else {
                return Ok(Some(refuse(404, "no songs are waiting on a phone")));
            };
            // Asked again: songs may have come into the library since they were offered.
            if let Ok(here) = state.library.lock().unwrap().songs_here(&offer.asked) {
                for (song, here) in offer.songs.iter_mut().zip(here) {
                    song.here = here;
                }
            }
            Ok(Some(reply_json(200, json!(offer))))
        }
        (Method::Post, "/api/phone/take") => {
            let body = body_json(request)?;
            Ok(Some(match chosen(&body) {
                Ok((sent_at_ms, choice)) => match choose_from(&state.library, &BOARD, sent_at_ms, choice) {
                    Ok(taking) => {
                        // The transfer starts now, not at the next look.
                        ASKED.fetch_add(1, Ordering::Relaxed);
                        reply_json(200, json!({ "taking": taking }))
                    }
                    Err(why) => refuse(409, why),
                },
                Err(why) => refuse(400, why),
            }))
        }
        (Method::Get, "/api/settings/phone") => Ok(Some(reply_json(
            200,
            json!({ "take_all": take_all(&state.library) }),
        ))),
        (Method::Post, "/api/settings/phone") => {
            let body = body_json(request)?;
            let Some(on) = body["take_all"].as_bool() else {
                return Ok(Some(refuse(400, "take_all is true or false")));
            };
            state
                .library
                .lock()
                .unwrap()
                .set_setting(TAKE_ALL_SETTING, if on { "1" } else { "0" })?;
            Ok(Some(reply_json(200, json!({ "take_all": on }))))
        }
        (Method::Get, "/api/phone") => {
            let now = BOARD.now.lock().unwrap().clone().unwrap_or(Now {
                state: "idle",
                ..Default::default()
            });
            let mut out = json!(now);
            out["answered"] = json!(ANSWERED.load(Ordering::Relaxed));
            out["phones"] = SEEN
                .lock()
                .unwrap()
                .iter()
                .map(|(name, has)| json!({ "name": name, "has_songs": has }))
                .collect();
            Ok(Some(reply_json(200, out)))
        }
        (Method::Post, "/api/phone/look") => {
            let asked = ASKED.fetch_add(1, Ordering::Relaxed) + 1;
            Ok(Some(reply_json(200, json!({ "asked": asked }))))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sent(json: &str) -> Sent {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_phone_songs_ids_are_taken_only_when_they_are_ids() {
        let s = sent(
            r#"{"file":"7.flac","title":" Song ","artist":"Band","album":"","isrc":"usabc1234567",
                "youtube_id":"dQw4w9WgXcQ","spotify_uri":"spotify:track:4uLU6hMCjMI75M1A2tKUQC",
                "art_url":"http://cover","liked":true,"bits":24}"#,
        );
        let song = as_song(&s);
        assert_eq!(song.title, "Song");
        assert_eq!(song.album, None);
        assert_eq!(song.isrc.as_deref(), Some("USABC1234567"));
        assert_eq!(song.art_url, None);
        assert_eq!(
            song.refs,
            vec![
                ("youtube".to_string(), "dQw4w9WgXcQ".to_string()),
                ("spotify".to_string(), "4uLU6hMCjMI75M1A2tKUQC".to_string()),
            ]
        );
        let odd = as_song(&sent(
            r#"{"file":"8.m4a","title":"T","artist":"A","youtube_id":"short","spotify_uri":"https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=x","isrc":"bad"}"#,
        ));
        assert_eq!(odd.isrc, None);
        assert_eq!(
            odd.refs,
            vec![("spotify".to_string(), "4uLU6hMCjMI75M1A2tKUQC".to_string())]
        );
    }

    #[test]
    fn a_list_of_another_kind_is_not_read_as_one() {
        let list: List =
            serde_json::from_str(r#"{"stash_transfer":1,"sent_at_ms":5,"songs":[]}"#).unwrap();
        assert_eq!(list.stash_transfer, 1);
        assert!(serde_json::from_str::<List>(r#"{"songs":[]}"#).is_err());
    }

    /// A moment of silence as a WAV file: a song file the library can read.
    fn wav() -> Vec<u8> {
        let data = 800u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&8_000u32.to_le_bytes());
        b.extend_from_slice(&16_000u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        b.resize(44 + data as usize, 0);
        b
    }

    /// A phone in memory: its name, its transfer folder's files, which of them stop sending (how many more times),
    /// whether it answers when opened again, and every read and delete asked of it.
    #[derive(Default)]
    struct Phone {
        name: String,
        files: Mutex<Vec<(String, Vec<u8>)>>,
        stalls: Mutex<HashMap<String, u32>>,
        answers_again: AtomicBool,
        reads: Mutex<Vec<String>>,
        opened_again: AtomicU64,
        /// Still listed, but every read fails as when its USB connection dropped.
        unreachable: AtomicBool,
    }

    impl Phone {
        fn with(songs: &[(&str, Vec<u8>)]) -> Arc<Phone> {
            let entries = songs
                .iter()
                .map(|(n, _)| json!({ "file": n, "title": format!("Song {n}"), "artist": "Someone" }))
                .collect();
            Phone::sending("Test phone", 7, songs, entries)
        }

        /// A phone named `name` with a send made at `sent_at_ms`: the files, and the list's entry for each.
        fn sending(
            name: &str,
            sent_at_ms: i64,
            songs: &[(&str, Vec<u8>)],
            entries: Vec<serde_json::Value>,
        ) -> Arc<Phone> {
            let phone = Phone {
                name: name.to_string(),
                ..Default::default()
            };
            let mut files: Vec<(String, Vec<u8>)> =
                songs.iter().map(|(n, b)| (n.to_string(), b.clone())).collect();
            let list = json!({ "stash_transfer": 1, "sent_at_ms": sent_at_ms, "songs": entries });
            files.push((stash_core::phone::LIST.to_string(), list.to_string().into_bytes()));
            *phone.files.lock().unwrap() = files;
            phone.answers_again.store(true, Ordering::Relaxed);
            Arc::new(phone)
        }

        fn stall(&self, name: &str, times: u32) {
            self.stalls.lock().unwrap().insert(name.to_string(), times);
        }

        fn left(&self) -> Vec<String> {
            self.files.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
        }
    }

    struct Open(Arc<Phone>);

    impl Folder for Open {
        fn files(&self) -> Vec<PhoneFile> {
            self.0
                .files
                .lock()
                .unwrap()
                .iter()
                .map(|(n, b)| PhoneFile {
                    object: n.clone(),
                    name: n.clone(),
                    size: b.len() as u64,
                })
                .collect()
        }
        fn bytes(&self, file: &PhoneFile, _cap: u64) -> Result<Vec<u8>, String> {
            if self.0.unreachable.load(Ordering::Relaxed) {
                return Err("the device is unreachable (0x80070141)".to_string());
            }
            let files = self.0.files.lock().unwrap();
            Ok(files.iter().find(|(n, _)| *n == file.name).unwrap().1.clone())
        }
        fn copy(&self, file: &PhoneFile, to: &Path, watch: &Watch) -> Result<u64, String> {
            self.0.reads.lock().unwrap().push(file.name.clone());
            if let Some(left) = self.0.stalls.lock().unwrap().get_mut(&file.name).filter(|n| **n > 0) {
                *left -= 1;
                (watch.stalled)();
                return Err(STALLED.to_string());
            }
            let bytes = self.bytes(file, 0)?;
            std::fs::write(to, &bytes).map_err(|e| e.to_string())?;
            (watch.progress)(bytes.len() as u64);
            Ok(bytes.len() as u64)
        }
        fn remove_file(&self, file: &PhoneFile) -> Result<(), String> {
            self.0.files.lock().unwrap().retain(|(n, _)| *n != file.name);
            Ok(())
        }
        fn remove(&self) -> Result<(), String> {
            self.0.files.lock().unwrap().clear();
            Ok(())
        }
    }

    impl Plugged for Arc<Phone> {
        fn name(&self) -> &str {
            &self.name
        }
        fn open_again(&self) -> Option<Box<dyn Folder>> {
            self.opened_again.fetch_add(1, Ordering::Relaxed);
            self.answers_again
                .load(Ordering::Relaxed)
                .then(|| Box::new(Open(Arc::clone(self))) as Box<dyn Folder>)
        }
        fn still_there(&self) -> bool {
            true
        }
    }

    /// A library in memory saving into a folder of its own, and a board of its own.
    struct Here {
        library: Mutex<Library>,
        board: Board,
        dir: std::path::PathBuf,
    }

    impl Here {
        /// One that takes every send whole, as it comes.
        fn new(name: &str) -> Here {
            let here = Here::asking(name);
            here.take_all(true);
            here
        }

        /// One that offers each send to choose from (as Stash does unless told otherwise).
        fn asking(name: &str) -> Here {
            let dir = std::env::temp_dir().join(format!("stash-phone-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let mut library = Library::open_in_memory().unwrap();
            library
                .set_setting(crate::saving::FOLDER, &dir.to_string_lossy())
                .unwrap();
            Here {
                library: Mutex::new(library),
                board: Board::new(),
                dir,
            }
        }

        fn take_all(&self, on: bool) {
            self.library
                .lock()
                .unwrap()
                .set_setting(TAKE_ALL_SETTING, if on { "1" } else { "0" })
                .unwrap();
        }

        fn take(&self, phone: &Arc<Phone>, tried: &mut HashMap<i64, u32>) -> Option<Now> {
            self.take_on(&self.board, phone, tried)
        }

        /// A look at `phone` with `board` (a fresh one: as after a restart).
        fn take_on(&self, board: &Board, phone: &Arc<Phone>, tried: &mut HashMap<i64, u32>) -> Option<Now> {
            let (edited, note) = (|| {}, |_: &str| {});
            let ctx = Ctx {
                library: &self.library,
                board,
                edited: &edited,
                log: &note,
            };
            take(&ctx, phone, Box::new(Open(Arc::clone(phone))), tried, &AtomicBool::new(false));
            board.now.lock().unwrap().clone()
        }

        fn choose(&self, sent_at_ms: i64, files: &[&str]) -> Result<usize, &'static str> {
            let files = files.iter().map(|f| f.to_string()).collect();
            choose_from(&self.library, &self.board, sent_at_ms, Choice::These(files))
        }

        /// The song files saved here (not the unfinished copies).
        fn saved(&self) -> usize {
            fn count(dir: &Path) -> usize {
                std::fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .flatten()
                            .map(|e| {
                                let path = e.path();
                                if path.is_dir() {
                                    count(&path)
                                } else {
                                    usize::from(!e.file_name().to_string_lossy().starts_with('.'))
                                }
                            })
                            .sum()
                    })
                    .unwrap_or(0)
            }
            count(&self.dir)
        }
    }

    impl Drop for Here {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_song_that_stops_sending_is_taken_again_once_the_phone_answers() {
        let here = Here::new("stall");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        let now = here.take(&phone, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.done, now.total), ("done", 3, 0, 3, 3));
        assert!(now.cleared, "everything came in: the folder is gone from the phone");
        assert_eq!(*phone.reads.lock().unwrap(), ["1.wav", "2.wav", "2.wav", "3.wav"]);
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), 1);
        assert_eq!(here.saved(), 3);
        // The window was told it stalled, then that it went on.
        let states = here.board.states.lock().unwrap().clone();
        let stalled = states.iter().position(|s| *s == "stalled").expect("told it stalled");
        assert_eq!(states[stalled + 1..].iter().find(|s| **s != "stalled"), Some(&"taking"));
    }

    #[test]
    fn a_song_that_stops_every_time_is_left_and_the_rest_come_in() {
        let here = Here::new("stuck");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 99);
        let now = here.take(&phone, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.done), ("done", 2, 1, 3));
        assert!(!now.cleared);
        // Left on the phone for another send: the song and the list.
        assert_eq!(phone.left(), ["2.wav", stash_core::phone::LIST]);
        assert_eq!(phone.reads.lock().unwrap().iter().filter(|r| *r == "2.wav").count(), 3);
    }

    #[test]
    fn a_phone_that_does_not_answer_again_stops_and_keeps_the_rest() {
        let here = Here::new("gone");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        phone.answers_again.store(false, Ordering::Relaxed);
        let now = here.take(&phone, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.taken, now.done), ("stopped", 1, 1));
        assert_eq!(phone.left(), ["2.wav", "3.wav", stash_core::phone::LIST]);
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), AGAIN_AFTER.len() as u64);
    }

    #[test]
    fn a_song_here_already_is_not_read_from_the_phone() {
        let here = Here::new("had");
        let first = Phone::with(&[("1.wav", wav())]);
        assert_eq!(here.take(&first, &mut HashMap::new()).unwrap().taken, 1);
        // The same song sent again (a new send of it).
        let again = Phone::with(&[("1.wav", wav())]);
        let now = here.take(&again, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.taken, now.had), ("done", 0, 1));
        assert!(again.reads.lock().unwrap().is_empty(), "nothing read for a song here already");
        assert!(now.cleared);
    }

    #[test]
    fn a_webm_is_not_kept_and_leaves_nothing_behind() {
        let here = Here::new("webm");
        let mut webm = WEBM.to_vec();
        webm.extend_from_slice(&[0u8; 60]);
        let phone = Phone::with(&[("1.webm", webm)]);
        let now = here.take(&phone, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.not_kept, now.taken), ("done", 1, 0));
        assert_eq!(here.saved(), 0);
        let leftovers = std::fs::read_dir(&here.dir).unwrap().count();
        assert_eq!(leftovers, 0, "no unfinished copy left");
    }

    #[test]
    fn only_tries_that_bring_nothing_in_count_towards_leaving_a_transfer() {
        // A phone that stops on every song after the first and never answers again: each plug-in brings one in.
        let here = Here::new("tries");
        let names: Vec<String> = (1..=5).map(|n| format!("{n}.wav")).collect();
        let songs: Vec<(&str, Vec<u8>)> = names.iter().map(|n| (n.as_str(), wav())).collect();
        let phone = Phone::with(&songs);
        phone.answers_again.store(false, Ordering::Relaxed);
        let mut tried = HashMap::new();
        for (i, name) in names.iter().enumerate().skip(1) {
            phone.stall(name, 1);
            let now = here.take(&phone, &mut tried).unwrap();
            assert_eq!((now.state, now.done), ("stopped", i), "plug-in {i} brought one more in");
        }
        assert!(tried.is_empty(), "every try brought a song in: none counted");
        // Now one that brings nothing in, plug-in after plug-in: left after TRIES of them.
        phone.stall("5.wav", 99);
        let reads = || phone.reads.lock().unwrap().len();
        for _ in 0..TRIES {
            here.take(&phone, &mut tried);
        }
        let before = reads();
        here.take(&phone, &mut tried);
        assert_eq!(reads(), before, "left: not tried again");
        assert_eq!(tried.get(&7), Some(&TRIES));
    }

    /// Three songs sent at 8: 1.wav is a FLAC on the phone with all its tags, in a playlist.
    fn three(name: &str) -> Arc<Phone> {
        let entries = vec![
            json!({ "file": "1.wav", "title": "Song 1.wav", "artist": "Someone", "album": "First",
                    "album_artist": "Someone", "format": "FLAC", "bits": 24, "sample_rate": 96000,
                    "duration_ms": 1000, "art_url": "https://cover/1", "liked": true, "playlists": ["Gym"] }),
            json!({ "file": "2.wav", "title": "Song 2.wav", "artist": "Someone", "art_url": "http://not-safe" }),
            json!({ "file": "3.wav", "title": "Song 3.wav", "artist": "Other" }),
        ];
        Phone::sending(name, 8, &[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())], entries)
    }

    #[test]
    fn a_send_waits_to_be_chosen_from_and_says_what_is_here_already() {
        let here = Here::asking("offer");
        here.take_all(true);
        assert_eq!(here.take(&Phone::with(&[("1.wav", wav())]), &mut HashMap::new()).unwrap().taken, 1);
        here.take_all(false);
        let phone = three("Test phone");
        let mut tried = HashMap::new();
        let now = here.take(&phone, &mut tried).unwrap();
        assert_eq!((now.state, now.total, now.here), ("ready", 3, 1));
        assert_eq!(now.bytes_total, 3 * wav().len() as u64);
        assert!(phone.reads.lock().unwrap().is_empty(), "nothing read before it is chosen");
        assert_eq!(phone.left().len(), 4, "nothing deleted from the phone");
        assert!(tried.is_empty(), "waiting is not a try");
        let offer = here.board.offer.lock().unwrap().clone().unwrap();
        assert_eq!((offer.phone.as_str(), offer.sent_at_ms), ("Test phone", 8));
        let first = &offer.songs[0];
        assert_eq!((first.here, first.format.as_deref(), first.bits, first.sample_rate), (true, Some("FLAC"), Some(24), Some(96000)));
        assert_eq!((first.album.as_deref(), first.liked, first.playlists.clone()), (Some("First"), true, vec!["Gym".to_string()]));
        assert_eq!(offer.songs[1].art_url, None, "only a safe cover is offered");
        assert!(!offer.songs[1].here && !offer.songs[2].here);
        // Looked at again: offered as it was, and said again when something else was said over it.
        *here.board.now.lock().unwrap() = None;
        assert_eq!(here.take(&phone, &mut tried).unwrap().state, "ready");
    }

    #[test]
    fn only_the_songs_chosen_are_taken_and_the_rest_left_for_the_phone_to_clear() {
        let here = Here::asking("chosen");
        let phone = three("Test phone");
        assert_eq!(here.take(&phone, &mut HashMap::new()).unwrap().state, "ready");
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        let told = here.board.now.lock().unwrap().clone().unwrap();
        assert_eq!((told.state, told.total), ("taking", 1), "told at once");
        assert!(here.board.offer.lock().unwrap().is_none(), "no longer offered");
        let now = here.take(&phone, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.taken, now.total, now.left, now.cleared), ("done", 1, 1, 2, false));
        assert_eq!(*phone.reads.lock().unwrap(), ["2.wav"]);
        // The list is gone (the phone then knows the computer is done); the songs not chosen are the phone's to clear.
        assert_eq!(phone.left(), ["1.wav", "3.wav"]);
        assert_eq!(here.board.choice(&here.library, 8), None, "the choice is let go");
        assert_eq!(here.saved(), 1);
    }

    #[test]
    fn songs_not_chosen_that_are_here_already_go_from_the_phone_too() {
        let here = Here::asking("known");
        here.take_all(true);
        assert_eq!(here.take(&Phone::with(&[("1.wav", wav())]), &mut HashMap::new()).unwrap().taken, 1);
        here.take_all(false);
        let phone = three("Test phone");
        assert_eq!(here.take(&phone, &mut HashMap::new()).unwrap().state, "ready");
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        let now = here.take(&phone, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.taken, now.had, now.left), ("done", 1, 1, 1));
        // 1.wav is here already: its copy went too, so the phone sends it no more. 3.wav is new, and was not chosen.
        assert_eq!(phone.left(), ["3.wav"]);
        assert_eq!(*phone.reads.lock().unwrap(), ["2.wav"], "nothing read for a song here already");
    }

    #[test]
    fn a_choice_carries_on_after_a_restart() {
        let here = Here::asking("restart");
        let phone = three("Test phone");
        here.take(&phone, &mut HashMap::new());
        assert_eq!(here.choose(8, &["1.wav", "2.wav"]), Ok(2));
        phone.stall("2.wav", 1);
        phone.answers_again.store(false, Ordering::Relaxed);
        let now = here.take(&phone, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.taken), ("stopped", 1));
        // Stash starts again: what was chosen is kept in the library, and the rest of it comes in.
        phone.answers_again.store(true, Ordering::Relaxed);
        let fresh = Board::new();
        let now = here.take_on(&fresh, &phone, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.taken, now.done, now.total, now.left), ("done", 1, 2, 2, 1));
        assert_eq!(phone.left(), ["3.wav"]);
        assert!(!phone.reads.lock().unwrap().iter().any(|r| r == "3.wav"), "3.wav was not chosen");
    }

    #[test]
    fn taking_nothing_leaves_every_song_and_removes_the_list() {
        let here = Here::asking("none");
        let phone = three("Test phone");
        here.take(&phone, &mut HashMap::new());
        let (at, choice) = chosen(&json!({ "sent_at_ms": 8, "files": [], "none": true })).unwrap();
        assert_eq!(choose_from(&here.library, &here.board, at, choice), Ok(0));
        let now = here.take(&phone, &mut HashMap::new()).unwrap();
        assert_eq!((now.state, now.taken, now.left), ("done", 0, 3));
        assert!(phone.reads.lock().unwrap().is_empty());
        assert_eq!(phone.left(), ["1.wav", "2.wav", "3.wav"]);
    }

    #[test]
    fn only_songs_the_send_offered_can_be_chosen() {
        let here = Here::asking("only");
        let phone = three("Test phone");
        assert_eq!(here.choose(8, &["1.wav"]), Err("those songs are no longer waiting on the phone"), "not offered yet");
        here.take(&phone, &mut HashMap::new());
        assert!(here.choose(9, &["1.wav"]).is_err(), "another send");
        assert_eq!(here.choose(8, &["1.wav", "9.wav", "stash-transfer.json"]), Ok(1));
        assert_eq!(
            here.board.choice(&here.library, 8),
            Some(Choice::These(HashSet::from(["1.wav".to_string()])))
        );
    }

    #[test]
    fn what_is_asked_to_be_taken_is_read_as_meant() {
        assert_eq!(chosen(&json!({ "sent_at_ms": 8, "all": true })), Ok((8, Choice::All)));
        assert_eq!(
            chosen(&json!({ "sent_at_ms": 8, "files": ["1.flac", 3] })),
            Ok((8, Choice::These(HashSet::from(["1.flac".to_string()]))))
        );
        assert!(chosen(&json!({ "sent_at_ms": 8, "files": [] })).is_err(), "nothing chosen by mistake");
        assert!(chosen(&json!({ "files": ["1.flac"] })).is_err());
        assert!(chosen(&json!({ "sent_at_ms": 8 })).is_err());
        for choice in [Choice::All, Choice::These(HashSet::new()), Choice::These(HashSet::from(["a".to_string()]))] {
            assert_eq!(Choice::from_kept(&choice.kept(8)), Some((8, choice)));
        }
        assert_eq!(Choice::from_kept(""), None);
    }

    #[test]
    fn one_send_is_offered_at_a_time() {
        let here = Here::asking("two");
        let (a, b) = (three("Phone A"), three("Phone B"));
        here.take(&a, &mut HashMap::new());
        let now = here.take(&b, &mut HashMap::new()).unwrap();
        assert_eq!(now.phone.as_deref(), Some("Phone A"), "Phone A's send is still the one shown");
        assert_eq!(here.board.offer.lock().unwrap().as_ref().unwrap().phone, "Phone A");
    }

    #[test]
    fn a_send_waiting_to_be_chosen_from_stays_offered_while_the_phone_does_not_answer() {
        let here = Here::asking("dropout");
        let phone = three("Test phone");
        let mut tried = HashMap::new();
        assert_eq!(here.take(&phone, &mut tried).unwrap().state, "ready");
        phone.unreachable.store(true, Ordering::Relaxed);
        let now = here.take(&phone, &mut tried).unwrap();
        assert_eq!(now.state, "ready", "nothing was being taken: not a transfer cut short");
        assert!(here.board.offer.lock().unwrap().is_some(), "still offered, with what was chosen from it");
        phone.unreachable.store(false, Ordering::Relaxed);
        assert_eq!(here.take(&phone, &mut tried).unwrap().state, "ready");
        // Chosen from, then not answering: that is a transfer cut short, which carries on once it answers.
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        phone.unreachable.store(true, Ordering::Relaxed);
        assert_eq!(here.take(&phone, &mut tried).unwrap().state, "stopped");
        phone.unreachable.store(false, Ordering::Relaxed);
        let now = here.take(&phone, &mut tried).unwrap();
        assert_eq!((now.state, now.taken), ("done", 1));
    }

    #[test]
    fn a_send_waiting_behind_another_phones_is_offered_once_that_one_is_taken() {
        let here = Here::asking("behind");
        let a = three("Phone A");
        let b = Phone::sending("Phone B", 9, &[("9.wav", wav())], vec![json!({ "file": "9.wav", "title": "B song", "artist": "Someone" })]);
        let mut tried = HashMap::new();
        assert_eq!(here.take(&a, &mut tried).unwrap().phone.as_deref(), Some("Phone A"));
        here.take(&b, &mut tried);
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        // Windows lists B first: its send is kept to offer, but not said over A's being taken.
        let now = here.take(&b, &mut tried).unwrap();
        assert_eq!((now.state, now.phone.as_deref()), ("taking", Some("Phone A")));
        let done = here.take(&a, &mut tried).unwrap();
        assert_eq!((done.state, done.phone.as_deref(), done.taken), ("done", Some("Phone A"), 1));
        // Then B's send is said, over A's result.
        let now = here.take(&b, &mut tried).unwrap();
        assert_eq!((now.state, now.phone.as_deref(), now.total), ("ready", Some("Phone B"), 1));
        assert_eq!(here.board.offer.lock().unwrap().as_ref().unwrap().phone, "Phone B");
    }

    #[test]
    fn a_phone_with_nothing_new_to_say_does_not_hide_another_phones_news() {
        let card = |phone: &str, state: &'static str| {
            Some(Now { state, phone: Some(phone.to_string()), at_ms: 5, ..Default::default() })
        };
        // Just plugged in: never over another phone's card.
        for state in ["preparing", "ready", "taking", "done", "stopped"] {
            let mut now = card("A", state);
            assert!(!told(&mut now, "B", "connected", 0), "connected over A's {state}");
            assert_eq!(now.unwrap().phone.as_deref(), Some("A"));
        }
        // Copying: over what another phone did, not over a send waiting to be chosen from or being taken.
        assert!(told(&mut card("A", "done"), "B", "preparing", 4));
        for state in ["ready", "taking", "stalled"] {
            assert!(!told(&mut card("A", state), "B", "preparing", 4), "preparing over A's {state}");
        }
        // The same phone: its send gone from the folder (cancelled, or sent again) is no longer to choose from; a
        // result stays over its "connected".
        assert!(told(&mut card("A", "ready"), "A", "preparing", 4));
        assert!(told(&mut card("A", "ready"), "A", "connected", 0));
        assert!(!told(&mut card("A", "done"), "A", "connected", 0));
        let mut same = card("A", "preparing");
        assert!(told(&mut same, "A", "preparing", 9));
        assert_eq!(same.as_ref().map(|n| (n.ready, n.at_ms)), Some((9, 5)), "the same news keeps its time");
        assert!(told(&mut None, "A", "connected", 0));
    }
}
