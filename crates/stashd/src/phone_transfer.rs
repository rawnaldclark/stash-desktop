//! Songs from Stash for Android over USB. "Send to computer" on the phone leaves its downloads in
//! `Download/Stash transfer/` with `stash-transfer.json` saying which song each file is; with the phone plugged in
//! ("File transfer"), the listener chooses which of them to take (each said to be here already or not, from what the
//! list says, before any is read), and this takes them into the library as downloaded songs, each deleted from the
//! phone once taken. Then its list is deleted from the phone (and the folder, once nothing is left in it): the phone
//! then clears its copies of songs not chosen. Settings can have every send taken whole as it comes, without asking.
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
use stash_core::phone::{HERE, PhoneFile, STALL, STALLED, Watch};
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, now_ms, refuse, reply_json};

/// How often the phones are looked at, and how often again a phone with no transfer on it.
const LOOK_EVERY: Duration = Duration::from_secs(2);
const PHONE_AGAIN: Duration = Duration::from_secs(3);
/// The largest list read.
const LIST_CAP: u64 = 16 * 1024 * 1024;
/// A transfer an earlier Stash took whole but could not delete from the phone (it kept it from being taken twice):
/// only its list is removed, so the phone knows this computer is done.
const TAKEN_SETTING: &str = "phone.taken";
/// A transfer is tried this many times in a row with nothing coming in before it is left (while Stash runs; plugging
/// the phone in again, or Settings' "Look for my phone", tries again). A try that takes songs off the phone starts the
/// count again, however often the phone has to be plugged back in.
const TRIES: u32 = 3;
/// How often one song may stop sending before it is left on the phone (for another send) and the rest go on: counted
/// across tries, so a song that stops every time does not hold back the songs after it.
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
    /// "taking", "stalled" (the phone stopped sending mid-song: it is being asked again), "done", "stopped"
    /// (unplugged while taking, or not answering again after stalling), "withdrawn" (the phone stopped the send while
    /// it was taken: its list gone), or "cannot_save" (songs can't be saved in `folder` on this computer: `error` says
    /// why; tried again once the phone is plugged in again).
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
    /// The send's list went from the phone with none of its songs left there (the phone then has nothing sent).
    cleared: bool,
    /// Every song chosen is in, but the send's list stayed on the phone: it is removed at the next look, so the phone
    /// is to stay plugged in (or be plugged in again) until then.
    list_left: bool,
    /// The songs not taken were left out of the phone's listing (as it was opened again): the next look takes them,
    /// with no need to send again.
    trying_again: bool,
    at_ms: i64,
    error: Option<String>,
    /// The downloads folder, when songs can't be saved in it.
    folder: Option<String>,
    /// The send this is about (its `sent_at_ms`): a try that carries on the same send adds to what was said of it.
    /// Not told.
    #[serde(skip)]
    sent_at_ms: Option<i64>,
    /// Songs counted in (taken, or here already) whose copy stayed on the phone when it was deleted: deleted again,
    /// not counted again. Not told.
    #[serde(skip)]
    undeleted: Vec<String>,
    /// What was said of the same send before this try: a try that brings nothing more in and ends as that did keeps
    /// its time. Not told.
    #[serde(skip)]
    before: Option<Before>,
    /// Of this try: the phone's copies deleted, and songs newly left on it for stopping too often. Either is the try
    /// getting somewhere (see [`TRIES`]). Not told.
    #[serde(skip)]
    removed: usize,
    #[serde(skip)]
    gave_up: usize,
    /// Of this try: songs not taken because the phone's listing left them out. Not told.
    #[serde(skip)]
    unlisted: usize,
}

/// What was said of a send before a try that carries it on: how it ended, when, and how many songs it counted in.
#[derive(Debug, Clone, Copy)]
struct Before {
    state: &'static str,
    at_ms: i64,
    brought: usize,
}

impl Now {
    /// The songs counted in: taken, here already, or in the library only.
    fn brought(&self) -> usize {
        self.taken + self.had + self.not_kept
    }

    /// Whether this is what was said of `phone`'s send made at `sent_at_ms`.
    fn is_of(&self, phone: &str, sent_at_ms: i64) -> bool {
        self.phone.as_deref() == Some(phone) && self.sent_at_ms == Some(sent_at_ms)
    }
}

/// What the looks keep of each send between its tries (while Stash runs).
#[derive(Default)]
struct Tries {
    /// By send (`sent_at_ms`): tries in a row that took nothing off the phone.
    tried: HashMap<i64, u32>,
    /// By send and file: how often the song stopped sending (see [`STALLS_PER_SONG`]).
    stalls: HashMap<(i64, String), u32>,
    /// Sends whose songs can't be saved here: not tried again until the phone is plugged in again (or a look is
    /// asked for), as each try would fail the same way.
    held: HashSet<i64>,
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
    /// Copies `file` to `to`, on the disk once it returns; fails with [`STALLED`] when the phone stopped sending, and
    /// with [`HERE`] first when the copy could not be written here.
    fn copy(&self, file: &PhoneFile, to: &Path, watch: &Watch) -> Result<u64, String>;
    /// Deletes one file, by its object.
    fn remove_file(&self, file: &PhoneFile) -> Result<(), String>;
    /// Deletes the folder when it is empty (fails otherwise, deleting nothing).
    fn remove_if_empty(&self) -> Result<(), String>;
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
    fn remove_if_empty(&self) -> Result<(), String> {
        stash_core::phone::Transfer::remove_if_empty(self)
    }
}

/// The phone a transfer is taken from: its name, opened again (after a read was called off, or to see what is in its
/// folder now), and whether Windows still has it.
trait Plugged {
    fn name(&self) -> &str;
    /// Its transfer folder opened again, as it is now (with its list or not), when it answers and has one.
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
        let transfer = stash_core::phone::open_transfer(&self.0.id).ok().flatten()?;
        Some(Box::new(transfer))
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

/// The library's commits made durable ([`Library::durable_commits`]) for as long as this lives, and as usual again
/// once it goes, however a take ends: a song is recorded here, and its like, before the phone deletes its copy (then
/// it never sends it again), so a power cut just after cannot take the record back.
struct Durable<'a>(&'a Mutex<Library>);

impl<'a> Durable<'a> {
    fn new(library: &'a Mutex<Library>) -> Self {
        let _ = library.lock().unwrap().durable_commits(true);
        Durable(library)
    }
}

impl Drop for Durable<'_> {
    fn drop(&mut self) {
        let library = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = library.durable_commits(false);
    }
}

/// The waits before each try again at a file another program holds.
#[cfg(not(test))]
const HELD_AGAIN_AFTER: [Duration; 3] = [Duration::from_millis(250); 3];
#[cfg(test)]
const HELD_AGAIN_AFTER: [Duration; 3] = [Duration::from_millis(1); 3];

/// Whether another program has the file open (Windows' sharing and lock violations): an antivirus or a sync client
/// looking at a file just written, which lets it go in a moment.
fn held_by_another(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33))
}

/// `act` on a file, tried again a few times a moment apart while another program holds it.
fn while_held<T>(mut act: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut waits = HELD_AGAIN_AFTER.iter();
    loop {
        match act() {
            Err(e) if held_by_another(&e) => match waits.next() {
                Some(wait) => std::thread::sleep(*wait),
                None => return Err(e),
            },
            done => return done,
        }
    }
}

/// Trouble putting a song's file in place here: this computer's ([`HERE`]: a full disk, a drive gone), but for a
/// file another program still holds, which is that song's alone (it fails, and the rest come in).
fn placing(e: std::io::Error) -> String {
    if held_by_another(&e) {
        format!("another program has the file open: {e}")
    } else {
        format!("{HERE}{e}")
    }
}

/// Trouble in the library here (its disk full, say): this computer's, as trouble keeping a file is.
fn library_trouble(e: stash_core::Error) -> String {
    format!("{HERE}Stash's library can't keep them ({e})")
}

/// Takes one song: its row, its file put where Stash saves songs (on the disk before it is recorded), its download
/// recorded. A song the library has as a file already is not read from the phone; any other is read once, whole, and
/// what kind of file it is is told from the copy here. Trouble keeping the file or the song here starts with [`HERE`].
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
            .map_err(library_trouble)?
            .first()
            .ok_or("the song was not kept")?;
        if sent.liked {
            library.set_liked(id, true, now_ms()).map_err(library_trouble)?;
        }
        let wanted = library.wants_file(id).map_err(library_trouble)?;
        (id, wanted, crate::saving::folder(&library))
    };
    if !wanted {
        return Ok(Took::Had);
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("{HERE}{e}"))?;
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
        let arriving = library.download_arriving(id, now_ms());
        if !matches!(arriving, Ok(true)) {
            drop(library);
            let _ = std::fs::remove_file(&part);
            return arriving.map(|_| Took::Had).map_err(library_trouble);
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
        .and_then(|_| while_held(|| std::fs::rename(&part, &target)));
    if let Err(e) = placed {
        return Err(failed(placing(e)));
    }
    // On the disk under its name before it is recorded and the phone's copy deleted: a power cut after that keeps it.
    // (Windows flushes a file only when it is open for writing.)
    let synced = while_held(|| std::fs::OpenOptions::new().write(true).open(&target))
        .and_then(|f| f.sync_all());
    if let Err(e) = synced {
        crate::saving::remove_saved(&target, &root);
        return Err(failed(placing(e)));
    }
    let read = downloads::read(&target).map_err(|e| {
        crate::saving::remove_saved(&target, &root);
        failed(match e {
            // The file gone from here as it was read back: this computer's trouble. A file the library cannot read
            // is the song's own.
            downloads::Trouble::Held(why, _) => format!("{HERE}{why}"),
            e => format!("{e:?}"),
        })
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
        .map_err(library_trouble);
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
fn take(ctx: &Ctx, phone: &dyn Plugged, folder: Box<dyn Folder>, tries: &mut Tries, stop: &AtomicBool) {
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
    let tried = tries.tried.get(&list.sent_at_ms).copied().unwrap_or(0);
    if taken_before.as_deref() == Some(list.sent_at_ms.to_string().as_str()) {
        // Taken whole by an earlier Stash, which could not delete it from the phone: its list goes now (tried as
        // often as a transfer is), so the phone knows this computer is done.
        if tried < TRIES {
            tries.tried.insert(list.sent_at_ms, tried + 1);
            match folder.remove_file(&list_file) {
                Ok(()) => (ctx.log)("the list of a transfer taken before went from the phone"),
                Err(e) => (ctx.log)(&format!("the list of a transfer taken before stayed on the phone: {e}")),
            }
        }
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
    // Chosen from, or taken whole: this phone has no send to choose from now (another phone's can be offered).
    {
        let mut offered = ctx.board.offer.lock().unwrap();
        if offered.as_ref().is_some_and(|o| o.phone == phone.name()) {
            *offered = None;
        }
    }
    if tried >= TRIES || tries.held.contains(&list.sent_at_ms) {
        return;
    }
    tries.tried.insert(list.sent_at_ms, tried + 1);
    let last_try = tried + 1 >= TRIES;
    let now = take_songs(ctx, phone, folder, &list_file, &list, &choice, &mut tries.stalls, last_try, stop);
    if now.removed + now.gave_up > 0 {
        tries.tried.remove(&list.sent_at_ms);
    } else if now.state == "cannot_save" {
        // Not the phone's doing: no try used up.
        if tried == 0 {
            tries.tried.remove(&list.sent_at_ms);
        } else {
            tries.tried.insert(list.sent_at_ms, tried);
        }
    }
    if now.state == "cannot_save" {
        tries.held.insert(list.sent_at_ms);
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
        sent_at_ms: Some(offer.sent_at_ms),
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

/// What came of opening the phone's transfer folder again after a read was called off.
enum Again {
    /// It answered, with a list in its folder.
    Open(Box<dyn Folder>),
    /// It answered, but with no list in its folder: the phone stopped the send.
    NoList,
    /// It did not answer (it is then to be unplugged, or its File transfer turned off and on).
    NoAnswer,
}

/// The phone's transfer folder opened again after a read was called off, tried a few times a little apart, until it
/// answers with a list.
fn open_again(phone: &dyn Plugged, stop: &AtomicBool) -> Again {
    let mut answered = false;
    for wait in AGAIN_AFTER {
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if stop.load(Ordering::Relaxed) {
                return Again::NoAnswer;
            }
            std::thread::sleep(Duration::from_millis(100).min(wait));
        }
        match phone.open_again() {
            Some(folder) if folder.files().iter().any(|f| f.name == stash_core::phone::LIST) => {
                return Again::Open(folder);
            }
            Some(_) => answered = true,
            None => {}
        }
    }
    if answered { Again::NoList } else { Again::NoAnswer }
}

/// What a folder holds of the send made at `sent_at_ms`.
enum Listed {
    /// Its list (read again: the same object can be another send once the phone numbered its files again).
    Same(PhoneFile),
    /// Another send's list: the phone sent again.
    Other,
    /// No list: the phone stopped the send.
    Gone,
    /// A list that did not answer when read.
    Unread,
}

/// What `folder` holds of the send made at `sent_at_ms`: its list is read again whatever its object.
fn listed(folder: &dyn Folder, sent_at_ms: i64) -> Listed {
    let Some(file) = folder
        .files()
        .into_iter()
        .find(|f| f.name == stash_core::phone::LIST)
    else {
        return Listed::Gone;
    };
    match folder.bytes(&file, LIST_CAP) {
        Err(_) => Listed::Unread,
        Ok(bytes)
            if serde_json::from_slice::<List>(&bytes).is_ok_and(|l| l.sent_at_ms == sent_at_ms) =>
        {
            Listed::Same(file)
        }
        Ok(_) => Listed::Other,
    }
}

/// What the phone's folder, opened again now, holds of the send made at `sent_at_ms`, when the phone is still plugged
/// in and answers: a read that fails because the phone stopped the send (its copies and list deleted as they are
/// read) or sent again is not the phone lost.
fn looked_again(phone: &dyn Plugged, sent_at_ms: i64) -> Option<Listed> {
    if !phone.still_there() {
        return None;
    }
    let folder = phone.open_again()?;
    Some(listed(folder.as_ref(), sent_at_ms))
}

/// Takes the songs of `list` chosen and still on the phone, one at a time, each deleted from the phone once taken. A
/// song that stops coming has its read called off: the phone is opened again and the song taken again from its start
/// (a song that has stopped [`STALLS_PER_SONG`] times, on this try or those before, is left, and the rest go on, read
/// from the phone opened again). With every chosen song taken, the take ends ([`end_of_take`]): the phone then knows
/// the computer is done. Songs that can't be saved here stop it at once ("cannot_save"); the phone stopping the send
/// stops it too ("withdrawn"). A try that carries on a send said before counts on from what was said; one that only
/// finishes it says nothing new ([`finish`]). `last_try`: another try comes only if this one takes songs off the
/// phone. What the transfer came to.
#[allow(clippy::too_many_arguments)]
fn take_songs(
    ctx: &Ctx,
    phone: &dyn Plugged,
    folder: Box<dyn Folder>,
    list_file: &PhoneFile,
    list: &List,
    choice: &Choice,
    stalls: &mut HashMap<(i64, String), u32>,
    last_try: bool,
    stop: &AtomicBool,
) -> Now {
    // What is recorded here is on the disk before the phone's copies go.
    let _durable = Durable::new(ctx.library);
    let index = |folder: &dyn Folder| -> HashMap<String, PhoneFile> {
        folder
            .files()
            .into_iter()
            .map(|f| (f.name.clone(), f))
            .collect()
    };
    let mut folder = Some(folder);
    let mut files = index(folder.as_deref().unwrap());
    // The list being taken, by its object on the phone: what goes at the end.
    let mut list_file = list_file.clone();
    // Chosen songs whose file is gone were taken by an earlier try (each file is deleted from the phone once taken).
    let wanted: Vec<&Sent> = list.songs.iter().filter(|s| choice.includes(&s.file)).collect();
    let total = wanted.len();
    let here: Vec<&Sent> = wanted
        .iter()
        .copied()
        .filter(|s| files.contains_key(&s.file))
        .collect();
    // What was said of this send by the tries before: this one carries it on.
    let earlier = ctx
        .board
        .now
        .lock()
        .unwrap()
        .clone()
        .filter(|n| n.is_of(phone.name(), list.sent_at_ms));
    if let Some(earlier) = &earlier
        && let Some(finished) = finish(
            ctx,
            phone,
            folder.as_deref().unwrap(),
            list,
            &list_file,
            choice,
            &here,
            &mut files,
            earlier,
        )
    {
        return finished;
    }
    // As first listed: each of `here` stays on the phone until this takes it.
    let listed_first = files.clone();
    let mut now = Now {
        state: "taking",
        phone: Some(phone.name().to_string()),
        total,
        done: total - here.len(),
        bytes_total: here.iter().map(|s| files[&s.file].size).sum(),
        at_ms: now_ms(),
        sent_at_ms: Some(list.sent_at_ms),
        ..Default::default()
    };
    if let Some(earlier) = &earlier {
        // What came in before counts with what comes in now.
        now.taken = earlier.taken;
        now.had = earlier.had;
        now.not_kept = earlier.not_kept;
        now.undeleted = earlier.undeleted.clone();
        now.before = Some(Before {
            state: earlier.state,
            at_ms: earlier.at_ms,
            brought: earlier.brought(),
        });
    }
    ctx.board.set(now.clone());
    (ctx.log)(&format!(
        "taking {} songs, {} MB ({} taken before)",
        here.len(),
        now.bytes_total / 1_000_000,
        now.done
    ));
    let started = Instant::now();
    let summary = |now: &Now| {
        (ctx.log)(&format!(
            "{} taken, {} here already, {} not kept (WebM), {} not taken, {} left on the phone (not chosen), in {:.0} s",
            now.taken,
            now.had,
            now.not_kept,
            now.failed,
            now.left,
            started.elapsed().as_secs_f64()
        ))
    };
    let (mut i, mut in_a_row) = (0, 0);
    while i < here.len() {
        if stop.load(Ordering::Relaxed) {
            return now;
        }
        let sent = here[i];
        let Some(file) = files.get(&sent.file).cloned() else {
            // Not listed since the phone was opened again (a listing can leave a file out), and only this takes songs
            // off the phone: not taken, and left there for another try.
            (ctx.log)(&format!(
                "{} was not listed when the phone was opened again: left for another try",
                sent.file
            ));
            now.failed += 1;
            now.unlisted += 1;
            continue_failed(ctx, &mut now, &listed_first[&sent.file]);
            i += 1;
            continue;
        };
        let current = folder.as_deref().expect("a folder while taking");
        if now.undeleted.contains(&sent.file) {
            // Counted in by a try before, which could not delete its copy: only the copy goes now.
            if current.remove_file(&file).is_ok() {
                files.remove(&sent.file);
                now.undeleted.retain(|f| f != &sent.file);
                now.removed += 1;
            }
            now.done += 1;
            now.bytes_done += file.size;
            i += 1;
            ctx.board.set(now.clone());
            continue;
        }
        let key = (list.sent_at_ms, sent.file.clone());
        if stalls.get(&key).is_some_and(|n| *n > STALLS_PER_SONG) {
            // It stopped every time it was read before: left on the phone, and the rest go on.
            now.failed += 1;
            continue_failed(ctx, &mut now, &file);
            i += 1;
            continue;
        }
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
        let at = Instant::now();
        match take_one(ctx, current, &file, sent, &watch) {
            Ok(took) => {
                match took {
                    Took::Taken => now.taken += 1,
                    Took::Had => now.had += 1,
                    Took::NotKept => now.not_kept += 1,
                }
                // Off the phone: what is left there is what is still to come (the phone counts it). A copy that stays
                // is deleted again before the list goes, and not counted again.
                if current.remove_file(&file).is_ok() {
                    files.remove(&sent.file);
                    now.removed += 1;
                } else {
                    now.undeleted.push(sent.file.clone());
                }
                in_a_row = 0;
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
                let times = stalls.entry(key).or_insert(0);
                *times += 1;
                let times = *times;
                (ctx.log)(&format!(
                    "{} ({} MB) stopped sending after {:.0} s (time {times}); opening the phone again",
                    sent.file,
                    size / 1_000_000,
                    at.elapsed().as_secs_f64()
                ));
                now.state = "stalled";
                now.bytes_done = base;
                ctx.board.set(now.clone());
                if times > STALLS_PER_SONG {
                    (ctx.log)(&format!("{} left on the phone: it stops every time", sent.file));
                    now.failed += 1;
                    now.gave_up += 1;
                    continue_failed(ctx, &mut now, &file);
                    i += 1;
                }
                // The phone closed before it is opened again: its File transfer stopped, so no song (this one again,
                // or the next) is read from it as it is.
                drop(folder.take());
                match open_again(phone, stop) {
                    Again::Open(again) => match listed(again.as_ref(), list.sent_at_ms) {
                        Listed::Same(l) => {
                            (ctx.log)("the phone answered again");
                            files = index(again.as_ref());
                            list_file = l;
                            folder = Some(again);
                        }
                        // Another list in place of the one read (whatever its object): the phone sent again meanwhile.
                        // This take ends with nothing removed, and the next look reads that send.
                        Listed::Other => {
                            (ctx.log)(&format!(
                                "the phone sent again after {} songs: what came in stays; the new send is next",
                                now.done
                            ));
                            return ended(ctx, now, "done");
                        }
                        Listed::Gone => {
                            (ctx.log)(&format!("the phone stopped the send after {} songs", now.done));
                            return ended(ctx, now, "withdrawn");
                        }
                        Listed::Unread => {
                            (ctx.log)(&format!(
                                "the phone did not answer again after {} songs: its list could not be read",
                                now.done
                            ));
                            return ended(ctx, now, "stopped");
                        }
                    },
                    Again::NoList => {
                        (ctx.log)(&format!("the phone stopped the send after {} songs", now.done));
                        return ended(ctx, now, "withdrawn");
                    }
                    Again::NoAnswer => {
                        (ctx.log)(&format!(
                            "the phone did not answer again after {} songs",
                            now.done
                        ));
                        return ended(ctx, now, "stopped");
                    }
                }
            }
            // This computer's trouble (its disk full, the downloads drive gone), not the phone's: every song would
            // fail the same way.
            Err(why) if why.starts_with(HERE) => return cannot_save(ctx, now, &why),
            Err(why) => {
                // The phone stopped the send as it was taken (Cancel on the phone deletes its copies and list), or
                // sent again: not the phone lost.
                if !lost(&why) {
                    match looked_again(phone, list.sent_at_ms) {
                        Some(Listed::Gone) => {
                            (ctx.log)(&format!(
                                "the phone stopped the send after {} songs: {why}",
                                now.done
                            ));
                            return ended(ctx, now, "withdrawn");
                        }
                        Some(Listed::Other) => {
                            (ctx.log)(&format!(
                                "the phone sent again after {} songs: what came in stays; the new send is next",
                                now.done
                            ));
                            return ended(ctx, now, "done");
                        }
                        _ => {}
                    }
                }
                // The phone gone (unplugged, File transfer turned off, its USB connection dropped: Windows can list
                // a phone for a while after it stops answering): stop, and say so; plugged in again, the rest is
                // taken. Songs failing one after another are taken as the phone gone too.
                in_a_row += 1;
                let gone = lost(&why) || in_a_row >= LOST_AFTER || !phone.still_there();
                if gone {
                    (ctx.log)(&format!("lost the phone after {} songs: {why}", now.done));
                    return ended(ctx, now, "stopped");
                }
                now.failed += 1;
                (ctx.log)(&format!("{} was not taken: {why}", sent.file));
                continue_failed(ctx, &mut now, &file);
                i += 1;
            }
        }
    }
    if now.failed == 0 {
        let current = folder.as_deref().expect("a folder while taking");
        return match end_of_take(ctx, phone, current, list, &list_file, choice, &mut files, &mut now) {
            Ok(state) => {
                summary(&now);
                ended(ctx, now, state)
            }
            Err(why) => cannot_save(ctx, now, &why),
        };
    }
    // Everything not taken stays on the phone, to be tried again: by the next look, with no need to send again, when
    // the phone's listing only left it out.
    now.trying_again = now.failed == now.unlisted && (now.removed > 0 || !last_try);
    summary(&now);
    ended(ctx, now, "done")
}

/// The end of a take with every song chosen in. The copies of songs counted in that stayed on the phone are deleted
/// once more (one left there once the list goes reads on the phone as a song not chosen; one that stays even so is
/// left for the phone to clear), those of songs not chosen that are here already go too ([`drop_here_already`]), then
/// the list read (`list_file`), by its object: never the folder with what is in it, which can hold songs this computer
/// has not seen (a listing can leave one out; the phone may have sent again since). The folder goes too once nothing
/// is left in it (only if it is empty). How the take ends: done, or stopped when the phone went as its list was
/// removed, either way with `list_left` when the list stayed (the choice is kept, and the next look removes it
/// again); trouble in the library here, as [`HERE`].
#[allow(clippy::too_many_arguments)]
fn end_of_take(
    ctx: &Ctx,
    phone: &dyn Plugged,
    folder: &dyn Folder,
    list: &List,
    list_file: &PhoneFile,
    choice: &Choice,
    files: &mut HashMap<String, PhoneFile>,
    now: &mut Now,
) -> Result<&'static str, String> {
    for name in std::mem::take(&mut now.undeleted) {
        let Some(file) = files.get(&name) else {
            continue;
        };
        match folder.remove_file(file) {
            Ok(()) => {
                files.remove(&name);
                now.removed += 1;
            }
            Err(e) => {
                (ctx.log)(&format!("{name} came in, but its copy stayed on the phone: {e}"));
                now.undeleted.push(name);
            }
        }
    }
    // Before the list goes: the phone counts a copy gone while it is there as on this computer.
    let unchosen: Vec<&Sent> = list
        .songs
        .iter()
        .filter(|s| !choice.includes(&s.file) && files.contains_key(&s.file))
        .collect();
    let dropped = drop_here_already(ctx, folder, &unchosen, files)?;
    now.had += dropped;
    now.removed += dropped;
    now.left = list
        .songs
        .iter()
        .filter(|s| !choice.includes(&s.file) && files.contains_key(&s.file))
        .count();
    match folder.remove_file(list_file) {
        Ok(()) => {
            files.remove(stash_core::phone::LIST);
            now.list_left = false;
            if now.left == 0 {
                now.cleared = true;
                // Empty as far as this computer saw: the folder goes too, only if it is (what else is in it stays).
                if files.is_empty() {
                    let _ = folder.remove_if_empty();
                }
            }
            ctx.board.chosen_taken(ctx.library, list.sent_at_ms);
            Ok("done")
        }
        Err(e) => {
            // Still chosen: the next look finds the chosen songs in and removes the list again.
            (ctx.log)(&format!("the list stayed on the phone, removed again at the next look: {e}"));
            now.list_left = true;
            if lost(&e) || !phone.still_there() {
                (ctx.log)(&format!("lost the phone after {} songs: {e}", now.done));
                return Ok("stopped");
            }
            Ok("done")
        }
    }
}

/// A try that only finishes a take of this send that ended before (`earlier`): every song chosen is in, but for copies
/// of songs counted in that stayed on the phone. Those go (tried once more), the copies of songs not chosen that are
/// here already too, then the list ([`end_of_take`]). Nothing new is said: what was said stays, its counts and its time
/// (a card closed on it stays closed), but for what is left on the phone and whether the list went (and so done, once
/// it went). None when a song chosen is still to be taken.
#[allow(clippy::too_many_arguments)]
fn finish(
    ctx: &Ctx,
    phone: &dyn Plugged,
    folder: &dyn Folder,
    list: &List,
    list_file: &PhoneFile,
    choice: &Choice,
    here: &[&Sent],
    files: &mut HashMap<String, PhoneFile>,
    earlier: &Now,
) -> Option<Now> {
    let ended_before = matches!(earlier.state, "done" | "stopped" | "cannot_save" | "withdrawn");
    if !ended_before || !here.iter().all(|s| earlier.undeleted.contains(&s.file)) {
        return None;
    }
    let mut now = Now {
        removed: 0,
        gave_up: 0,
        unlisted: 0,
        before: None,
        ..earlier.clone()
    };
    match end_of_take(ctx, phone, folder, list, list_file, choice, files, &mut now) {
        Ok(state) => {
            (ctx.log)(if now.list_left {
                "a transfer taken before: its list stayed on the phone again"
            } else {
                "a transfer taken before is finished: its list went from the phone"
            });
            now.state = state;
            now.error = None;
            now.folder = None;
            let mut said = ctx.board.now.lock().unwrap();
            if said.as_ref().is_some_and(|n| n.is_of(phone.name(), list.sent_at_ms)) {
                *said = Some(now.clone());
            }
        }
        Err(why) => return Some(cannot_save(ctx, now, &why)),
    }
    (ctx.edited)();
    Some(now)
}

/// Ends a take whose songs can't be saved here (`why`, after [`HERE`]): this computer's trouble (its disk full, the
/// downloads drive gone, the library failing), not the phone's, and every song would fail the same way. Said, with
/// where the songs go.
fn cannot_save(ctx: &Ctx, mut now: Now, why: &str) -> Now {
    let reason = why.strip_prefix(HERE).unwrap_or(why).to_string();
    let dir = crate::saving::folder(&ctx.library.lock().unwrap());
    (ctx.log)(&format!(
        "songs can't be saved in {} after {} songs: {reason}",
        dir.display(),
        now.done
    ));
    now.error = Some(reason);
    now.folder = Some(dir.to_string_lossy().into_owned());
    ended(ctx, now, "cannot_save")
}

/// Ends a take as `state`: said, and the library pages told. A try that carries on a send, brings nothing more in, and
/// ends as the one before did is the same news at the same time (a card closed on it stays closed).
fn ended(ctx: &Ctx, mut now: Now, state: &'static str) -> Now {
    now.state = state;
    now.song = None;
    now.at_ms = match now.before {
        Some(before) if before.state == state && before.brought == now.brought() => before.at_ms,
        _ => now_ms(),
    };
    ctx.board.set(now.clone());
    (ctx.edited)();
    now
}

/// Deletes from the phone the copies of `songs` (not chosen, and on the phone) that this computer has already, as each
/// song taken is: the phone sees them go and sends them no more, so a send carries only songs this computer has not
/// got. One liked on the phone is liked here first, as a song taken is. How many went; trouble in the library here, as
/// [`HERE`] (nothing is deleted then: a like would go with its copy).
fn drop_here_already(
    ctx: &Ctx,
    folder: &dyn Folder,
    songs: &[&Sent],
    files: &mut HashMap<String, PhoneFile>,
) -> Result<usize, String> {
    if songs.is_empty() {
        return Ok(0);
    }
    let asked: Vec<import::Song> = songs.iter().map(|s| as_song(s)).collect();
    let here = {
        let mut library = ctx.library.lock().unwrap();
        let here = library.songs_here(&asked).map_err(library_trouble)?;
        // The phone sends a song no more once its copy goes, so its like comes now (found as taking it finds it).
        for ((sent, &here), song) in songs.iter().zip(&here).zip(&asked) {
            if here && sent.liked {
                let ids = library
                    .adopt_songs(std::slice::from_ref(song))
                    .map_err(library_trouble)?;
                if let Some(&id) = ids.first() {
                    library.set_liked(id, true, now_ms()).map_err(library_trouble)?;
                }
            }
        }
        here
    };
    let mut gone = 0;
    for (sent, here) in songs.iter().zip(here) {
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
    Ok(gone)
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
        let here = |e: std::io::Error| format!("{HERE}{e}");
        let copied = (|| -> Result<u64, String> {
            let mut from = std::fs::File::open(&file.object).map_err(|e| e.to_string())?;
            let mut into = std::fs::File::create(to).map_err(here)?;
            let (mut block, mut n) = (vec![0u8; 1 << 20], 0u64);
            loop {
                let got = std::io::Read::read(&mut from, &mut block).map_err(|e| e.to_string())?;
                if got == 0 {
                    into.sync_all().map_err(here)?;
                    return Ok(n);
                }
                into.write_all(&block[..got]).map_err(here)?;
                n += got as u64;
                (watch.progress)(n);
            }
        })();
        copied.inspect_err(|_| {
            let _ = std::fs::remove_file(to);
        })
    }
    fn remove_file(&self, file: &PhoneFile) -> Result<(), String> {
        std::fs::remove_file(&file.object).map_err(|e| e.to_string())
    }
    fn remove_if_empty(&self) -> Result<(), String> {
        std::fs::remove_dir(&self.0).map_err(|e| e.to_string())
    }
}

impl Plugged for LocalFolder {
    fn name(&self) -> &str {
        "Test folder"
    }
    fn open_again(&self) -> Option<Box<dyn Folder>> {
        self.0
            .is_dir()
            .then(|| Box::new(LocalFolder(self.0.clone())) as Box<dyn Folder>)
    }
    fn still_there(&self) -> bool {
        self.0.is_dir()
    }
}

/// [`run`] with a folder for a phone (`STASH_PHONE_FOLDER`).
fn run_folder(state: &State, stop: &AtomicBool, dir: std::path::PathBuf) {
    let phone = LocalFolder(dir);
    let mut tries = Tries::default();
    while !stop.load(Ordering::Relaxed) {
        let asked = ASKED.load(Ordering::Relaxed);
        if asked > ANSWERED.load(Ordering::Relaxed) {
            tries = Tries::default();
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
            take(&ctx, &phone, Box::new(LocalFolder(phone.0.clone())), &mut tries, stop);
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
    let mut tries = Tries::default();
    // For each phone, the songs in its folder while it has no list, and when they last grew in number.
    let mut grew: HashMap<String, (usize, Option<Instant>)> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        let asked = ASKED.load(Ordering::Relaxed);
        if asked > ANSWERED.load(Ordering::Relaxed) {
            looked.clear();
            tries = Tries::default();
        }
        if let Ok(phones) = stash_core::phone::phones() {
            phones_now(&BOARD, &mut tries, looked.keys(), &phones);
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
                        nothing_sent(&BOARD, &phone.name, transfer.is_some(), copying, ready);
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
                        take(&ctx, &Usb(&phone), Box::new(transfer), &mut tries, stop);
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
/// send to choose from or one being taken; either way, this phone's send is no longer offered (it has none now).
#[cfg(any(windows, test))]
fn tell(board: &Board, phone: &str, what: &'static str, ready: usize) {
    told(&mut board.now.lock().unwrap(), phone, what, ready);
    let mut offer = board.offer.lock().unwrap();
    if offer.as_ref().is_some_and(|o| o.phone == phone) {
        *offer = None;
    }
}

/// What a look makes of the phones listed now (`phones`), with those looked at before (`before`, by id), before it
/// looks inside them. One unplugged since: plugged in again, what it sent is tried again, however often it was before
/// (a song that stopped sending too often is still left: it would stop again). One unplugged before anything was
/// taken has nothing more to say, and nothing to choose from; one unplugged while taken, a transfer cut short
/// ([`unlisted`]).
fn phones_now<'a>(
    board: &Board,
    tries: &mut Tries,
    before: impl IntoIterator<Item = &'a String>,
    phones: &[stash_core::phone::Phone],
) {
    if before.into_iter().any(|id| !phones.iter().any(|p| &p.id == id)) {
        tries.tried.clear();
        tries.held.clear();
    }
    unlisted(&mut board.now.lock().unwrap(), &|name| {
        phones.iter().any(|p| p.name == name)
    });
    let mut offer = board.offer.lock().unwrap();
    if offer.as_ref().is_some_and(|o| !phones.iter().any(|p| p.name == o.phone)) {
        *offer = None;
    }
}

/// A look inside a phone with no list to take: its transfer folder `opened` (it answers, with no list in it) or not
/// (it does not answer, or has no such folder), with `ready` songs in it, still coming in while `copying`. A transfer
/// said to be taken from it was cut short: by the phone, which stopped the send (its folder answers, with no list), or
/// by the phone not answering ([`cut_short`]). A list said to have stayed on it is gone: there is nothing left for
/// this computer to finish. Then it is said to be connected, or copying ([`tell`]).
#[cfg(any(windows, test))]
fn nothing_sent(board: &Board, phone: &str, opened: bool, copying: bool, ready: usize) {
    if let Some(n) = board
        .now
        .lock()
        .unwrap()
        .as_mut()
        .filter(|n| n.phone.as_deref() == Some(phone))
    {
        if opened && matches!(n.state, "taking" | "stalled") {
            n.state = "withdrawn";
            n.song = None;
            n.at_ms = now_ms();
        } else {
            cut_short(n);
        }
        if opened && n.list_left {
            n.list_left = false;
            n.cleared = n.left == 0;
            if n.state == "stopped" {
                n.state = "done";
            }
        }
    }
    tell(
        board,
        phone,
        if copying { "preparing" } else { "connected" },
        if copying { ready } else { 0 },
    );
}

/// What the window was told, with the phones listed now: a phone no longer listed has nothing more to say before
/// anything was taken, and a transfer from it said to be taken was cut short; one unplugged before this computer was
/// done with it (its list stayed on the phone) is to be plugged in again.
fn unlisted(now: &mut Option<Now>, listed: &dyn Fn(&str) -> bool) {
    let Some(n) = now.as_mut() else {
        return;
    };
    if n.phone.as_deref().is_some_and(listed) {
        return;
    }
    if matches!(n.state, "connected" | "preparing" | "ready") {
        *now = None;
    } else if n.state == "done" && n.list_left {
        n.state = "stopped";
        n.at_ms = now_ms();
    } else {
        cut_short(n);
    }
}

/// A transfer said to be taken ("taking", "stalled") seen by the looks outside [`take`] (only they take songs, and a
/// take ends done, stopped, withdrawn or with songs that can't be saved): it was cut short before or as it was taken,
/// by the phone unplugged or not answering. Said as stopped, with what came in; taken again once the phone answers
/// with its list.
fn cut_short(n: &mut Now) {
    if matches!(n.state, "taking" | "stalled") {
        n.state = "stopped";
        n.song = None;
        n.at_ms = now_ms();
    }
}

/// [`tell`] on `now`: whether it was said.
fn told(now: &mut Option<Now>, phone: &str, what: &'static str, ready: usize) -> bool {
    let same = now.as_ref().filter(|n| n.phone.as_deref() == Some(phone));
    let other = now.as_ref().filter(|n| n.phone.as_deref() != Some(phone));
    if what == "connected"
        && (other.is_some()
            || same.is_some_and(|n| {
                matches!(n.state, "taking" | "stalled" | "done" | "stopped" | "withdrawn" | "cannot_save")
            }))
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

/// Takes from the send offered now what was chosen from it: how many songs that is. Refused when a song chosen is not
/// offered now (gone from the phone since the window read the songs), so the window reads them again rather than that
/// song quietly not coming in. The window is told at once that they are being taken.
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
            if files.iter().any(|f| !offer.songs.iter().any(|s| &s.file == f)) {
                return Err("the songs on the phone changed: look them over and take them again");
            }
            Choice::These(files)
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
        sent_at_ms: Some(sent_at_ms),
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
        /// Files left out of every listing once the phone was opened again (as a listing can leave one out).
        unlisted_again: Mutex<HashSet<String>>,
        /// A list the phone sends in place of its own when a song stops sending (sent again meanwhile).
        sends_again: Mutex<Option<Vec<u8>>>,
        /// How often the list was sent again: it is a new object each time.
        list_sent: AtomicU64,
        /// Files whose delete fails, and what it says.
        undeletable: Mutex<HashMap<String, String>>,
        /// Files whose delete fails this many more times.
        undeletable_times: Mutex<HashMap<String, u32>>,
        /// The folder itself was deleted.
        folder_gone: AtomicBool,
        /// A list sent again keeps the object of the one before (the phone numbered its files again).
        same_object: AtomicBool,
        /// The file as whose read the phone stops the send (Cancel on the phone): every file goes from its folder.
        withdraws_at: Mutex<Option<String>>,
        /// Files listed whose read fails.
        unreadable: Mutex<HashSet<String>>,
        /// Each copy written here is held open by another program (an antivirus looking at it), until let go.
        holds_copies: AtomicBool,
        held: Mutex<Vec<std::fs::File>>,
    }

    impl Phone {
        fn with(songs: &[(&str, Vec<u8>)]) -> Arc<Phone> {
            Phone::with_at(7, songs)
        }

        /// The same, sent at `sent_at_ms` (another send).
        fn with_at(sent_at_ms: i64, songs: &[(&str, Vec<u8>)]) -> Arc<Phone> {
            let entries = songs
                .iter()
                .map(|(n, _)| json!({ "file": n, "title": format!("Song {n}"), "artist": "Someone" }))
                .collect();
            Phone::sending("Test phone", sent_at_ms, songs, entries)
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

        /// A file's object on the phone: its name, or for a list sent again a new one.
        fn object_of(&self, name: &str) -> String {
            match self.list_sent.load(Ordering::Relaxed) {
                n if n > 0 && name == stash_core::phone::LIST && !self.same_object.load(Ordering::Relaxed) => {
                    format!("{name}#{n}")
                }
                _ => name.to_string(),
            }
        }
    }

    /// The phone opened: once a read on it stops sending, every later read on it stops too, until the phone is
    /// opened again (as a phone's File transfer does).
    struct Open(Arc<Phone>, AtomicBool);

    impl Open {
        fn of(phone: &Arc<Phone>) -> Box<dyn Folder> {
            Box::new(Open(Arc::clone(phone), AtomicBool::new(false)))
        }
    }

    impl Folder for Open {
        fn files(&self) -> Vec<PhoneFile> {
            let again = self.0.opened_again.load(Ordering::Relaxed) > 0;
            let unlisted = self.0.unlisted_again.lock().unwrap();
            self.0
                .files
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, _)| !(again && unlisted.contains(n)))
                .map(|(n, b)| PhoneFile {
                    object: self.0.object_of(n),
                    name: n.clone(),
                    size: b.len() as u64,
                })
                .collect()
        }
        fn bytes(&self, file: &PhoneFile, _cap: u64) -> Result<Vec<u8>, String> {
            if self.0.unreachable.load(Ordering::Relaxed) {
                return Err("the device is unreachable (0x80070141)".to_string());
            }
            if self.1.load(Ordering::Relaxed) {
                return Err("the phone did not answer (0x80004005)".to_string());
            }
            let files = self.0.files.lock().unwrap();
            files
                .iter()
                .find(|(n, _)| *n == file.name)
                .map(|(_, b)| b.clone())
                .ok_or_else(|| "the phone did not answer (0x80070490)".to_string())
        }
        fn copy(&self, file: &PhoneFile, to: &Path, watch: &Watch) -> Result<u64, String> {
            self.0.reads.lock().unwrap().push(file.name.clone());
            if self.0.withdraws_at.lock().unwrap().take_if(|at| *at == file.name).is_some() {
                self.0.files.lock().unwrap().clear();
            }
            if self.0.unreadable.lock().unwrap().contains(&file.name) {
                return Err("the phone did not answer (0x80070490)".to_string());
            }
            let stops = match self.0.stalls.lock().unwrap().get_mut(&file.name) {
                Some(left) if *left > 0 => {
                    *left -= 1;
                    true
                }
                _ => false,
            };
            if stops || self.1.load(Ordering::Relaxed) {
                if let Some(list) = self.0.sends_again.lock().unwrap().take() {
                    let mut files = self.0.files.lock().unwrap();
                    if let Some(old) = files.iter_mut().find(|(n, _)| n == stash_core::phone::LIST) {
                        old.1 = list;
                    }
                    self.0.list_sent.fetch_add(1, Ordering::Relaxed);
                }
                self.1.store(true, Ordering::Relaxed);
                (watch.stalled)();
                return Err(STALLED.to_string());
            }
            let bytes = self.bytes(file, 0)?;
            std::fs::write(to, &bytes).map_err(|e| format!("{HERE}{e}"))?;
            #[cfg(windows)]
            if self.0.holds_copies.load(Ordering::Relaxed) {
                use std::os::windows::fs::OpenOptionsExt;
                let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(to).unwrap();
                self.0.held.lock().unwrap().push(held);
            }
            (watch.progress)(bytes.len() as u64);
            Ok(bytes.len() as u64)
        }
        fn remove_file(&self, file: &PhoneFile) -> Result<(), String> {
            if let Some(why) = self.0.undeletable.lock().unwrap().get(&file.name) {
                return Err(why.clone());
            }
            if let Some(times) = self.0.undeletable_times.lock().unwrap().get_mut(&file.name).filter(|n| **n > 0) {
                *times -= 1;
                return Err("the phone did not answer (0x80004005)".to_string());
            }
            let mut files = self.0.files.lock().unwrap();
            let before = files.len();
            files.retain(|(n, _)| self.0.object_of(n) != file.object);
            if files.len() == before {
                return Err("no such object on the phone".to_string());
            }
            Ok(())
        }
        fn remove_if_empty(&self) -> Result<(), String> {
            if !self.0.files.lock().unwrap().is_empty() {
                return Err("the folder is not empty".to_string());
            }
            self.0.folder_gone.store(true, Ordering::Relaxed);
            Ok(())
        }
    }

    impl Plugged for Arc<Phone> {
        fn name(&self) -> &str {
            &self.name
        }
        fn open_again(&self) -> Option<Box<dyn Folder>> {
            self.opened_again.fetch_add(1, Ordering::Relaxed);
            self.answers_again.load(Ordering::Relaxed).then(|| Open::of(self))
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

        fn take(&self, phone: &Arc<Phone>, tries: &mut Tries) -> Option<Now> {
            self.take_on(&self.board, phone, tries)
        }

        /// A look at `phone` with `board` (a fresh one: as after a restart).
        fn take_on(&self, board: &Board, phone: &Arc<Phone>, tries: &mut Tries) -> Option<Now> {
            let (edited, note) = (|| {}, |_: &str| {});
            let ctx = Ctx {
                library: &self.library,
                board,
                edited: &edited,
                log: &note,
            };
            take(&ctx, phone, Open::of(phone), tries, &AtomicBool::new(false));
            board.now.lock().unwrap().clone()
        }

        /// How many times the board has been said something.
        fn said(&self) -> usize {
            self.board.states.lock().unwrap().len()
        }

        /// What the window reads of the board.
        fn told(&self) -> serde_json::Value {
            json!(self.board.now.lock().unwrap().clone().unwrap())
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
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.done, now.total), ("done", 3, 0, 3, 3));
        assert!(now.cleared, "everything came in: the list is gone from the phone");
        assert!(phone.left().is_empty() && phone.folder_gone.load(Ordering::Relaxed), "and the folder, empty");
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
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.done), ("done", 2, 1, 3));
        assert!(!now.cleared && !now.trying_again, "a song left for stopping is not tried again by itself");
        // Left on the phone for another send: the song and the list.
        assert_eq!(phone.left(), ["2.wav", stash_core::phone::LIST]);
        assert_eq!(phone.reads.lock().unwrap().iter().filter(|r| *r == "2.wav").count(), 3);
        // 3.wav was read from the phone opened again after the third stop, not from the phone as it stopped.
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), 3);
        assert!(!tries.stalls.contains_key(&(7, "3.wav".to_string())));
    }

    #[test]
    fn a_phone_that_does_not_answer_again_stops_and_keeps_the_rest() {
        let here = Here::new("gone");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        phone.answers_again.store(false, Ordering::Relaxed);
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.done), ("stopped", 1, 1));
        assert_eq!(phone.left(), ["2.wav", "3.wav", stash_core::phone::LIST]);
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), AGAIN_AFTER.len() as u64);
    }

    #[test]
    fn a_song_here_already_is_not_read_from_the_phone() {
        let here = Here::new("had");
        let first = Phone::with(&[("1.wav", wav())]);
        assert_eq!(here.take(&first, &mut Tries::default()).unwrap().taken, 1);
        // The same song sent again (a new send of it).
        let again = Phone::with_at(9, &[("1.wav", wav())]);
        let now = here.take(&again, &mut Tries::default()).unwrap();
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
        let now = here.take(&phone, &mut Tries::default()).unwrap();
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
        let mut tries = Tries::default();
        for (i, name) in names.iter().enumerate().skip(1) {
            phone.stall(name, 1);
            let now = here.take(&phone, &mut tries).unwrap();
            assert_eq!((now.state, now.done), ("stopped", i), "plug-in {i} brought one more in");
        }
        assert!(tries.tried.is_empty(), "every try brought a song in: none counted");
        // Now one that stops every time, plug-in after plug-in: once it has stopped more than STALLS_PER_SONG times
        // it is left (that try got somewhere), and the tries after it bring nothing in: left after TRIES of them.
        phone.stall("5.wav", 99);
        let reads = || phone.reads.lock().unwrap().len();
        // It stopped once already, above.
        for _ in 0..STALLS_PER_SONG {
            here.take(&phone, &mut tries);
        }
        assert!(tries.tried.is_empty(), "the song was left: the try counted as one that got somewhere");
        let left_at = reads();
        for _ in 0..TRIES {
            assert_eq!(here.take(&phone, &mut tries).unwrap().failed, 1);
        }
        assert_eq!(reads(), left_at, "a song left is not read again");
        assert_eq!(tries.tried.get(&7), Some(&TRIES));
        let said = here.board.states.lock().unwrap().len();
        here.take(&phone, &mut tries);
        assert_eq!(here.board.states.lock().unwrap().len(), said, "left: not tried again");
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
        assert_eq!(here.take(&Phone::with(&[("1.wav", wav())]), &mut Tries::default()).unwrap().taken, 1);
        here.take_all(false);
        let phone = three("Test phone");
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.total, now.here), ("ready", 3, 1));
        assert_eq!(now.bytes_total, 3 * wav().len() as u64);
        assert!(phone.reads.lock().unwrap().is_empty(), "nothing read before it is chosen");
        assert_eq!(phone.left().len(), 4, "nothing deleted from the phone");
        assert!(tries.tried.is_empty(), "waiting is not a try");
        let offer = here.board.offer.lock().unwrap().clone().unwrap();
        assert_eq!((offer.phone.as_str(), offer.sent_at_ms), ("Test phone", 8));
        let first = &offer.songs[0];
        assert_eq!((first.here, first.format.as_deref(), first.bits, first.sample_rate), (true, Some("FLAC"), Some(24), Some(96000)));
        assert_eq!((first.album.as_deref(), first.liked, first.playlists.clone()), (Some("First"), true, vec!["Gym".to_string()]));
        assert_eq!(offer.songs[1].art_url, None, "only a safe cover is offered");
        assert!(!offer.songs[1].here && !offer.songs[2].here);
        // Looked at again: offered as it was, and said again when something else was said over it.
        *here.board.now.lock().unwrap() = None;
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "ready");
    }

    #[test]
    fn only_the_songs_chosen_are_taken_and_the_rest_left_for_the_phone_to_clear() {
        let here = Here::asking("chosen");
        let phone = three("Test phone");
        assert_eq!(here.take(&phone, &mut Tries::default()).unwrap().state, "ready");
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        let told = here.board.now.lock().unwrap().clone().unwrap();
        assert_eq!((told.state, told.total), ("taking", 1), "told at once");
        assert!(here.board.offer.lock().unwrap().is_none(), "no longer offered");
        let now = here.take(&phone, &mut Tries::default()).unwrap();
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
        assert_eq!(here.take(&Phone::with(&[("1.wav", wav())]), &mut Tries::default()).unwrap().taken, 1);
        here.take_all(false);
        let phone = three("Test phone");
        assert_eq!(here.take(&phone, &mut Tries::default()).unwrap().state, "ready");
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.had, now.left), ("done", 1, 1, 1));
        // 1.wav is here already: its copy went too, so the phone sends it no more. 3.wav is new, and was not chosen.
        assert_eq!(phone.left(), ["3.wav"]);
        assert_eq!(*phone.reads.lock().unwrap(), ["2.wav"], "nothing read for a song here already");
    }

    #[test]
    fn a_choice_carries_on_after_a_restart() {
        let here = Here::asking("restart");
        let phone = three("Test phone");
        here.take(&phone, &mut Tries::default());
        assert_eq!(here.choose(8, &["1.wav", "2.wav"]), Ok(2));
        phone.stall("2.wav", 1);
        phone.answers_again.store(false, Ordering::Relaxed);
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken), ("stopped", 1));
        // Stash starts again: what was chosen is kept in the library, and the rest of it comes in.
        phone.answers_again.store(true, Ordering::Relaxed);
        let fresh = Board::new();
        let now = here.take_on(&fresh, &phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.done, now.total, now.left), ("done", 1, 2, 2, 1));
        assert_eq!(phone.left(), ["3.wav"]);
        assert!(!phone.reads.lock().unwrap().iter().any(|r| r == "3.wav"), "3.wav was not chosen");
    }

    #[test]
    fn taking_nothing_leaves_every_song_and_removes_the_list() {
        let here = Here::asking("none");
        let phone = three("Test phone");
        here.take(&phone, &mut Tries::default());
        let (at, choice) = chosen(&json!({ "sent_at_ms": 8, "files": [], "none": true })).unwrap();
        assert_eq!(choose_from(&here.library, &here.board, at, choice), Ok(0));
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.left), ("done", 0, 3));
        assert!(phone.reads.lock().unwrap().is_empty());
        assert_eq!(phone.left(), ["1.wav", "2.wav", "3.wav"]);
    }

    #[test]
    fn only_songs_the_send_offered_can_be_chosen() {
        let here = Here::asking("only");
        let phone = three("Test phone");
        assert_eq!(here.choose(8, &["1.wav"]), Err("those songs are no longer waiting on the phone"), "not offered yet");
        here.take(&phone, &mut Tries::default());
        assert!(here.choose(9, &["1.wav"]).is_err(), "another send");
        // A song the send does not offer now (gone from the phone since the window read them, or never one of its
        // songs): refused, not quietly left out, and the send is still offered.
        let changed = Err("the songs on the phone changed: look them over and take them again");
        assert_eq!(here.choose(8, &["1.wav", "9.wav"]), changed);
        assert_eq!(here.choose(8, &["1.wav", "stash-transfer.json"]), changed);
        assert!(here.board.offer.lock().unwrap().is_some());
        assert_eq!(here.board.choice(&here.library, 8), None);
        assert_eq!(here.choose(8, &["1.wav"]), Ok(1));
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
        here.take(&a, &mut Tries::default());
        let now = here.take(&b, &mut Tries::default()).unwrap();
        assert_eq!(now.phone.as_deref(), Some("Phone A"), "Phone A's send is still the one shown");
        assert_eq!(here.board.offer.lock().unwrap().as_ref().unwrap().phone, "Phone A");
    }

    #[test]
    fn a_send_waiting_to_be_chosen_from_stays_offered_while_the_phone_does_not_answer() {
        let here = Here::asking("dropout");
        let phone = three("Test phone");
        let mut tries = Tries::default();
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "ready");
        phone.unreachable.store(true, Ordering::Relaxed);
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!(now.state, "ready", "nothing was being taken: not a transfer cut short");
        assert!(here.board.offer.lock().unwrap().is_some(), "still offered, with what was chosen from it");
        phone.unreachable.store(false, Ordering::Relaxed);
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "ready");
        // Chosen from, then not answering: that is a transfer cut short, which carries on once it answers.
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        phone.unreachable.store(true, Ordering::Relaxed);
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "stopped");
        phone.unreachable.store(false, Ordering::Relaxed);
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken), ("done", 1));
    }

    #[test]
    fn a_send_waiting_behind_another_phones_is_offered_once_that_one_is_taken() {
        let here = Here::asking("behind");
        let a = three("Phone A");
        let b = Phone::sending("Phone B", 9, &[("9.wav", wav())], vec![json!({ "file": "9.wav", "title": "B song", "artist": "Someone" })]);
        let mut tries = Tries::default();
        assert_eq!(here.take(&a, &mut tries).unwrap().phone.as_deref(), Some("Phone A"));
        here.take(&b, &mut tries);
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        // Windows lists B first: its send is kept to offer, but not said over A's being taken.
        let now = here.take(&b, &mut tries).unwrap();
        assert_eq!((now.state, now.phone.as_deref()), ("taking", Some("Phone A")));
        let done = here.take(&a, &mut tries).unwrap();
        assert_eq!((done.state, done.phone.as_deref(), done.taken), ("done", Some("Phone A"), 1));
        // Then B's send is said, over A's result.
        let now = here.take(&b, &mut tries).unwrap();
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

    /// A list for the send made at `sent_at_ms` of `files`.
    fn list_of(sent_at_ms: i64, files: &[&str]) -> Vec<u8> {
        let songs: Vec<serde_json::Value> = files
            .iter()
            .map(|n| json!({ "file": n, "title": format!("Song {n}"), "artist": "Someone" }))
            .collect();
        json!({ "stash_transfer": 1, "sent_at_ms": sent_at_ms, "songs": songs }).to_string().into_bytes()
    }

    #[test]
    fn songs_that_cannot_be_saved_here_stop_the_take_at_once_without_using_a_try() {
        let here = Here::new("nosave");
        // The downloads folder is somewhere that can't be written (a drive gone, a disk full).
        let blocker = here.dir.join("not a folder");
        std::fs::write(&blocker, b"").unwrap();
        let into = blocker.join("Stash");
        let set = |to: &Path| {
            here.library
                .lock()
                .unwrap()
                .set_setting(crate::saving::FOLDER, &to.to_string_lossy())
                .unwrap()
        };
        set(&into);
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("cannot_save", 0, 0));
        assert_eq!(now.folder.as_deref(), Some(into.to_string_lossy().as_ref()));
        assert!(now.error.as_deref().is_some_and(|e| !e.is_empty() && !e.starts_with(HERE)), "why, in plain words");
        assert!(tries.tried.is_empty(), "not the phone's doing: no try used up");
        assert_eq!(phone.left().len(), 4, "nothing deleted from the phone");
        // Looked at again: not tried again (it would fail the same way), and the card stays as it was.
        let said = here.said();
        assert_eq!(here.take(&phone, &mut tries).unwrap().at_ms, now.at_ms);
        assert_eq!(here.said(), said, "nothing said: not tried");
        assert!(phone.reads.lock().unwrap().is_empty());
        // Room again, and the phone plugged in again (the looks forget what they held): the songs come in.
        set(&here.dir);
        phones_now(&here.board, &mut tries, [&"phone".to_string()], &[]);
        assert!(tries.held.is_empty(), "unplugged: tried again once plugged in");
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken), ("done", 3));
    }

    #[test]
    fn a_copy_that_cannot_be_written_here_stops_the_take_and_gives_back_the_try_it_took() {
        let here = Here::new("nowrite");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        // One try used up: the phone stopped sending, and did not answer again.
        phone.stall("1.wav", 1);
        phone.answers_again.store(false, Ordering::Relaxed);
        let mut tries = Tries::default();
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "stopped");
        assert_eq!(tries.tried.get(&7), Some(&1));
        // The downloads folder is there, but the copy can't be written in it (in the way: a folder of its name).
        let id = here
            .library
            .lock()
            .unwrap()
            .adopt_songs(&[as_song(&sent(r#"{"file":"1.wav","title":"Song 1.wav","artist":"Someone"}"#))])
            .unwrap()[0];
        std::fs::create_dir_all(here.dir.join(format!(".stash-phone-{id}.part"))).unwrap();
        let reads = phone.reads.lock().unwrap().len();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!(now.state, "cannot_save");
        assert_eq!(phone.reads.lock().unwrap().len(), reads + 1, "read, and could not be written here");
        assert_eq!(now.folder.as_deref(), Some(here.dir.to_string_lossy().as_ref()));
        assert_eq!(tries.tried.get(&7), Some(&1), "the try it took is given back");
        assert!(tries.held.contains(&7));
        assert_eq!(phone.left().len(), 3, "nothing deleted from the phone");
    }

    #[test]
    fn a_library_that_cannot_be_written_is_this_computers_trouble_not_the_phones() {
        let here = Here::new("dbfull");
        here.library.lock().unwrap().refuse_writes().unwrap();
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav()), ("4.wav", wav())]);
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.done, now.failed), ("cannot_save", 0, 0), "stopped at once, not after LOST_AFTER");
        assert!(now.error.as_deref().is_some_and(|e| e.contains("library")), "why: {:?}", now.error);
        assert!(tries.tried.is_empty() && tries.held.contains(&7));
        assert!(phone.reads.lock().unwrap().is_empty());
        assert_eq!(phone.left().len(), 5);
    }

    #[test]
    fn a_like_that_cannot_be_kept_here_keeps_the_copy_of_a_song_not_chosen() {
        let here = Here::asking("likefull");
        here.take_all(true);
        assert_eq!(here.take(&Phone::with(&[("1.wav", wav())]), &mut Tries::default()).unwrap().taken, 1);
        here.take_all(false);
        // 1.wav, liked on the phone, is here already: taking nothing would let its copy go with its like.
        let phone = three("Test phone");
        here.take(&phone, &mut Tries::default());
        let (at, choice) = chosen(&json!({ "sent_at_ms": 8, "files": [], "none": true })).unwrap();
        assert_eq!(choose_from(&here.library, &here.board, at, choice), Ok(0));
        here.library.lock().unwrap().refuse_writes().unwrap();
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!(now.state, "cannot_save");
        assert_eq!(phone.left().len(), 4, "its copy and the list stay, for when the like can be kept");
    }

    #[test]
    fn what_is_recorded_while_songs_are_taken_is_on_the_disk_before_their_copies_go() {
        let here = Here::new("durable");
        let phone = Phone::with(&[("1.wav", wav())]);
        let durable = Mutex::new(Vec::new());
        let (edited, note) = (
            || durable.lock().unwrap().push(here.library.lock().unwrap().commits_durable().unwrap()),
            |_: &str| {},
        );
        let ctx = Ctx {
            library: &here.library,
            board: &here.board,
            edited: &edited,
            log: &note,
        };
        take(&ctx, &phone, Open::of(&phone), &mut Tries::default(), &AtomicBool::new(false));
        assert_eq!(*durable.lock().unwrap(), [true], "durable as the take ended");
        assert!(!here.library.lock().unwrap().commits_durable().unwrap(), "as usual again after");
        // However it ends: here with songs that can't be saved.
        here.library.lock().unwrap().refuse_writes().unwrap();
        let other = Phone::sending("Other phone", 9, &[("2.wav", wav())], vec![json!({ "file": "2.wav", "title": "Two", "artist": "Someone" })]);
        take(&ctx, &other, Open::of(&other), &mut Tries::default(), &AtomicBool::new(false));
        assert_eq!(here.board.now.lock().unwrap().as_ref().unwrap().state, "cannot_save");
        assert_eq!(*durable.lock().unwrap(), [true, true]);
        assert!(!here.library.lock().unwrap().commits_durable().unwrap());
    }

    #[test]
    fn a_file_another_program_holds_a_moment_is_tried_again_and_one_it_keeps_fails_alone() {
        assert!(placing(std::io::Error::from_raw_os_error(112)).starts_with(HERE), "a full disk: this computer's");
        if !cfg!(windows) {
            return;
        }
        let held = || Err::<(), _>(std::io::Error::from_raw_os_error(32));
        let mut asked = 0;
        assert!(while_held(|| { asked += 1; if asked < 3 { held() } else { Ok(()) } }).is_ok());
        assert_eq!(asked, 3, "let go after two tries");
        asked = 0;
        let kept = while_held(|| { asked += 1; held() }).unwrap_err();
        assert_eq!(asked, 1 + HELD_AGAIN_AFTER.len());
        assert!(!placing(kept).starts_with(HERE), "held by another program: that song's trouble alone");
    }

    #[cfg(windows)]
    #[test]
    fn a_song_whose_copy_another_program_keeps_open_fails_and_the_rest_come_in() {
        let here = Here::new("heldopen");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.holds_copies.store(true, Ordering::Relaxed);
        let mut tries = Tries::default();
        // Held for good: neither is put in place, each failing alone (not this computer's trouble, nor the phone lost).
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 0, 2));
        assert_eq!(phone.left().len(), 3, "left on the phone, to be tried again");
        assert!(!tries.held.contains(&7));
        // Let go: the next look takes them.
        phone.holds_copies.store(false, Ordering::Relaxed);
        phone.held.lock().unwrap().clear();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.cleared), ("done", 2, 0, true));
    }

    #[test]
    fn the_end_of_a_take_removes_the_list_and_never_a_file_it_did_not_take() {
        let here = Here::new("stray");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        // A file in the folder the list does not name (another copy of Stash's, one the phone left there).
        phone.files.lock().unwrap().push(("stray.flac".to_string(), wav()));
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.cleared), ("done", 2, true));
        assert_eq!(phone.left(), ["stray.flac"]);
        assert!(!phone.folder_gone.load(Ordering::Relaxed), "the folder stays with what is in it");
    }

    #[test]
    fn a_song_left_out_of_the_listing_after_the_phone_is_opened_again_is_not_counted_taken() {
        let here = Here::new("unlisted");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        phone.unlisted_again.lock().unwrap().insert("3.wav".to_string());
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.done, now.cleared), ("done", 2, 1, 3, false));
        // Never read, so still on the phone, with the list, for another try.
        assert_eq!(phone.left(), ["3.wav", stash_core::phone::LIST]);
        assert_eq!(*phone.reads.lock().unwrap(), ["1.wav", "2.wav", "2.wav"]);
    }

    #[test]
    fn a_send_made_again_while_songs_are_taken_ends_the_take_with_nothing_more_removed() {
        let here = Here::new("again");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        // As 2.wav stops, the phone sends again (2 and 3, as songs not taken yet): a new list.
        *phone.sends_again.lock().unwrap() = Some(list_of(9, &["2.wav", "3.wav"]));
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.cleared), ("done", 1, 0, false));
        assert_eq!(phone.left(), ["2.wav", "3.wav", stash_core::phone::LIST], "the new send is the phone's");
        // The next look reads the new send.
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.total, now.cleared), ("done", 2, 2, true));
        assert!(phone.left().is_empty());

        // The list a new object, but the same send (the phone numbered its files again): the take goes on, and that
        // list goes at the end.
        let here = Here::new("renumbered");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        *phone.sends_again.lock().unwrap() = Some(list_of(7, &["1.wav", "2.wav", "3.wav"]));
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.cleared), ("done", 3, true));
        assert!(phone.left().is_empty());
    }

    #[test]
    fn a_song_that_stops_every_time_is_left_across_tries_and_the_songs_after_it_come_in() {
        // The phone does not answer after a song stops until it is plugged in again: each try ends at that song.
        let here = Here::new("replug");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 99);
        phone.answers_again.store(false, Ordering::Relaxed);
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken), ("stopped", 1));
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "stopped");
        // The third time it stops, it is left; the phone is opened again for the song after it, and does not answer.
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.failed), ("stopped", 1));
        assert!(tries.tried.is_empty(), "a song was left: that try got somewhere");
        assert_eq!(*phone.reads.lock().unwrap(), ["1.wav", "2.wav", "2.wav", "2.wav"], "3.wav not read as it stopped");
        // Plugged in again: the song after it comes in, and the card counts every song that did.
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 2, 1));
        assert_eq!(phone.left(), ["2.wav", stash_core::phone::LIST]);
        assert!(tries.tried.is_empty(), "songs came in");
        assert!(!tries.stalls.contains_key(&(7, "3.wav".to_string())), "no stop counted on the song after it");
        // Not read again on a later try.
        let reads = phone.reads.lock().unwrap().len();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.failed), ("done", 1));
        assert_eq!(phone.reads.lock().unwrap().len(), reads);
        assert_eq!(phone.reads.lock().unwrap().iter().filter(|r| *r == "2.wav").count(), 3);
    }

    #[test]
    fn a_try_that_takes_nothing_off_the_phone_counts_though_its_songs_are_here() {
        // The phone's deletes fail, and one song stops every time: every try finds the first song here already.
        let here = Here::new("undeleted");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.undeletable.lock().unwrap().insert("1.wav".to_string(), "the phone did not answer (0x80004005)".to_string());
        phone.stall("2.wav", 99);
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.taken, now.failed), (1, 1));
        let first = now.at_ms;
        for _ in 0..TRIES {
            std::thread::sleep(Duration::from_millis(3));
            let now = here.take(&phone, &mut tries).unwrap();
            // 1.wav came in already: its copy is deleted again, and it is not counted again.
            assert_eq!((now.state, now.taken, now.had, now.failed), ("done", 1, 0, 1));
            assert_eq!(now.at_ms, first, "nothing more came in: the same news, so a card closed on it stays closed");
        }
        assert_eq!(phone.reads.lock().unwrap().iter().filter(|r| *r == "1.wav").count(), 1);
        assert_eq!(tries.tried.get(&7), Some(&TRIES), "left after TRIES tries, not tried every few seconds for ever");
        let said = here.board.states.lock().unwrap().len();
        here.take(&phone, &mut tries);
        assert_eq!(here.board.states.lock().unwrap().len(), said);
    }

    #[test]
    fn a_liked_song_not_chosen_that_is_here_already_is_liked_here_before_its_copy_goes() {
        let here = Here::asking("liked");
        here.take_all(true);
        assert_eq!(here.take(&Phone::with(&[("1.wav", wav())]), &mut Tries::default()).unwrap().taken, 1);
        here.take_all(false);
        let liked = || here.library.lock().unwrap().liked_ids_at(0, 10).unwrap();
        assert!(liked().is_empty());
        // Liked on the phone since; the window shows it here already, and takes 2.wav only.
        let phone = three("Test phone");
        here.take(&phone, &mut Tries::default());
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.had), ("done", 1, 1));
        assert!(!phone.left().contains(&"1.wav".to_string()), "its copy went from the phone");
        let one = here
            .library
            .lock()
            .unwrap()
            .adopt_songs(&[as_song(&sent(r#"{"file":"1.wav","title":"Song 1.wav","artist":"Someone"}"#))])
            .unwrap();
        assert_eq!(liked(), one);
    }

    #[test]
    fn a_phones_send_gone_while_another_phones_is_taken_is_no_longer_offered() {
        let here = Here::asking("cancelled");
        let a = three("Phone A");
        let b = Phone::sending("Phone B", 9, &[("9.wav", wav())], vec![json!({ "file": "9.wav", "title": "B song", "artist": "Someone" })]);
        let mut tries = Tries::default();
        here.take(&a, &mut tries);
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        // B's send is kept to offer behind A's being taken; then B cancels it (its folder empties).
        here.take(&b, &mut tries);
        assert_eq!(here.board.offer.lock().unwrap().as_ref().unwrap().phone, "Phone B");
        tell(&here.board, "Phone B", "connected", 0);
        assert!(here.board.offer.lock().unwrap().is_none(), "B has nothing to offer now, though A's card stays");
        assert_eq!(here.board.now.lock().unwrap().as_ref().unwrap().phone.as_deref(), Some("Phone A"));
        assert_eq!(here.take(&a, &mut tries).unwrap().state, "done");
        // A sends again: offered.
        let again = Phone::sending("Phone A", 10, &[("4.wav", wav())], vec![json!({ "file": "4.wav", "title": "Song 4", "artist": "Someone" })]);
        let now = here.take(&again, &mut tries).unwrap();
        assert_eq!((now.state, now.phone.as_deref(), now.total), ("ready", Some("Phone A"), 1));
    }

    #[test]
    fn a_send_taken_whole_while_it_waited_is_no_longer_offered() {
        let here = Here::asking("whole");
        let a = three("Phone A");
        let mut tries = Tries::default();
        assert_eq!(here.take(&a, &mut tries).unwrap().state, "ready");
        // Every send to come in whole, turned on while A's waits.
        here.take_all(true);
        assert_eq!(here.take(&a, &mut tries).unwrap().taken, 3);
        assert!(here.board.offer.lock().unwrap().is_none());
        here.take_all(false);
        let b = Phone::sending("Phone B", 9, &[("9.wav", wav())], vec![json!({ "file": "9.wav", "title": "B song", "artist": "Someone" })]);
        let now = here.take(&b, &mut tries).unwrap();
        assert_eq!((now.state, now.phone.as_deref()), ("ready", Some("Phone B")));
    }

    #[test]
    fn a_transfer_said_to_be_taken_from_a_phone_unplugged_is_said_to_be_cut_short() {
        let card = |state: &'static str| {
            Some(Now {
                state,
                phone: Some("A".to_string()),
                total: 5,
                done: 2,
                song: Some("Song · Someone".to_string()),
                at_ms: 5,
                ..Default::default()
            })
        };
        let gone = |_: &str| false;
        for state in ["connected", "preparing", "ready"] {
            let mut now = card(state);
            unlisted(&mut now, &gone);
            assert!(now.is_none(), "{state}: nothing more to say");
        }
        // Said to be taken (by the window's Take, or a take that ended without saying): cut short, with what came in.
        for state in ["taking", "stalled"] {
            let mut now = card(state);
            unlisted(&mut now, &gone);
            let n = now.unwrap();
            assert_eq!((n.state, n.total, n.done, n.song), ("stopped", 5, 2, None), "{state}");
            assert!(n.at_ms > 5, "said anew");
        }
        for state in ["done", "stopped", "withdrawn", "cannot_save"] {
            let mut now = card(state);
            unlisted(&mut now, &gone);
            assert_eq!(now.map(|n| (n.state, n.at_ms)), Some((state, 5)), "{state} stays");
        }
        // Still plugged in: nothing changes; a phone with no list to take while said to be taken is cut short too.
        let mut now = card("taking");
        unlisted(&mut now, &|name| name == "A");
        assert_eq!(now.as_ref().unwrap().state, "taking");
        cut_short(now.as_mut().unwrap());
        assert_eq!(now.unwrap().state, "stopped");
    }

    #[test]
    fn a_list_that_stayed_on_the_phone_is_removed_again_at_the_next_look() {
        let here = Here::asking("listed");
        let phone = three("Test phone");
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        let list = stash_core::phone::LIST.to_string();
        phone.undeletable.lock().unwrap().insert(list.clone(), "the phone did not answer (0x80004005)".to_string());
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.left, now.cleared), ("done", 1, 2, false));
        // Not "you can unplug the phone": its list is still there.
        assert_eq!(here.told()["list_left"], true);
        assert!(here.board.choice(&here.library, 8).is_some(), "still chosen");
        let taken = here.library.lock().unwrap().setting(TAKEN_SETTING).unwrap();
        assert_eq!(taken, None, "not marked taken: its list is still to go");
        // The next look: the chosen song is in, so only the list goes, and nothing new is said: what came in stays
        // said, at the same time (a card closed on it stays closed).
        phone.undeletable.lock().unwrap().clear();
        let (said, at_ms) = (here.said(), now.at_ms);
        std::thread::sleep(Duration::from_millis(3));
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!(here.said(), said, "no new taking, no new done");
        assert_eq!((now.state, now.taken, now.left, now.at_ms), ("done", 1, 2, at_ms));
        let told = here.told();
        assert_eq!(
            (told["state"].as_str(), told["taken"].as_u64(), told["list_left"].as_bool(), told["at_ms"].as_i64()),
            (Some("done"), Some(1), Some(false), Some(at_ms))
        );
        assert_eq!(phone.left(), ["1.wav", "3.wav"]);
        assert_eq!(here.board.choice(&here.library, 8), None, "let go once the list went");
        assert_eq!(*phone.reads.lock().unwrap(), ["2.wav"]);

        // The phone gone as its list was removed: a transfer cut short, all in but for the list.
        let here = Here::new("listlost");
        let phone = Phone::with(&[("1.wav", wav())]);
        phone.undeletable.lock().unwrap().insert(list, "the device is unreachable (0x80070141)".to_string());
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.done, now.total), ("stopped", 1, 1, 1));
        assert_eq!(here.told()["list_left"], true);
        // Plugged in again: the list goes, and the card says what came in, done, at the time it said it.
        phone.undeletable.lock().unwrap().clear();
        let (said, at_ms) = (here.said(), now.at_ms);
        std::thread::sleep(Duration::from_millis(3));
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!(here.said(), said);
        assert_eq!((now.state, now.taken, now.cleared, now.list_left, now.at_ms), ("done", 1, true, false, at_ms));
        assert!(phone.left().is_empty() && phone.folder_gone.load(Ordering::Relaxed));
    }

    #[test]
    fn a_try_that_carries_on_a_send_counts_what_came_in_before() {
        let here = Here::new("carry");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        phone.answers_again.store(false, Ordering::Relaxed);
        let mut tries = Tries::default();
        let first = here.take(&phone, &mut tries).unwrap();
        assert_eq!((first.state, first.taken), ("stopped", 1));
        // Plugged in again: the rest come in, said with the one before.
        phone.answers_again.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(3));
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.done, now.total, now.cleared), ("done", 3, 3, 3, true));
        assert!(now.at_ms > first.at_ms, "news: the card shows again");
        assert_eq!(here.told()["taken"], 3);
        // Another send of the phone's is not counted with this one.
        assert_eq!(here.take(&Phone::with_at(9, &[("4.wav", wav())]), &mut tries).unwrap().taken, 1);
    }

    #[test]
    fn songs_a_listing_left_out_are_said_to_come_in_by_themselves_while_tries_are_left() {
        let here = Here::new("again-told");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        phone.unlisted_again.lock().unwrap().insert("3.wav".to_string());
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 2, 1));
        // Not "send again from the phone": the next look takes it.
        assert_eq!(here.told()["trying_again"], true);
        phone.unlisted_again.lock().unwrap().clear();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.cleared, now.trying_again), ("done", 3, 0, true, false));

        // On the last try, with nothing taken off the phone, there is no next look to take it.
        let here = Here::new("again-last");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.stall("1.wav", 1);
        phone.undeletable.lock().unwrap().insert("1.wav".to_string(), "the phone did not answer (0x80004005)".to_string());
        phone.unlisted_again.lock().unwrap().insert("2.wav".to_string());
        let mut tries = Tries::default();
        tries.tried.insert(7, TRIES - 1);
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.removed), ("done", 1, 1, 0));
        assert_eq!(here.told()["trying_again"], false);
        assert_eq!(tries.tried.get(&7), Some(&TRIES));
    }

    #[test]
    fn a_copy_that_stayed_on_the_phone_is_deleted_again_before_the_list_goes() {
        let here = Here::new("undeleted-once");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.undeletable_times.lock().unwrap().insert("1.wav".to_string(), 1);
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.cleared), ("done", 2, true));
        assert!(phone.left().is_empty(), "its copy went, then the list");

        // One that stays even so: the list goes all the same (the phone is not left waiting), its copy for the phone.
        let here = Here::new("undeleted-twice");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.undeletable_times.lock().unwrap().insert("1.wav".to_string(), 2);
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.list_left), ("done", 2, false));
        assert_eq!(phone.left(), ["1.wav"]);
        assert!(!phone.folder_gone.load(Ordering::Relaxed));
    }

    #[test]
    fn a_transfer_an_earlier_stash_took_has_only_its_list_removed() {
        let here = Here::new("before");
        here.library.lock().unwrap().set_setting(TAKEN_SETTING, "7").unwrap();
        let phone = Phone::with(&[("1.wav", wav())]);
        let list = stash_core::phone::LIST.to_string();
        phone.undeletable.lock().unwrap().insert(list.clone(), "the phone did not answer (0x80004005)".to_string());
        let mut tries = Tries::default();
        for _ in 0..=TRIES {
            here.take(&phone, &mut tries);
        }
        assert_eq!(tries.tried.get(&7), Some(&TRIES), "tried as often as a transfer is");
        phone.undeletable.lock().unwrap().clear();
        here.take(&phone, &mut Tries::default());
        assert_eq!(phone.left(), ["1.wav"], "the list went; nothing else");
        assert!(phone.reads.lock().unwrap().is_empty(), "not taken again");
    }

    #[test]
    fn a_song_whose_file_here_has_gone_missing_is_taken_again() {
        let here = Here::new("missing");
        assert_eq!(here.take(&Phone::with(&[("1.wav", wav())]), &mut Tries::default()).unwrap().taken, 1);
        // Deleted from the downloads folder, and a scan found it gone.
        std::fs::remove_dir_all(&here.dir).unwrap();
        std::fs::create_dir_all(&here.dir).unwrap();
        let marked = here
            .library
            .lock()
            .unwrap()
            .mark_missing(&here.dir.to_string_lossy(), &HashSet::new(), 1)
            .unwrap();
        assert_eq!(marked, 1);
        let phone = Phone::with_at(9, &[("1.wav", wav())]);
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.had), ("done", 1, 0));
        assert_eq!(*phone.reads.lock().unwrap(), ["1.wav"]);
        assert_eq!(here.saved(), 1);
    }

    #[test]
    fn a_send_made_again_under_the_same_object_is_told_apart_by_its_time() {
        let here = Here::new("same-object");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        // Sent again as 2.wav stops, and the new list has the old one's object (the phone numbered its files again).
        phone.same_object.store(true, Ordering::Relaxed);
        *phone.sends_again.lock().unwrap() = Some(list_of(9, &["2.wav", "3.wav"]));
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.cleared), ("done", 1, false));
        assert_eq!(phone.left(), ["2.wav", "3.wav", stash_core::phone::LIST], "the new send, list and all, is the phone's");
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.total, now.cleared), ("done", 2, 2, true));
    }

    #[test]
    fn a_send_the_phone_stops_while_it_is_taken_is_said_to_be_stopped_by_the_phone() {
        let here = Here::new("withdrawn");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav()), ("4.wav", wav())]);
        // Cancel on the phone as 2.wav is read: its copies and list go, and the reads fail (not as a phone lost).
        *phone.withdraws_at.lock().unwrap() = Some("2.wav".to_string());
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.done, now.total), ("withdrawn", 1, 1, 4));
        assert_eq!(*phone.reads.lock().unwrap(), ["1.wav", "2.wav"], "stopped at the first read that failed");
        assert_eq!(here.told()["state"], "withdrawn");
        // A read failing while the list is still there is the song's, as before (the phone is looked at to see).
        let here = Here::new("not-withdrawn");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.unreadable.lock().unwrap().insert("1.wav".to_string());
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 1, 1));
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), 1);

        // Stopped as a song stopped sending: the phone answers again, with no list.
        let here = Here::new("withdrawn-stall");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.stall("2.wav", 1);
        *phone.withdraws_at.lock().unwrap() = Some("2.wav".to_string());
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.done), ("withdrawn", 1, 1));
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), AGAIN_AFTER.len() as u64, "asked as often as for a list");
    }

    #[test]
    fn a_look_at_a_phone_with_no_list_says_what_became_of_a_transfer_said_to_be_taken() {
        let card = |state: &'static str| Now {
            state,
            phone: Some("A".to_string()),
            total: 5,
            done: 2,
            song: Some("Song · Someone".to_string()),
            at_ms: 5,
            ..Default::default()
        };
        let board = Board::new();
        // Its folder answers with no list: the phone stopped the send.
        for state in ["taking", "stalled"] {
            *board.now.lock().unwrap() = Some(card(state));
            nothing_sent(&board, "A", true, false, 0);
            let n = board.now.lock().unwrap().clone().unwrap();
            assert_eq!((n.state, n.done, n.total, n.song.as_deref()), ("withdrawn", 2, 5, None), "{state}");
            assert!(n.at_ms > 5);
            // Said over the phone's "connected" (it is plugged in with nothing sent), until it has news.
            assert!(!told(&mut Some(n), "A", "connected", 0));
        }
        // It does not answer: cut short, as before.
        *board.now.lock().unwrap() = Some(card("taking"));
        nothing_sent(&board, "A", false, false, 0);
        assert_eq!(board.now.lock().unwrap().as_ref().unwrap().state, "stopped");
        // Another phone's card is not this phone's to change; a phone copying is said so over a result.
        *board.now.lock().unwrap() = Some(card("taking"));
        nothing_sent(&board, "B", true, true, 3);
        assert_eq!(board.now.lock().unwrap().as_ref().unwrap().state, "taking");
        *board.now.lock().unwrap() = Some(card("done"));
        nothing_sent(&board, "A", true, true, 3);
        assert_eq!(board.now.lock().unwrap().as_ref().map(|n| (n.state, n.ready)), Some(("preparing", 3)));
        // A list said to have stayed is gone from the phone: nothing is left to finish.
        for state in ["done", "stopped"] {
            *board.now.lock().unwrap() = Some(Now { list_left: true, done: 5, ..card(state) });
            nothing_sent(&board, "A", true, false, 0);
            let n = board.now.lock().unwrap().clone().unwrap();
            assert_eq!((n.state, n.list_left, n.cleared, n.at_ms), ("done", false, true, 5), "{state}");
        }
    }

    #[test]
    fn a_phone_unplugged_is_tried_again_and_what_it_was_said_to_do_is_cut_short() {
        let board = Board::new();
        let phone = |id: &str, name: &str| stash_core::phone::Phone { id: id.to_string(), name: name.to_string() };
        let mut tries = Tries::default();
        tries.tried.insert(7, TRIES);
        tries.held.insert(7);
        tries.stalls.insert((7, "2.wav".to_string()), 3);
        let (a, b) = ("a-id".to_string(), "b-id".to_string());
        // Both still there: nothing changes.
        *board.now.lock().unwrap() = Some(Now { state: "taking", phone: Some("A".into()), at_ms: 5, ..Default::default() });
        phones_now(&board, &mut tries, [&a, &b], &[phone("a-id", "A"), phone("b-id", "B")]);
        assert_eq!((tries.tried.len(), tries.held.len()), (1, 1));
        assert_eq!(board.now.lock().unwrap().as_ref().unwrap().state, "taking");
        // A unplugged: every send is tried again (a song that stops every time is still left), A's transfer cut short,
        // and its send no longer offered.
        *board.offer.lock().unwrap() = Some(Offer { phone: "A".into(), sent_at_ms: 7, songs: Vec::new(), asked: Vec::new() });
        phones_now(&board, &mut tries, [&a, &b], &[phone("b-id", "B")]);
        assert!(tries.tried.is_empty() && tries.held.is_empty());
        assert_eq!(tries.stalls.len(), 1);
        assert_eq!(board.now.lock().unwrap().as_ref().unwrap().state, "stopped");
        assert!(board.offer.lock().unwrap().is_none());
        // Unplugged while its list stayed: to be plugged in again.
        *board.now.lock().unwrap() = Some(Now { state: "done", phone: Some("B".into()), list_left: true, at_ms: 5, ..Default::default() });
        phones_now(&board, &mut tries, [&b], &[]);
        let n = board.now.lock().unwrap().clone().unwrap();
        assert_eq!((n.state, n.list_left), ("stopped", true));
        assert!(n.at_ms > 5);
    }
}
