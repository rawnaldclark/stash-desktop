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
//! A send can come with the phone's library ([`stash_core::phone::LIBRARY`]: its likes, playlists and plays), which
//! [`library`] reads, sums up, and brings in as chosen, before the songs. A send without one is its songs, as before.
//!
//! - `GET /api/phone`: what is happening: nothing, a send waiting to be chosen from, songs being taken from a phone
//!   (how many of how many), or what the last transfer did; and the phones plugged in now (with "File transfer"
//!   chosen), each with whether it has songs sent. A send waiting says whether it came with the phone's library
//!   (`with_library`), and the card whether what to bring in from that phone's library was chosen before
//!   (`library_choice_kept`).
//! - `GET /api/phone/songs`: the send waiting to be chosen from: each song as the phone describes it (its format and
//!   quality, its tags, the playlists it is in), its size, and whether it is here already.
//! - `GET /api/phone/library`: the library that came with the send waiting to be chosen from, summed up (404 when none
//!   did).
//! - `POST /api/phone/take` `{sent_at_ms, files}` (or `all: true`, or `files: [], none: true`): takes those songs.
//!   With `library: {likes, plays, playlists, only_here}` (`likes` true, false, or the services they were made on),
//!   those parts of the phone's library come in first; what was left out is kept, and each later send's library comes
//!   in but for that.
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
use stash_core::phone::{HERE, ONE_SONG, PhoneFile, STALL, STALLED, Watch, held_by_another};
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, now_ms, refuse, reply_json};

mod library;

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
    /// it was taken: its list gone), or "cannot_save" (songs can't be saved in `folder` on this computer, or kept in
    /// Stash's library when `library`: `error` says why; tried again once the phone is plugged in again).
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
    /// The send waiting to be chosen from ("ready") came with the phone's library: `GET /api/phone/library` says what
    /// is in it.
    with_library: bool,
    /// Songs left on the phone because they were not chosen.
    left: usize,
    /// The send's list went from the phone with none of its songs left there (the phone then has nothing sent).
    cleared: bool,
    /// Every song chosen is in, but the send's list stayed on the phone: it is removed at a look while the phone is
    /// plugged in (by itself while `trying_again`, else once the phone is plugged in again).
    list_left: bool,
    /// The next look carries the send on by itself, with no need to send again or plug the phone in again: it takes
    /// the songs not taken, which the phone's listing left out (as it was opened again), or it removes the list that
    /// stayed.
    trying_again: bool,
    at_ms: i64,
    error: Option<String>,
    /// The downloads folder, when songs can't be saved in it.
    folder: Option<String>,
    /// Songs can't be kept in Stash's library (not the downloads folder), which is on `drive`.
    library: bool,
    drive: Option<String>,
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

/// What was said of a send before a try that carries it on: how it ended, when, how many songs it counted in, and what
/// it asked of the listener.
#[derive(Debug, Clone, Copy)]
struct Before {
    state: &'static str,
    at_ms: i64,
    brought: usize,
    trying_again: bool,
    list_left: bool,
}

impl Before {
    fn of(n: &Now) -> Before {
        Before {
            state: n.state,
            at_ms: n.at_ms,
            brought: n.brought(),
            trying_again: n.trying_again,
            list_left: n.list_left,
        }
    }
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
    /// By phone: how its last send's last try ended, kept apart from the board (another card can take its place
    /// there), for a try that carries that send on.
    ended: HashMap<String, Now>,
}

impl Tries {
    /// Every send tried again from the start (Settings' "Look for my phone"): what each came to stays known.
    fn again(&mut self) {
        self.tried.clear();
        self.stalls.clear();
        self.held.clear();
    }

    /// How `phone`'s send made at `sent_at_ms` ended before this try, if it did: as the board says it when the board is
    /// on that send's ending (it may say more since: the phone unplugged, the list found gone), else as its last try
    /// ended.
    fn earlier(&self, board: &Board, phone: &str, sent_at_ms: i64) -> Option<Now> {
        let ending = |n: &Now| n.is_of(phone, sent_at_ms) && is_over(n.state);
        let shown = board.now.lock().unwrap().clone();
        shown
            .filter(ending)
            .or_else(|| self.ended.get(phone).filter(|n| ending(n)).cloned())
    }
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
    /// The phone's library that came with the send, when one did that can be used.
    #[serde(skip)]
    library: Option<std::sync::Arc<import::web_library::FromPhone>>,
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
    /// The library each phone's last send came with, as read (once for the send).
    library: Mutex<Vec<library::Read>>,
    /// Each thing said, and the time it was said at.
    #[cfg(test)]
    states: Mutex<Vec<&'static str>>,
    #[cfg(test)]
    times: Mutex<Vec<i64>>,
}

impl Board {
    const fn new() -> Self {
        Board {
            now: Mutex::new(None),
            offer: Mutex::new(None),
            chosen: Mutex::new(None),
            library: Mutex::new(Vec::new()),
            #[cfg(test)]
            states: Mutex::new(Vec::new()),
            #[cfg(test)]
            times: Mutex::new(Vec::new()),
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
        {
            self.states.lock().unwrap().push(now.state);
            self.times.lock().unwrap().push(now.at_ms);
        }
        *self.now.lock().unwrap() = Some(now);
    }

    /// How much of the transfer has come, as a song comes in.
    fn bytes_done(&self, bytes: u64) {
        if let Some(now) = self.now.lock().unwrap().as_mut() {
            now.bytes_done = bytes;
        }
    }

    /// The song being taken has stopped coming: its read is being called off and the phone asked again. Said anew
    /// (`news`), unless the take only carries on a send that ended before and has brought nothing more in (a card closed
    /// on that send stays closed).
    fn stalled(&self, news: bool) {
        #[cfg(test)]
        self.states.lock().unwrap().push("stalled");
        if let Some(now) = self.now.lock().unwrap().as_mut() {
            now.state = "stalled";
            if news {
                now.at_ms = now_ms();
            }
            #[cfg(test)]
            self.times.lock().unwrap().push(now.at_ms);
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

/// The waits before each try again at what another holds a moment: a file another program has open, the library
/// another writer keeps.
#[cfg(not(test))]
const HELD_AGAIN_AFTER: [Duration; 3] = [Duration::from_millis(250); 3];
#[cfg(test)]
const HELD_AGAIN_AFTER: [Duration; 3] = [Duration::from_millis(1); 3];

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

/// Trouble with a song's file here, in words: one another program holds is said so.
fn file_trouble(e: &std::io::Error) -> String {
    if held_by_another(e) {
        format!("another program has the file open: {e}")
    } else {
        e.to_string()
    }
}

/// Trouble making or writing a song's file in the downloads folder: the drive's ([`HERE`]: a full disk, a drive gone),
/// which every song would meet; but the song's alone ([`ONE_SONG`]) when another program holds its file.
fn in_the_folder(e: std::io::Error) -> String {
    let whose = if held_by_another(&e) { ONE_SONG } else { HERE };
    format!("{whose}{}", file_trouble(&e))
}

/// What trouble in Stash's library starts with when it is the library's file or drive (full, failing, not opening,
/// read-only, damaged): every song would meet it, so the take stops (and says the library, not the downloads folder).
const LIBRARY: &str = "in Stash's library: ";

/// Trouble in the library here: its file's or drive's ([`LIBRARY`]), or that one song's ([`ONE_SONG`]: a row in its
/// way, another writer keeping the library past the wait for it).
fn library_trouble(e: stash_core::Error) -> String {
    let whose = if e.of_the_file() { LIBRARY } else { ONE_SONG };
    match &e {
        stash_core::Error::Db(db) => format!("{whose}{db}"),
        e => format!("{whose}{e}"),
    }
}

/// `act` on the library, tried again a few times a moment apart while another writer keeps it (a scan's: each write
/// waits a while for it first); its trouble as [`library_trouble`] says.
fn in_library<T>(
    library: &Mutex<Library>,
    mut act: impl FnMut(&mut Library) -> stash_core::Result<T>,
) -> Result<T, String> {
    let mut waits = HELD_AGAIN_AFTER.iter();
    loop {
        // Locked for the act only: let go while waiting, for the writer to be done.
        let done = act(&mut library.lock().unwrap());
        match done {
            Err(e) if e.busy() => match waits.next() {
                Some(wait) => std::thread::sleep(*wait),
                None => return Err(library_trouble(e)),
            },
            done => return done.map_err(library_trouble),
        }
    }
}

/// Takes one song: its row, its file put where Stash saves songs (on the disk before it is recorded), its download
/// recorded. A song the library has as a file already is not read from the phone; any other is read once, whole, and
/// what kind of file it is is told from the copy here. A song the list marks liked is liked here too, when `likes`
/// (not when the phone's library is brought in without its likes). Trouble here starts with [`HERE`] or [`LIBRARY`]
/// when no song could be kept (a drive full or gone), with [`ONE_SONG`] when it is only this song's.
fn take_one(
    ctx: &Ctx,
    folder: &dyn Folder,
    file: &PhoneFile,
    sent: &Sent,
    watch: &Watch,
    likes: bool,
) -> Result<Took, String> {
    let song = as_song(sent);
    let (id, wanted, root) = in_library(ctx.library, |library| {
        let Some(&id) = library.adopt_songs(std::slice::from_ref(&song))?.first() else {
            return Ok(None);
        };
        if sent.liked && likes {
            library.set_liked(id, true, now_ms())?;
        }
        Ok(Some((id, library.wants_file(id)?, crate::saving::folder(library))))
    })?
    .ok_or_else(|| format!("{ONE_SONG}the song was not kept"))?;
    if !wanted {
        return Ok(Took::Had);
    }
    std::fs::create_dir_all(&root).map_err(in_the_folder)?;
    let part = root.join(format!(".stash-phone-{id}.part"));
    // One left by a try before, whose file another program held: gone first, once that program lets it go.
    match while_held(|| std::fs::remove_file(&part)) {
        Err(e) if held_by_another(&e) => return Err(in_the_folder(e)),
        _ => {}
    }
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
    let arriving = in_library(ctx.library, |library| {
        Ok(library
            .download_arriving(id, now_ms())?
            .then(|| library.track_number(id).ok().flatten()))
    });
    let number = match arriving {
        Ok(Some(number)) => number,
        arriving => {
            let _ = std::fs::remove_file(&part);
            return arriving.map(|_| Took::Had);
        }
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
    if let Err(e) = target.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())) {
        return Err(failed(in_the_folder(e)));
    }
    // Put in place under its name: trouble then is this song's (its name, a file of its name in the way), as a drive
    // full or gone shows in the next song's copy.
    if let Err(e) = while_held(|| std::fs::rename(&part, &target)) {
        return Err(failed(format!("{ONE_SONG}{}", file_trouble(&e))));
    }
    // On the disk under its name before it is recorded and the phone's copy deleted: a power cut after that keeps it.
    // (Windows flushes a file only when it is open for writing.)
    let synced = while_held(|| std::fs::OpenOptions::new().write(true).open(&target))
        .and_then(|f| f.sync_all());
    if let Err(e) = synced {
        crate::saving::remove_saved(&target, &root);
        return Err(failed(in_the_folder(e)));
    }
    // The file gone from here as it was read back, or one the library cannot read: this song's trouble.
    let read = downloads::read(&target).map_err(|e| {
        crate::saving::remove_saved(&target, &root);
        failed(match e {
            downloads::Trouble::Held(why, _) | downloads::Trouble::Failed(why) => format!("{ONE_SONG}{why}"),
            e => format!("{ONE_SONG}{e:?}"),
        })
    })?;
    let saved = SavedDownload {
        addon: "phone".into(),
        tier: None,
        at_ms: now_ms(),
        fetched: false,
        cover_inside: false,
    };
    let kept = in_library(ctx.library, |library| library.download_saved(id, &read, &saved));
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
    let listing = folder.files();
    let Some(list_file) = listing
        .iter()
        .find(|f| f.name == stash_core::phone::LIST)
        .cloned()
    else {
        return;
    };
    let bytes = match folder.bytes(&list_file, LIST_CAP) {
        Ok(bytes) => bytes,
        Err(why) => {
            // Listed but not answering (its USB connection dropped): said once, as a transfer cut short, on what was
            // said of the phone (its counts and its send kept). A send of this phone's waiting to be chosen from is
            // left as it is: nothing was being taken, and the same send is offered again (with what the window chose
            // from it) once the phone answers. Another phone's card is not this one's to say over.
            let waiting = ctx
                .board
                .offer
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|o| o.phone == phone.name());
            if lost(&why) && !waiting {
                let mut now = ctx.board.now.lock().unwrap();
                match now.as_mut() {
                    Some(n) if n.phone.as_deref() != Some(phone.name()) => {}
                    Some(n) if !matches!(n.state, "connected" | "preparing") => out_of_reach(n),
                    _ => {
                        *now = Some(Now {
                            state: "stopped",
                            phone: Some(phone.name().to_string()),
                            at_ms: now_ms(),
                            ..Default::default()
                        })
                    }
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
            // The library that came with it goes first, as at the end of a take.
            if let Some(file) = listing.iter().find(|f| f.name == stash_core::phone::LIBRARY)
                && let Err(e) = folder.remove_file(file)
            {
                (ctx.log)(&format!("the library that came with a transfer taken before stayed on the phone, for it to clear: {e}"));
            }
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
    let earlier = tries.earlier(ctx.board, phone.name(), list.sent_at_ms);
    // The parts of the phone's library to bring in before the songs, while they are still to come in from this send:
    // read from the phone (once for the send) before the take holds anything.
    let library = library::for_take(ctx, phone.name(), folder.as_ref(), &listing, list.sent_at_ms);
    let now = take_songs(
        ctx,
        phone,
        folder,
        &list_file,
        &list,
        &choice,
        library,
        earlier,
        &mut tries.stalls,
        last_try,
        stop,
    );
    if is_over(now.state) {
        tries.ended.insert(phone.name().to_string(), now.clone());
    }
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
        with_library: offer.library.is_some(),
        bytes_total: offer.songs.iter().map(|s| s.bytes).sum(),
        at_ms,
        sent_at_ms: Some(offer.sent_at_ms),
        ..Default::default()
    }
}

/// Offers `list`'s songs still on the phone to choose from, each said to be here already or not (found as taking it
/// would find it, changing nothing), with the phone's library when one came with the send (read once for the send:
/// one the phone did not answer for is read again at the next look, and offered once it is). Worked out once for a
/// send, and again when what is on the phone changes. A send of no songs with a library is offered too. One send is
/// offered at a time: another phone's waits until that one is chosen from or unplugged.
fn offer(ctx: &Ctx, phone: &dyn Plugged, folder: &dyn Folder, list: &List) {
    let listing = folder.files();
    let sizes: HashMap<&str, u64> = listing.iter().map(|f| (f.name.as_str(), f.size)).collect();
    let on_phone: Vec<&Sent> = list
        .songs
        .iter()
        .filter(|s| sizes.contains_key(s.file.as_str()))
        .collect();
    if ctx.board.offer.lock().unwrap().as_ref().is_some_and(|o| o.phone != phone.name()) {
        return;
    }
    // Read with no lock held: a phone reads slowly.
    let library = library::of_send(ctx, phone.name(), folder, &listing, list.sent_at_ms);
    // Chosen from as its library was read: not offered again.
    if ctx
        .board
        .chosen
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|c| c.contains_key(&list.sent_at_ms))
    {
        return;
    }
    {
        let mut offered = ctx.board.offer.lock().unwrap();
        if let Some(o) = offered.as_mut() {
            if o.phone != phone.name() {
                return;
            }
            if o.sent_at_ms == list.sent_at_ms && o.songs.len() == on_phone.len() {
                // Its library read since it was offered: offered with it now.
                let late = o.library.is_none() && library.is_some();
                if late {
                    o.library = library;
                }
                // Offered already; said again when something else was said over it: on this phone, or what another
                // phone did (its transfer done or stopped), but not over another phone's send being chosen or taken.
                let mut now = ctx.board.now.lock().unwrap();
                let shown = now.as_ref().is_some_and(|n| n.state == "ready");
                if (!shown || late) && !busy_elsewhere(now.as_ref(), phone.name()) {
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
            bytes: sizes[s.file.as_str()],
            here,
        })
        .collect();
    let offer = Offer {
        phone: phone.name().to_string(),
        sent_at_ms: list.sent_at_ms,
        songs,
        asked,
        library,
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
        "offering {} songs, {} MB ({} here already){}",
        now.total,
        now.bytes_total / 1_000_000,
        now.here,
        if now.with_library { ", with the phone's library" } else { "" }
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
/// the computer is done. Songs that can't be saved here stop it at once ("cannot_save"), as the phone stopping the send
/// does ("withdrawn"); a song only this computer can't keep fails alone. A try that carries on a send that ended before
/// (`earlier`) counts on from what was said of it, at the time it was said until more comes in; one that only finishes
/// it says nothing new ([`finish`]). `last_try`: another try comes only if this one takes songs off the phone. The parts
/// of the phone's library kept for it come in before the first song while they are still to come from this send
/// (`library`: [`library::bring_in`]); trouble with that is never the songs' unless it is the library's file here, which
/// stops the take as songs that can't be saved do. Kept without the likes, the likes the list marks do not come in
/// either. What the transfer came to.
#[allow(clippy::too_many_arguments)]
fn take_songs(
    ctx: &Ctx,
    phone: &dyn Plugged,
    folder: Box<dyn Folder>,
    list_file: &PhoneFile,
    list: &List,
    choice: &Choice,
    library: library::ForTake,
    earlier: Option<Now>,
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
    let likes = library.likes();
    // How the tries before ended this send: this one carries it on.
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
            last_try,
            likes,
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
        // What came in before counts with what comes in now. Said at the time it was said until more comes in: a card
        // closed on it stays closed while this try brings nothing new.
        now.taken = earlier.taken;
        now.had = earlier.had;
        now.not_kept = earlier.not_kept;
        now.undeleted = earlier.undeleted.clone();
        now.at_ms = earlier.at_ms;
        now.before = Some(Before::of(earlier));
    }
    // Whether this take has said anything new.
    let mut news = earlier.is_none();
    ctx.board.set(now.clone());
    (ctx.log)(&format!(
        "taking {} songs, {} MB ({} taken before)",
        here.len(),
        now.bytes_total / 1_000_000,
        now.done
    ));
    // The phone's library, before any song: a song sent that it names then lands on the row it made.
    if let (Some(parts), Some(from)) = (&library.kept, &library.due) {
        if stop.load(Ordering::Relaxed) {
            return now;
        }
        match library::bring_in(ctx, phone.name(), list, choice, parts, from) {
            Ok(()) => {}
            Err(why) if why.starts_with(LIBRARY) => return cannot_save(ctx, now, &why),
            Err(why) => (ctx.log)(&format!("the phone's library did not come in, its songs do: {why}")),
        }
    }
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
        if now.undeleted.contains(&sent.file) {
            // Counted in by a try before, which could not delete its copy: only the copy goes now (from the phone as
            // it is open: not one closed after a song stopped sending, which is left closed when nothing is to be read).
            if let Some(current) = folder.as_deref()
                && current.remove_file(&file).is_ok()
            {
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
        let current = folder.as_deref().expect("a folder while a song is to be read");
        now.state = "taking";
        now.song = Some(format!("{} · {}", sent.title.trim(), sent.artist.trim()));
        ctx.board.set(now.clone());
        let (base, size, board) = (now.bytes_done, file.size, ctx.board);
        let progress = |n: u64| board.bytes_done(base + n.min(size));
        let stalled = || board.stalled(news);
        let watch = Watch {
            stall_after: STALL,
            progress: &progress,
            stalled: &stalled,
        };
        let at = Instant::now();
        match take_one(ctx, current, &file, sent, &watch, likes) {
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
                if !news {
                    // More came in than before: news, said anew (a card closed shows again).
                    news = true;
                    now.at_ms = now_ms();
                }
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
                // The phone closed: its File transfer stopped, so no song (this one again, or the next) is read from it
                // as it is. Opened again for a song still to be read; with none (each one after it left, or in
                // already), the take goes on to its end with what came in.
                drop(folder.take());
                let to_read = here[i..].iter().any(|s| {
                    files.contains_key(&s.file)
                        && !now.undeleted.contains(&s.file)
                        && stalls
                            .get(&(list.sent_at_ms, s.file.clone()))
                            .is_none_or(|n| *n <= STALLS_PER_SONG)
                });
                if !to_read {
                    continue;
                }
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
            // This computer's trouble (its disk full, the downloads drive gone, the library's file failing), not the
            // phone's: every song would fail the same way.
            Err(why) if why.starts_with(HERE) || why.starts_with(LIBRARY) => return cannot_save(ctx, now, &why),
            // This computer's trouble with this song alone (its file held by another program, the library kept by
            // another writer, a row in its way): it fails, and the rest go on. Not the phone's, so the phone is not
            // looked at again for it, and it does not count towards taking the phone as gone.
            Err(why) if why.starts_with(ONE_SONG) => {
                now.failed += 1;
                (ctx.log)(&format!("{} was not taken: {why}", sent.file));
                continue_failed(ctx, &mut now, &file);
                i += 1;
            }
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
        return match end_of_take(ctx, phone, current, list, &list_file, choice, &mut files, &mut now, likes) {
            Ok(state) => {
                now.trying_again = state == "done" && carried_on(&now, last_try);
                summary(&now);
                ended(ctx, now, state)
            }
            Err(why) => cannot_save(ctx, now, &why),
        };
    }
    // Everything not taken stays on the phone, to be tried again: by the next look, with no need to send again, when
    // the phone's listing only left it out.
    now.trying_again = carried_on(&now, last_try);
    summary(&now);
    ended(ctx, now, "done")
}

/// Whether the next look carries a take that ended done on by itself (`trying_again`): the songs not taken were only
/// left out of the phone's listing, or only the list stayed on the phone; and the send has a try left (one that took
/// copies off the phone always leaves one more).
fn carried_on(now: &Now, last_try: bool) -> bool {
    let only = if now.failed > 0 {
        now.failed == now.unlisted
    } else {
        now.list_left
    };
    only && (now.removed > 0 || !last_try)
}

/// Whether a take saying `state` is over: done, stopped, withdrawn, or with songs that can't be saved.
fn is_over(state: &str) -> bool {
    matches!(state, "done" | "stopped" | "withdrawn" | "cannot_save")
}

/// The end of a take with every song chosen in. The copies of songs counted in that stayed on the phone are deleted
/// once more (one left there once the list goes reads on the phone as a song not chosen; one that stays even so is
/// left for the phone to clear), those of songs not chosen that are here already go too ([`drop_here_already`], their
/// likes with them when `likes`), then the phone's library (one that stays is the phone's to clear), then the list
/// read (`list_file`), by its object: never the folder with what is in it, which can hold songs this computer
/// has not seen (a listing can leave one out; the phone may have sent again since). The folder goes too once nothing
/// is left in it (only if it is empty). How the take ends: done, or stopped when the phone went as its list was
/// removed, either way with `list_left` when the list stayed (the choice is kept, and a look removes it again);
/// trouble with the library's file here, as [`LIBRARY`].
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
    likes: bool,
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
    let dropped = drop_here_already(ctx, folder, &unchosen, files, likes)?;
    now.had += dropped;
    now.removed += dropped;
    now.left = list
        .songs
        .iter()
        .filter(|s| !choice.includes(&s.file) && files.contains_key(&s.file))
        .count();
    // The phone's library goes before the list (whatever of it was brought in: that was before the songs). One that
    // stays is no trouble: the phone clears it.
    if let Some(file) = files.get(stash_core::phone::LIBRARY).cloned() {
        match folder.remove_file(&file) {
            Ok(()) => {
                files.remove(stash_core::phone::LIBRARY);
            }
            Err(e) => (ctx.log)(&format!(
                "the library that came with the send stayed on the phone, for it to clear: {e}"
            )),
        }
    }
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
            library::let_go(ctx.board, list.sent_at_ms);
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
/// it went, with no song tried again), unless what the card asks of the listener changes ([`ending`]). Said only where
/// the board is on this send (never over another phone's card); either way it is what the try came to. None when a
/// song chosen is still to be taken.
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
    last_try: bool,
    likes: bool,
) -> Option<Now> {
    if !here.iter().all(|s| earlier.undeleted.contains(&s.file)) {
        return None;
    }
    // The songs counted failed are not listed now either: none of them is left to read.
    let mut now = Now {
        removed: 0,
        gave_up: 0,
        unlisted: earlier.failed,
        before: Some(Before::of(earlier)),
        ..earlier.clone()
    };
    let now = match end_of_take(ctx, phone, folder, list, list_file, choice, files, &mut now, likes) {
        Ok(state) => {
            (ctx.log)(if now.list_left {
                "a transfer taken before: its list stayed on the phone again"
            } else {
                "a transfer taken before is finished: its list went from the phone"
            });
            if !now.list_left {
                // With the list gone no song of the send is tried again.
                now.failed = 0;
            }
            now.trying_again = state == "done" && carried_on(&now, last_try);
            now.error = None;
            now.folder = None;
            now.library = false;
            now.drive = None;
            ending(now, state)
        }
        Err(why) => ending(trouble(ctx, now, &why), "cannot_save"),
    };
    {
        let mut said = ctx.board.now.lock().unwrap();
        if said.as_ref().is_some_and(|n| n.is_of(phone.name(), list.sent_at_ms)) {
            *said = Some(now.clone());
        }
    }
    (ctx.edited)();
    Some(now)
}

/// What a take whose songs can't be kept here says (`why`, after [`HERE`] or [`LIBRARY`]): this computer's trouble
/// (its disk full, the downloads drive gone, the library's file failing), not the phone's, and every song would fail
/// the same way. With where: the downloads folder, or the library and the drive it is on. Nothing carries on by itself
/// then: the send is tried again once the phone is plugged in again.
fn trouble(ctx: &Ctx, mut now: Now, why: &str) -> Now {
    now.trying_again = false;
    if let Some(reason) = why.strip_prefix(LIBRARY) {
        let file = ctx.library.lock().unwrap().file();
        (ctx.log)(&format!(
            "songs can't be kept in Stash's library ({}) after {} songs: {reason}",
            file.as_deref().map_or_else(|| "in memory".into(), |f| f.display().to_string()),
            now.done
        ));
        now.error = Some(reason.to_string());
        now.folder = None;
        now.library = true;
        now.drive = file.as_deref().and_then(drive_of);
    } else {
        let reason = why.strip_prefix(HERE).unwrap_or(why).to_string();
        let dir = crate::saving::folder(&ctx.library.lock().unwrap());
        (ctx.log)(&format!(
            "songs can't be saved in {} after {} songs: {reason}",
            dir.display(),
            now.done
        ));
        now.error = Some(reason);
        now.folder = Some(dir.to_string_lossy().into_owned());
        now.library = false;
        now.drive = None;
    }
    now
}

/// The drive `file` is on, as Windows names it ("C:"); with none (another system), the folder it is in.
fn drive_of(file: &Path) -> Option<String> {
    match file.components().next() {
        Some(std::path::Component::Prefix(prefix)) => Some(match prefix.kind() {
            std::path::Prefix::Disk(letter) | std::path::Prefix::VerbatimDisk(letter) => {
                format!("{}:", char::from(letter))
            }
            _ => prefix.as_os_str().to_string_lossy().into_owned(),
        }),
        _ => file.parent().map(|p| p.display().to_string()),
    }
}

/// Ends a take whose songs can't be kept here ([`trouble`]).
fn cannot_save(ctx: &Ctx, now: Now, why: &str) -> Now {
    ended(ctx, trouble(ctx, now, why), "cannot_save")
}

/// A take ending as `state`, and its time: that of what was said of the send before (`before`) when this is the same
/// news (as many songs in, ending the same way, asking the same of the listener), or the list that stayed going as
/// the card said it would; else now (a card closed on the send shows again).
fn ending(mut now: Now, state: &'static str) -> Now {
    now.state = state;
    now.song = None;
    now.at_ms = match now.before {
        Some(b)
            if b.brought == now.brought()
                && (b.state == state && b.trying_again == now.trying_again && b.list_left == now.list_left
                    || b.list_left && !now.list_left && state == "done") =>
        {
            b.at_ms
        }
        _ => now_ms(),
    };
    now
}

/// Ends a take as `state` ([`ending`]): said, and the library pages told.
fn ended(ctx: &Ctx, now: Now, state: &'static str) -> Now {
    let now = ending(now, state);
    ctx.board.set(now.clone());
    (ctx.edited)();
    now
}

/// Deletes from the phone the copies of `songs` (not chosen, and on the phone) that this computer has already, as each
/// song taken is: the phone sees them go and sends them no more, so a send carries only songs this computer has not
/// got. One liked on the phone is liked here first, as a song taken is (when `likes`); one whose like can't be kept
/// now keeps its copy (a like would go with it). How many went; trouble with the library's file here, as [`LIBRARY`].
fn drop_here_already(
    ctx: &Ctx,
    folder: &dyn Folder,
    songs: &[&Sent],
    files: &mut HashMap<String, PhoneFile>,
    likes: bool,
) -> Result<usize, String> {
    if songs.is_empty() {
        return Ok(0);
    }
    let asked: Vec<import::Song> = songs.iter().map(|s| as_song(s)).collect();
    let here = match in_library(ctx.library, |library| library.songs_here(&asked)) {
        Ok(here) => here,
        Err(why) if why.starts_with(LIBRARY) => return Err(why),
        Err(why) => {
            (ctx.log)(&format!("songs not chosen were not looked for here: {why}"));
            return Ok(0);
        }
    };
    let mut gone = 0;
    for ((sent, here), song) in songs.iter().zip(here).zip(&asked) {
        if !here {
            continue;
        }
        let Some(file) = files.get(&sent.file) else {
            continue;
        };
        if sent.liked && likes {
            // The phone sends a song no more once its copy goes, so its like comes now (found as taking it finds it).
            let liked = in_library(ctx.library, |library| {
                if let Some(&id) = library.adopt_songs(std::slice::from_ref(song))?.first() {
                    library.set_liked(id, true, now_ms())?;
                }
                Ok(())
            });
            match liked {
                Ok(()) => {}
                Err(why) if why.starts_with(LIBRARY) => return Err(why),
                Err(why) => {
                    (ctx.log)(&format!("{}: its like could not be kept here, so its copy stays: {why}", sent.file));
                    continue;
                }
            }
        }
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
            tries.again();
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
            tries.again();
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
                    // there that are not growing in number are copies left from a send taken already. The phone's
                    // library is not a song.
                    let ready = transfer.as_ref().map_or(0, |t| {
                        t.files.iter().filter(|f| f.name != stash_core::phone::LIBRARY).count()
                    });
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

/// A look inside a phone with no list to take: its transfer folder `opened` (it answers, with every file in it listed,
/// and no list among them) or not (it does not answer, or has no such folder), with `ready` songs in it, still coming
/// in while `copying`. A transfer said to be taken from it, or said to have stopped while taken, was ended by the phone,
/// which stopped the send (its folder answers, with no list: its Cancel, whatever it deleted first). A list said to
/// have stayed on it is gone: there is nothing left for this computer to finish. A phone that does not answer is out of
/// reach ([`out_of_reach`]). Then it is said to be connected, or copying ([`tell`]).
#[cfg(any(windows, test))]
fn nothing_sent(board: &Board, phone: &str, opened: bool, copying: bool, ready: usize) {
    if let Some(n) = board
        .now
        .lock()
        .unwrap()
        .as_mut()
        .filter(|n| n.phone.as_deref() == Some(phone))
    {
        if !opened {
            out_of_reach(n);
        } else if matches!(n.state, "taking" | "stalled") || n.state == "stopped" && !n.list_left {
            n.state = "withdrawn";
            n.song = None;
            n.at_ms = now_ms();
        } else if n.list_left {
            n.list_left = false;
            n.trying_again = false;
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
    } else {
        out_of_reach(n);
    }
}

/// What was said of a phone, once it is out of reach (unplugged, or not answering): a transfer said to be taken from it
/// was cut short ([`cut_short`]), and one whose list stayed on it, or with songs the computer was to try again by
/// itself, is to be plugged in again: nothing more comes in until then. The rest stays as it was said.
fn out_of_reach(n: &mut Now) {
    if n.state == "done" && (n.list_left || n.trying_again) {
        n.state = "stopped";
        n.trying_again = false;
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

/// Takes from the send offered now what was chosen from it: how many songs that is, with the `parts` of the phone's
/// library chosen, when they are (kept: they come in from each later send's library too). Refused when a song chosen
/// is not offered now (gone from the phone since the window read the songs), or a playlist chosen is not in the library
/// the send came with (or it came with none), so the window reads them again rather than that song or playlist quietly
/// not coming in. The window is told at once that they are being taken.
fn choose_from(
    library: &Mutex<Library>,
    board: &Board,
    sent_at_ms: i64,
    choice: Choice,
    parts: Option<library::Asked>,
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
    if let Some(parts) = &parts {
        let left_out = library::can_bring(&offer, parts)?;
        // Kept before the send is chosen from: the take that follows brings all but these in.
        library::keep(library, &offer.phone, &left_out);
    }
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
        (Method::Get, "/api/phone/library") => {
            let Some(offer) = BOARD.offer.lock().unwrap().clone() else {
                return Ok(Some(refuse(404, "no songs are waiting on a phone")));
            };
            let Some(from) = offer.library.clone() else {
                return Ok(Some(refuse(404, "the phone sent no library with these songs")));
            };
            Ok(Some(reply_json(200, library::summary(&state.library, &offer, &from))))
        }
        (Method::Post, "/api/phone/take") => {
            let body = body_json(request)?;
            let asked = chosen(&body).and_then(|(at, choice)| Ok((at, choice, library::asked(&body)?)));
            Ok(Some(match asked {
                Ok((sent_at_ms, choice, parts)) => match choose_from(&state.library, &BOARD, sent_at_ms, choice, parts) {
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
            // Whether what to bring in from this phone's library was chosen before: its sends' libraries then come in
            // by themselves.
            out["library_choice_kept"] = json!(
                now.phone
                    .as_deref()
                    .is_some_and(|phone| library::choice_kept(&state.library, phone))
            );
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

    /// What is told the name of each file a phone in memory has read.
    type OnRead = Box<dyn FnMut(&str) + Send>;

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
        /// The copy written here of each of these files is held open by another program (an antivirus looking at it),
        /// until let go.
        holds: Mutex<HashSet<String>>,
        held: Mutex<Vec<std::fs::File>>,
        /// Told the name of each file as it is read.
        on_read: Mutex<Option<OnRead>>,
        /// The file as whose read the phone's Cancel lands (that read fails), and whether the phone deletes its list
        /// first (else last): from then on its files go one at a time, one more as it is listed or read.
        cancels_at: Mutex<Option<(String, bool)>>,
        clearing: Mutex<std::collections::VecDeque<String>>,
        /// Opened again, it lists its folder only part way (it stopped listing): no answer, as `open_transfer` says
        /// then.
        lists_partly: AtomicBool,
        /// Every file deleted from its folder, in order.
        removed: Mutex<Vec<String>>,
        /// How often its library was read, and how many more reads of it it does not answer.
        library_reads: AtomicU64,
        library_unanswered: AtomicU64,
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

        /// Its library sent with the send (these bytes), written as the phone writes it: before its list.
        fn sends_library(&self, bytes: Vec<u8>) {
            let mut files = self.files.lock().unwrap();
            let at = files
                .iter()
                .position(|(n, _)| n == stash_core::phone::LIST)
                .unwrap_or(files.len());
            files.insert(at, (stash_core::phone::LIBRARY.to_string(), bytes));
        }

        fn left(&self) -> Vec<String> {
            self.files.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
        }

        /// Clearing its files after a Cancel, it deletes one more.
        fn clears_one(&self) {
            if let Some(name) = self.clearing.lock().unwrap().pop_front() {
                self.files.lock().unwrap().retain(|(n, _)| *n != name);
            }
        }

        /// Its Cancel lands now: what it deletes, in order.
        fn cancels(&self, list_first: bool) {
            let list = stash_core::phone::LIST.to_string();
            let mut order: Vec<String> = self.left().into_iter().filter(|n| *n != list).collect();
            if list_first {
                order.insert(0, list);
            } else {
                order.push(list);
            }
            *self.clearing.lock().unwrap() = order.into();
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
            self.0.clears_one();
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
            if file.name == stash_core::phone::LIBRARY {
                self.0.library_reads.fetch_add(1, Ordering::Relaxed);
                let unanswered = &self.0.library_unanswered;
                if unanswered.load(Ordering::Relaxed) > 0 {
                    unanswered.fetch_sub(1, Ordering::Relaxed);
                    return Err("the phone did not answer (0x80004005)".to_string());
                }
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
            if let Some(told) = self.0.on_read.lock().unwrap().as_mut() {
                told(&file.name);
            }
            if self.0.withdraws_at.lock().unwrap().take_if(|at| *at == file.name).is_some() {
                self.0.files.lock().unwrap().clear();
            }
            if let Some((_, list_first)) = self.0.cancels_at.lock().unwrap().take_if(|(at, _)| *at == file.name) {
                self.0.cancels(list_first);
                self.0.clears_one();
                return Err("the phone did not answer (0x80070490)".to_string());
            }
            self.0.clears_one();
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
            if self.0.unreadable.lock().unwrap().contains(&file.name) {
                return Err("the phone did not answer (0x80070490)".to_string());
            }
            let bytes = self.bytes(file, 0)?;
            std::fs::write(to, &bytes).map_err(|e| format!("{HERE}{e}"))?;
            #[cfg(windows)]
            if self.0.holds.lock().unwrap().contains(&file.name) {
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
            self.0.removed.lock().unwrap().push(file.name.clone());
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
            let answers = self.answers_again.load(Ordering::Relaxed) && !self.lists_partly.load(Ordering::Relaxed);
            answers.then(|| Open::of(self))
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

        /// One that takes every send whole, its library kept in a file (beside its downloads folder, not in it).
        fn on_disk(name: &str) -> Here {
            let here = Here::new(name);
            let db = here.db();
            for f in [db.clone(), db.with_extension("db-wal"), db.with_extension("db-shm")] {
                let _ = std::fs::remove_file(f);
            }
            let mut library = Library::open(&db).unwrap();
            library
                .set_setting(crate::saving::FOLDER, &here.dir.to_string_lossy())
                .unwrap();
            *here.library.lock().unwrap() = library;
            here.take_all(true);
            here
        }

        /// Where [`Here::on_disk`] keeps its library.
        fn db(&self) -> std::path::PathBuf {
            self.dir.with_extension("db")
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
            choose_from(&self.library, &self.board, sent_at_ms, Choice::These(files), None)
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
            // A library in a file is closed before its file goes.
            if let Ok(library) = self.library.get_mut() {
                *library = Library::open_in_memory().unwrap();
            }
            let db = self.db();
            for f in [db.clone(), db.with_extension("db-wal"), db.with_extension("db-shm")] {
                let _ = std::fs::remove_file(f);
            }
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
        assert_eq!(choose_from(&here.library, &here.board, at, choice, None), Ok(0));
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
        let here = Here::on_disk("dbfull");
        here.library.lock().unwrap().refuse_writes().unwrap();
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav()), ("4.wav", wav())]);
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.done, now.failed), ("cannot_save", 0, 0), "stopped at once, not after LOST_AFTER");
        assert!(tries.tried.is_empty() && tries.held.contains(&7));
        assert!(phone.reads.lock().unwrap().is_empty());
        assert_eq!(phone.left().len(), 5);
        // Said as the library's trouble, on the drive it is on: not the downloads folder's (on another drive, maybe).
        let told = here.told();
        assert_eq!((told["library"].as_bool(), told["folder"].as_str()), (Some(true), None));
        assert_eq!(told["drive"].as_str(), drive_of(&here.db()).as_deref());
        assert!(told["drive"].is_string());
        let why = told["error"].as_str().unwrap();
        assert!(why.contains("readonly") && !why.starts_with(LIBRARY) && !why.contains("database: "), "why: {why}");
        // The downloads folder's trouble is said as before.
        let here = Here::new("dbfull-not");
        std::fs::write(here.dir.join("not a folder"), b"").unwrap();
        here.library
            .lock()
            .unwrap()
            .set_setting(crate::saving::FOLDER, &here.dir.join("not a folder").join("Stash").to_string_lossy())
            .unwrap();
        here.take(&Phone::with(&[("1.wav", wav())]), &mut Tries::default());
        let told = here.told();
        assert_eq!((told["state"].as_str(), told["library"].as_bool(), told["drive"].as_str()), (Some("cannot_save"), Some(false), None));
        assert!(told["folder"].is_string());
    }

    #[test]
    fn the_drive_a_library_is_on_is_named_as_windows_names_it() {
        if cfg!(windows) {
            assert_eq!(drive_of(Path::new(r"C:\Users\someone\AppData\Roaming\Stash\library.db")).as_deref(), Some("C:"));
            assert_eq!(drive_of(Path::new(r"\\?\D:\Stash\library.db")).as_deref(), Some("D:"));
            assert_eq!(drive_of(Path::new(r"\\server\share\Stash\library.db")).as_deref(), Some(r"\\server\share"));
        } else {
            assert_eq!(drive_of(Path::new("/home/someone/.local/share/stash/library.db")).as_deref(), Some("/home/someone/.local/share/stash"));
        }
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
        assert_eq!(choose_from(&here.library, &here.board, at, choice, None), Ok(0));
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
        assert!(in_the_folder(std::io::Error::from_raw_os_error(112)).starts_with(HERE), "a full disk: the drive's");
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
        assert!(in_the_folder(kept).starts_with(ONE_SONG), "held by another program: that song's trouble alone");
    }

    #[cfg(windows)]
    #[test]
    fn songs_whose_copies_another_program_keeps_open_fail_alone_and_the_rest_come_in() {
        let here = Here::new("heldopen");
        let names = ["1.wav", "2.wav", "3.wav", "4.wav", "5.wav"];
        let phone = Phone::with(&names.map(|n| (n, wav())));
        // Held for good, three in a row: none of them is put in place, each failing alone (neither this computer unable
        // to keep songs, nor the phone lost: it is not looked at again for them).
        phone.holds.lock().unwrap().extend(names[..3].iter().map(|n| n.to_string()));
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 2, 3));
        assert_eq!(phone.left(), ["1.wav", "2.wav", "3.wav", stash_core::phone::LIST], "left on the phone, to be tried again");
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), 0);
        assert!(!tries.held.contains(&7));
        // Still held at the next look: the copies made here before are in the way, and those songs fail alone again,
        // not read from the phone again for nothing.
        let reads = phone.reads.lock().unwrap().len();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 2, 3));
        assert_eq!(phone.reads.lock().unwrap().len(), reads);
        // Let go: the next look takes them.
        phone.holds.lock().unwrap().clear();
        phone.held.lock().unwrap().clear();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.cleared), ("done", 5, 0, true));
    }

    #[test]
    fn a_library_another_writer_keeps_for_one_song_fails_that_song_and_the_rest_come_in() {
        let here = Here::on_disk("busy");
        // A write here that finds another writing gives up at once (as one does after its wait, 5 s).
        here.library.lock().unwrap().wait_for_writers(Duration::ZERO).unwrap();
        let scan = Arc::new(Mutex::new(rusqlite::Connection::open(here.db()).unwrap()));
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav()), ("4.wav", wav())]);
        // A scan writes from as 2.wav is read until that song is given up on.
        let writing = Arc::clone(&scan);
        *phone.on_read.lock().unwrap() = Some(Box::new(move |name: &str| {
            if name == "2.wav" {
                writing
                    .lock()
                    .unwrap()
                    .execute_batch("BEGIN IMMEDIATE; INSERT INTO settings(key, value) VALUES ('scan', '1');")
                    .unwrap();
            }
        }));
        let note = |line: &str| {
            if line.starts_with("2.wav was not taken") {
                scan.lock().unwrap().execute_batch("COMMIT").unwrap();
            }
        };
        let edited = || {};
        let ctx = Ctx {
            library: &here.library,
            board: &here.board,
            edited: &edited,
            log: &note,
        };
        let mut tries = Tries::default();
        take(&ctx, &phone, Open::of(&phone), &mut tries, &AtomicBool::new(false));
        let now = here.board.now.lock().unwrap().clone().unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 3, 1), "not stopped, nor held: the rest came in");
        assert_eq!(phone.left(), ["2.wav", stash_core::phone::LIST]);
        assert!(tries.held.is_empty());
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), 0, "the phone is not looked at again for it");
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
            *board.now.lock().unwrap() = Some(Now { list_left: true, done: 5, trying_again: true, ..card(state) });
            nothing_sent(&board, "A", true, false, 0);
            let n = board.now.lock().unwrap().clone().unwrap();
            assert_eq!((n.state, n.list_left, n.cleared, n.trying_again, n.at_ms), ("done", false, true, false, 5), "{state}");
        }
        // A transfer said to have stopped (contact lost, as the phone deleted its copies before its list) whose folder
        // answers with no list: the phone stopped the send.
        *board.now.lock().unwrap() = Some(card("stopped"));
        nothing_sent(&board, "A", true, false, 0);
        let n = board.now.lock().unwrap().clone().unwrap();
        assert_eq!((n.state, n.done, n.total), ("withdrawn", 2, 5));
        assert!(n.at_ms > 5, "news: a card closed shows again");
        // Not answering (a folder that does not open, or one listed only part way, which is no answer): what was said
        // stays, but for a list said to stay plugged in for, which is to be plugged in again.
        for state in ["stopped", "done", "withdrawn", "cannot_save"] {
            *board.now.lock().unwrap() = Some(card(state));
            nothing_sent(&board, "A", false, false, 0);
            assert_eq!(board.now.lock().unwrap().as_ref().map(|n| (n.state, n.at_ms)), Some((state, 5)), "{state}");
        }
        *board.now.lock().unwrap() = Some(Now { list_left: true, done: 5, trying_again: true, ..card("done") });
        nothing_sent(&board, "A", false, false, 0);
        let n = board.now.lock().unwrap().clone().unwrap();
        assert_eq!((n.state, n.list_left, n.trying_again, n.done), ("stopped", true, false, 5));
        assert!(n.at_ms > 5);
        // Songs the computer was to try again by itself can't come in while the phone doesn't answer: plugged in again.
        *board.now.lock().unwrap() = Some(Now { failed: 1, trying_again: true, ..card("done") });
        nothing_sent(&board, "A", false, false, 0);
        let n = board.now.lock().unwrap().clone().unwrap();
        assert_eq!((n.state, n.failed, n.trying_again, n.done), ("stopped", 1, false, 2));
        assert!(n.at_ms > 5);
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
        *board.offer.lock().unwrap() = Some(Offer { phone: "A".into(), sent_at_ms: 7, songs: Vec::new(), asked: Vec::new(), library: None });
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

    /// Ten songs sent at 7.
    fn ten() -> Arc<Phone> {
        let names: Vec<String> = (1..=10).map(|n| format!("{n}.wav")).collect();
        let songs: Vec<(&str, Vec<u8>)> = names.iter().map(|n| (n.as_str(), wav())).collect();
        Phone::with(&songs)
    }

    #[test]
    fn a_send_the_phone_stops_as_it_is_taken_is_said_to_be_stopped_by_the_phone_whatever_it_deletes_first() {
        // Cancel on the phone as 3.wav is read, its files deleted one at a time, its list first (as Stash for Android
        // does): the first read that fails finds the folder with no list.
        let here = Here::new("cancel-list-first");
        let phone = ten();
        *phone.cancels_at.lock().unwrap() = Some(("3.wav".to_string(), true));
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.done, now.failed, now.total), ("withdrawn", 2, 2, 0, 10));
        assert_eq!(here.told()["state"], "withdrawn");

        // Its list last (as earlier builds do): reads fail on copies already gone while the list is still there, so
        // the take ends as contact lost; the next look, its files gone, finds the folder with no list.
        let here = Here::new("cancel-list-last");
        let phone = ten();
        *phone.cancels_at.lock().unwrap() = Some(("3.wav".to_string(), false));
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.done, now.failed), ("stopped", 2, 4, 2));
        assert!(phone.left().contains(&stash_core::phone::LIST.to_string()), "its list not deleted yet");
        while !phone.clearing.lock().unwrap().is_empty() {
            phone.clears_one();
        }
        assert!(phone.left().is_empty());
        nothing_sent(&here.board, "Test phone", true, false, 0);
        let told = here.told();
        assert_eq!(
            (told["state"].as_str(), told["done"].as_u64(), told["failed"].as_u64(), told["total"].as_u64()),
            (Some("withdrawn"), Some(4), Some(2), Some(10))
        );
    }

    #[test]
    fn a_listing_the_phone_stops_part_way_never_says_it_stopped_the_send() {
        // As a song stops sending, the phone opened again lists its folder only part way (which can leave the list
        // out): no answer, not the send stopped.
        let here = Here::new("partly-stall");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        phone.lists_partly.store(true, Ordering::Relaxed);
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken), ("stopped", 1));
        // A look then finding it listing part way does not answer either: said as it was.
        nothing_sent(&here.board, "Test phone", false, false, 0);
        assert_eq!(here.told()["state"], "stopped");
        // A read that fails, the phone listing part way as it is looked at again: the song's, never the send stopped.
        let here = Here::new("partly-read");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.unreadable.lock().unwrap().insert("1.wav".to_string());
        phone.lists_partly.store(true, Ordering::Relaxed);
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 1, 1));
    }

    #[test]
    fn a_phone_whose_list_does_not_answer_keeps_what_was_said_of_its_send() {
        // Every song chosen in, the list left on the phone; then the phone stops answering as its list is read.
        let here = Here::asking("list-unread");
        let phone = three("Test phone");
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        let list = stash_core::phone::LIST.to_string();
        phone.undeletable.lock().unwrap().insert(list.clone(), "the phone did not answer (0x80004005)".to_string());
        let done = here.take(&phone, &mut tries).unwrap();
        assert_eq!((done.state, done.taken, done.list_left), ("done", 1, true));
        phone.unreachable.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(3));
        let now = here.take(&phone, &mut tries).unwrap();
        // To be plugged in again, with what came in.
        assert_eq!((now.state, now.taken, now.list_left, now.sent_at_ms), ("stopped", 1, true, Some(8)));
        assert!(now.at_ms > done.at_ms);
        // It answers: only the list goes, and what came in is said, done.
        phone.unreachable.store(false, Ordering::Relaxed);
        phone.undeletable.lock().unwrap().clear();
        let said = here.said();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.left, now.list_left), ("done", 1, 2, false));
        assert_eq!(here.said(), said, "no taking, no new done: not \"Nothing new came in\"");
        assert_eq!(phone.left(), ["1.wav", "3.wav"]);

        // A transfer being taken is cut short; another phone's card is not this one's to say over; with no card, it
        // is said.
        let board = Board::new();
        let here = Here::new("list-unread-cards");
        let phone = Phone::with(&[("1.wav", wav())]);
        phone.unreachable.store(true, Ordering::Relaxed);
        let card = |phone: &str, state: &'static str| Now {
            state,
            phone: Some(phone.to_string()),
            total: 4,
            done: 2,
            taken: 2,
            sent_at_ms: Some(7),
            at_ms: 5,
            ..Default::default()
        };
        *board.now.lock().unwrap() = Some(card("Test phone", "taking"));
        let n = here.take_on(&board, &phone, &mut Tries::default()).unwrap();
        assert_eq!((n.state, n.done, n.taken, n.sent_at_ms), ("stopped", 2, 2, Some(7)));
        *board.now.lock().unwrap() = Some(card("Other phone", "taking"));
        let n = here.take_on(&board, &phone, &mut Tries::default()).unwrap();
        assert_eq!((n.state, n.phone.as_deref(), n.at_ms), ("taking", Some("Other phone"), 5));
        *board.now.lock().unwrap() = None;
        let n = here.take_on(&board, &phone, &mut Tries::default()).unwrap();
        assert_eq!((n.state, n.phone.as_deref(), n.total), ("stopped", Some("Test phone"), 0));
    }

    #[test]
    fn what_a_send_came_to_is_kept_while_another_phones_card_is_shown() {
        // Phone A's chosen song in, its list left; then phone B's send is offered over A's result.
        let here = Here::asking("kept");
        let a = three("Phone A");
        let mut tries = Tries::default();
        here.take(&a, &mut tries);
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        let list = stash_core::phone::LIST.to_string();
        a.undeletable.lock().unwrap().insert(list, "the phone did not answer (0x80004005)".to_string());
        let done = here.take(&a, &mut tries).unwrap();
        assert_eq!((done.state, done.taken, done.list_left), ("done", 1, true));
        let b = Phone::sending("Phone B", 9, &[("9.wav", wav())], vec![json!({ "file": "9.wav", "title": "B song", "artist": "Someone" })]);
        assert_eq!(here.take(&b, &mut tries).unwrap().phone.as_deref(), Some("Phone B"));
        // A's next look finishes its send: B's card is not written over, and A's send is kept as finished.
        a.undeletable.lock().unwrap().clear();
        let said = here.said();
        let now = here.take(&a, &mut tries).unwrap();
        assert_eq!((now.state, now.phone.as_deref()), ("ready", Some("Phone B")));
        assert_eq!(here.said(), said);
        assert_eq!(a.left(), ["1.wav", "3.wav"]);
        let kept = &tries.ended["Phone A"];
        assert_eq!((kept.state, kept.taken, kept.list_left, kept.at_ms), ("done", 1, false, done.at_ms));

        // A try that carries on a send after another card took its place counts with what came in before.
        let here = Here::new("kept-carry");
        let a = Phone::sending(
            "Phone A",
            7,
            &[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())],
            (1..=3).map(|n| json!({ "file": format!("{n}.wav"), "title": format!("Song {n}"), "artist": "Someone" })).collect(),
        );
        a.stall("2.wav", 1);
        a.unlisted_again.lock().unwrap().insert("3.wav".to_string());
        let mut tries = Tries::default();
        assert_eq!(here.take(&a, &mut tries).unwrap().taken, 2);
        *here.board.now.lock().unwrap() = Some(Now { state: "connected", phone: Some("Phone B".into()), ..Default::default() });
        a.unlisted_again.lock().unwrap().clear();
        let now = here.take(&a, &mut tries).unwrap();
        assert_eq!((now.state, now.phone.as_deref(), now.taken, now.failed, now.cleared), ("done", Some("Phone A"), 3, 0, true));
    }

    #[test]
    fn a_send_finished_with_the_list_gone_tries_no_song_again() {
        // A song left out of the listing after the phone was opened again, and of the next look's too: the list goes,
        // and the card does not say the computer tries it again.
        let here = Here::new("finish-failed");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())]);
        phone.stall("2.wav", 1);
        phone.unlisted_again.lock().unwrap().insert("3.wav".to_string());
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.trying_again), ("done", 2, 1, true));
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.trying_again, now.list_left), ("done", 2, 0, false, false));
        let told = here.told();
        assert_eq!((told["failed"].as_u64(), told["trying_again"].as_bool()), (Some(0), Some(false)));
        assert!(!phone.left().contains(&stash_core::phone::LIST.to_string()));
    }

    #[test]
    fn a_take_is_news_when_what_its_card_asks_of_the_listener_changes() {
        let before = Now {
            state: "done",
            taken: 2,
            failed: 1,
            unlisted: 1,
            trying_again: true,
            at_ms: 5,
            sent_at_ms: Some(7),
            ..Default::default()
        };
        let again = |change: &dyn Fn(&mut Now)| {
            let mut n = Now { before: Some(Before::of(&before)), ..before.clone() };
            change(&mut n);
            n
        };
        assert_eq!(ending(again(&|_| {}), "done").at_ms, 5, "the same news at the same time");
        assert!(ending(again(&|n| n.trying_again = false), "done").at_ms > 5, "no longer tried again by itself");
        assert!(ending(again(&|n| n.taken = 3), "done").at_ms > 5, "more came in");
        assert!(ending(again(&|_| {}), "stopped").at_ms > 5, "ending another way");
        let left = Now { failed: 0, list_left: true, ..before.clone() };
        let again = |change: &dyn Fn(&mut Now)| {
            let mut n = Now { before: Some(Before::of(&left)), ..left.clone() };
            change(&mut n);
            n
        };
        assert!(ending(again(&|n| n.trying_again = false), "done").at_ms > 5, "to be plugged in again now");
        assert!(ending(again(&|n| n.list_left = false), "stopped").at_ms > 5);
        // The list that stayed gone, as the card said it would be: no news.
        assert_eq!(
            ending(again(&|n| {
                n.list_left = false;
                n.trying_again = false;
            }), "done")
            .at_ms,
            5
        );
    }

    #[test]
    fn a_list_that_stays_after_the_last_try_asks_for_the_phone_to_be_plugged_in_again() {
        let here = Here::asking("list-last-try");
        let phone = three("Test phone");
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        assert_eq!(here.choose(8, &["2.wav"]), Ok(1));
        let list = stash_core::phone::LIST.to_string();
        phone.undeletable.lock().unwrap().insert(list, "the phone did not answer (0x80004005)".to_string());
        // A song came off the phone: the computer finishes by itself at the next look.
        let first = here.take(&phone, &mut tries).unwrap();
        assert_eq!((first.state, first.list_left, first.trying_again), ("done", true, true));
        // The looks after it get no further: the same news, until the last.
        for _ in 1..TRIES {
            std::thread::sleep(Duration::from_millis(3));
            let now = here.take(&phone, &mut tries).unwrap();
            assert_eq!((now.state, now.list_left, now.trying_again, now.at_ms), ("done", true, true, first.at_ms));
        }
        std::thread::sleep(Duration::from_millis(3));
        let last = here.take(&phone, &mut tries).unwrap();
        assert_eq!((last.state, last.list_left, last.trying_again), ("done", true, false));
        assert!(last.at_ms > first.at_ms, "news: to be plugged in again");
        assert_eq!(here.told()["trying_again"], false);
        let said = here.said();
        here.take(&phone, &mut tries);
        assert_eq!(here.said(), said, "not tried again until the phone is plugged in again");
        assert!(!tries.ended["Test phone"].trying_again);
    }

    #[test]
    fn a_try_that_brings_nothing_new_keeps_a_closed_card_closed() {
        // A song whose read fails every time: each try after the first carries the send on and brings nothing more in.
        let here = Here::new("quiet");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.unreadable.lock().unwrap().insert("2.wav".to_string());
        let mut tries = Tries::default();
        let first = here.take(&phone, &mut tries).unwrap();
        assert_eq!((first.state, first.taken, first.failed), ("done", 1, 1));
        std::thread::sleep(Duration::from_millis(3));
        // It stops sending too, before its read fails: still nothing new.
        phone.stall("2.wav", 1);
        let from = here.board.times.lock().unwrap().len();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.at_ms), ("done", 1, 1, first.at_ms));
        let times = here.board.times.lock().unwrap()[from..].to_vec();
        assert!(here.board.states.lock().unwrap()[from..].contains(&"stalled"));
        assert!(times.iter().all(|t| *t == first.at_ms), "taking and stalled said at the time before: {times:?}");
        // Read at last: news from the moment it comes in.
        phone.unreadable.lock().unwrap().clear();
        std::thread::sleep(Duration::from_millis(3));
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.cleared), ("done", 2, 0, true));
        assert!(now.at_ms > first.at_ms);
    }

    #[test]
    fn a_song_left_for_stopping_with_nothing_after_it_to_read_ends_the_take_without_opening_the_phone_again() {
        // The phone does not answer after a song stops until it is plugged in again: the last song stops every time.
        let here = Here::new("last-left");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.stall("2.wav", 99);
        phone.answers_again.store(false, Ordering::Relaxed);
        let mut tries = Tries::default();
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "stopped");
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "stopped");
        // The third time it is left: nothing after it is to be read, so the take ends, not waiting on the phone.
        let opened = phone.opened_again.load(Ordering::Relaxed);
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 1, 1));
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), opened);
        assert_eq!(phone.left(), ["2.wav", stash_core::phone::LIST]);

        // A song after it in already, whose copy stayed: not deleted from the phone as it is (closed after the stop),
        // but at the next look.
        let here = Here::new("last-left-undeleted");
        let phone = Phone::with(&[("1.wav", wav()), ("2.wav", wav())]);
        phone.unreadable.lock().unwrap().insert("1.wav".to_string());
        phone.undeletable_times.lock().unwrap().insert("2.wav".to_string(), 1);
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.taken, now.failed, now.undeleted.clone()), (1, 1, vec!["2.wav".to_string()]));
        // 1.wav now stops sending, as it did twice before: it is left.
        phone.unreadable.lock().unwrap().clear();
        phone.stall("1.wav", 99);
        tries.stalls.insert((7, "1.wav".to_string()), STALLS_PER_SONG);
        let opened = phone.opened_again.load(Ordering::Relaxed);
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed), ("done", 1, 1));
        assert_eq!(phone.opened_again.load(Ordering::Relaxed), opened);
        assert_eq!(phone.left(), ["1.wav", "2.wav", stash_core::phone::LIST]);
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.failed, now.undeleted.len()), ("done", 1, 1, 0));
        assert_eq!(phone.left(), ["1.wav", stash_core::phone::LIST]);
    }

    use stash_core::phone::{LIBRARY as LIBRARY_FILE, LIST as LIST_FILE};

    /// A song as the phone's library file names it: its title, its artist, and `more` of it.
    fn item(title: &str, artist: &str, more: serde_json::Value) -> serde_json::Value {
        let mut item = json!({ "title": title, "artist": artist });
        if let Some(more) = more.as_object() {
            item.as_object_mut().unwrap().extend(more.clone());
        }
        item
    }

    /// The phone's library, as sent with its send made at `sent_at_ms`. Liked: a song known by 1.wav's ISRC (under other
    /// words), one by 2.wav's YouTube id, "Song 0.wav" (a file here, once [`had_two`] took it), and "Far Away" (neither:
    /// it streams). Its playlists, in the file's order: a Daily Mix synced from Spotify, its own "Gym", and a YouTube
    /// Music mix with no songs. And two plays.
    fn library_at(sent_at_ms: i64) -> serde_json::Value {
        let art = |url: &str| json!({ "artwork": [{ "url": url }] });
        let by_isrc = item("First of all", "Someone", json!({ "isrc": "USABC1234567", "artwork": [{ "url": "https://art/1" }] }));
        let by_video = item(
            "Song 2 (Official Video)",
            "Someone VEVO",
            json!({ "refs": { "youtube": "dQw4w9WgXcQ" }, "artwork": [{ "url": "https://art/2" }] }),
        );
        let had = item("Song 0.wav", "Someone", art("https://art/0"));
        let far = item("Far Away", "Nobody", art("https://art/far"));
        let three = item("Song 3.wav", "Other", json!({}));
        json!({
            "kind": "stash-web-library", "v": 1, "generator": "Stash for Android", "sent_at_ms": sent_at_ms,
            "likes": [
                { "item": by_isrc, "likedAt": 100 }, { "item": by_video, "likedAt": 101 },
                { "item": had, "likedAt": 102 }, { "item": far, "likedAt": 103 },
            ],
            "playlists": [
                { "id": "app-9", "name": "Daily Mix 1", "from": "spotify", "items": [far, by_isrc, far, by_video, had, three] },
                { "id": "app-2", "name": "Gym", "items": [three, far] },
                { "id": "app-12", "name": "Supermix", "from": "youtube", "items": [] },
            ],
            "history": [{ "item": far, "playedAt": 300 }, { "item": by_isrc, "playedAt": 301 }],
        })
    }

    /// A send of three songs made at 8, with the phone's library ([`library_at`]): 1.wav has an ISRC and 2.wav a
    /// YouTube id, both on the album "First"; 3.wav is on "Second".
    fn sending_library(name: &str) -> Arc<Phone> {
        sending_with(name, library_at(8))
    }

    /// [`sending_library`]'s send, with `library`.
    fn sending_with(name: &str, library: serde_json::Value) -> Arc<Phone> {
        let entries = vec![
            json!({ "file": "1.wav", "title": "Song 1.wav", "artist": "Someone", "album": "First",
                    "isrc": "USABC1234567", "art_url": "https://cover/1" }),
            json!({ "file": "2.wav", "title": "Song 2.wav", "artist": "Someone", "album": "First",
                    "youtube_id": "dQw4w9WgXcQ" }),
            json!({ "file": "3.wav", "title": "Song 3.wav", "artist": "Other", "album": "Second" }),
        ];
        let phone = Phone::sending(name, 8, &[("1.wav", wav()), ("2.wav", wav()), ("3.wav", wav())], entries);
        phone.sends_library(library.to_string().into_bytes());
        phone
    }

    /// [`library_at`]'s library as a phone that says where its likes were made and what its lists are sends it: the
    /// song known by 1.wav's ISRC liked in Stash, the video on YouTube Music, Song 0.wav on Spotify and YouTube Music,
    /// and Far Away not said (as an older phone's). Its lists, in the file's order: the Daily Mix (a Spotify mix), Gym
    /// (its own), the Supermix (a YouTube Music mix), then a SoundCloud radio, a Spotify album, a Stash Mix, another of
    /// its own, a Spotify playlist and a YouTube Music radio, each of these of Far Away.
    fn live_at(sent_at_ms: i64) -> serde_json::Value {
        let mut library = library_at(sent_at_ms);
        for (at, on) in [json!(["stash"]), json!(["youtube"]), json!(["spotify", "youtube"])].into_iter().enumerate() {
            library["likes"][at]["liked_on"] = on;
        }
        library["playlists"][0]["kind"] = json!("mix");
        library["playlists"][2]["kind"] = json!("mix");
        let far = library["playlists"][0]["items"][0].clone();
        let lists = library["playlists"].as_array_mut().unwrap();
        for (id, name, from, kind) in [
            ("soundcloud-77", "Related tracks", Some("soundcloud"), "radio"),
            ("spotify-album-5", "After Hours", Some("spotify"), "album"),
            ("stash-mix-1", "Stash Mix 1", None, "stash_mix"),
            ("custom_7", "Mine", None, "playlist"),
            ("spotify-6", "Road", Some("spotify"), "playlist"),
            ("youtube-radio-1", "Song radio", Some("youtube"), "radio"),
        ] {
            lists.push(json!({ "id": id, "name": name, "from": from, "kind": kind, "items": [far] }));
        }
        library
    }

    /// A library that offers each send, with "Song 0.wav" and "Song 3.wav" taken into it as files (from an earlier send).
    fn had_two(name: &str) -> Here {
        let here = Here::asking(name);
        here.take_all(true);
        let entries = vec![
            json!({ "file": "0.wav", "title": "Song 0.wav", "artist": "Someone" }),
            json!({ "file": "3.wav", "title": "Song 3.wav", "artist": "Other" }),
        ];
        let before = Phone::sending("Test phone", 7, &[("0.wav", wav()), ("3.wav", wav())], entries);
        assert_eq!(here.take(&before, &mut Tries::default()).unwrap().taken, 2);
        here.take_all(false);
        here
    }

    /// What a take asks of the library: every like or none, the plays or not, the playlists chosen, and whether only
    /// the songs here.
    fn parts(likes: bool, plays: bool, playlists: &[&str], only_here: bool) -> library::Asked {
        library::Asked {
            likes: if likes { library::Likes::All } else { library::Likes::None },
            plays,
            playlists: playlists.iter().map(|p| p.to_string()).collect(),
            only_here,
            downloads: true,
        }
    }

    /// `parts` with the likes made on `services` only.
    fn liked_on(services: &[&'static str], playlists: &[&str]) -> library::Asked {
        library::Asked {
            likes: library::Likes::On(services.to_vec()),
            ..parts(false, false, playlists, false)
        }
    }

    /// What is kept for a phone of what was left out, as the setting keeps it and `GET /api/phone/library` says it.
    fn left_out(likes: &[&str], all_likes: bool, plays: bool, playlists: &[&str], only_here: bool) -> serde_json::Value {
        json!({
            "likes_left_out": likes,
            "all_likes_left_out": all_likes,
            "plays": plays,
            "playlists_left_out": playlists,
            "only_here": only_here,
            "downloads": true,
        })
    }

    /// [`left_out`] kept for "Test phone".
    fn keep_left_out(here: &Here, left: serde_json::Value) {
        library::keep(&here.library, "Test phone", &serde_json::from_value(left).unwrap());
    }

    /// The songs liked here, each with whether it is here (a file: it plays with no connection).
    fn liked(here: &Here) -> Vec<(String, bool)> {
        let rows = here.library.lock().unwrap().liked_at(0, 100).unwrap();
        let mut liked: Vec<(String, bool)> = rows.into_iter().map(|t| (t.title, t.here)).collect();
        liked.sort();
        liked
    }

    /// The playlists here, each with its number of songs.
    fn playlists(here: &Here) -> Vec<(String, u64)> {
        let rows = here.library.lock().unwrap().playlists().unwrap();
        rows.into_iter().map(|p| (p.name, p.tracks)).collect()
    }

    /// The songs played, newest first.
    fn plays(here: &Here) -> Vec<String> {
        let rows = here.library.lock().unwrap().history_at(0, 100).unwrap();
        rows.into_iter().map(|t| t.title).collect()
    }

    /// The songs of the library, each with whether it is here.
    fn songs(here: &Here) -> Vec<(String, bool)> {
        let rows = here.library.lock().unwrap().tracks_at(0, 100).unwrap();
        let mut songs: Vec<(String, bool)> = rows.into_iter().map(|t| (t.title, t.here)).collect();
        songs.sort();
        songs
    }

    fn setting(here: &Here, key: &str) -> Option<String> {
        here.library.lock().unwrap().setting(key).unwrap().filter(|v| !v.is_empty())
    }

    /// What `GET /api/phone/library` answers of the send offered now: None for its 404.
    fn summed_up(here: &Here) -> Option<serde_json::Value> {
        let offer = here.board.offer.lock().unwrap().clone()?;
        let from = offer.library.clone()?;
        Some(library::summary(&here.library, &offer, &from))
    }

    fn choose_with(here: &Here, sent_at_ms: i64, choice: Choice, library: library::Asked) -> Result<usize, &'static str> {
        choose_from(&here.library, &here.board, sent_at_ms, choice, Some(library))
    }

    #[test]
    fn a_library_sent_with_a_send_is_offered_with_it_and_summed_up() {
        let here = had_two("library-offer");
        let phone = sending_library("Test phone");
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.total, now.here, now.with_library), ("ready", 3, 1, true));
        assert_eq!(here.told()["with_library"], true);
        let w = wav().len() as u64;
        assert_eq!(
            summed_up(&here).unwrap(),
            json!({
                "sent_at_ms": 8,
                "phone": "Test phone",
                // 3.wav is here already.
                "downloads": { "songs": 2, "bytes": 2 * w, "here": 1 },
                // Downloaded: the songs known by 1.wav's ISRC and 2.wav's YouTube id (sent now), and Song 0.wav (a file
                // here); not Far Away.
                // An older phone's file: it does not say where its likes were made, of its likes or of its files.
                "likes": { "songs": 4, "downloaded": 3, "by": [] },
                "liked_files": null,
                "plays": 2,
                // Its own playlists first, then those synced, Spotify's before YouTube Music's; covers distinct, first
                // songs first, four at most. An older phone's file says no kind. None is here yet.
                "playlists": [
                    { "id": "app-2", "name": "Gym", "from": null, "kind": null, "songs": 2, "downloaded": 1,
                      "covers": ["https://art/far"], "here": false },
                    { "id": "app-9", "name": "Daily Mix 1", "from": "spotify", "kind": null, "songs": 6, "downloaded": 4,
                      "covers": ["https://art/far", "https://art/1", "https://art/2", "https://art/0"], "here": false },
                    { "id": "app-12", "name": "Supermix", "from": "youtube", "kind": null, "songs": 0, "downloaded": 0,
                      "covers": [], "here": false },
                ],
                // From the send's songs, most songs first.
                "albums": [
                    { "title": "First", "artist": "Someone", "art_url": "https://cover/1", "songs": 2 },
                    { "title": "Second", "artist": "Other", "art_url": null, "songs": 1 },
                ],
                "choice": null,
            })
        );
        // Looked at again: offered as it was, its library not read again; nothing read or deleted before it is chosen.
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "ready");
        assert_eq!(phone.library_reads.load(Ordering::Relaxed), 1);
        assert!(phone.reads.lock().unwrap().is_empty() && phone.removed.lock().unwrap().is_empty());
        assert!(songs(&here).iter().all(|(_, file)| *file), "nothing of the library came in yet");
    }

    #[test]
    fn a_library_of_another_send_or_none_that_can_be_read_leaves_the_send_its_songs() {
        let cases: [(&str, Option<Vec<u8>>); 3] = [
            ("library-stale", Some(library_at(5).to_string().into_bytes())),
            ("library-garbage", Some(br#"{"kind":"something else"}"#.to_vec())),
            ("library-none", None),
        ];
        for (name, bytes) in cases {
            let here = Here::asking(name);
            // A choice kept from before brings nothing in from it either.
            keep_left_out(&here, left_out(&[], false, true, &["app-2", "app-12"], false));
            let phone = three("Test phone");
            if let Some(bytes) = bytes {
                phone.sends_library(bytes);
            }
            let mut tries = Tries::default();
            let now = here.take(&phone, &mut tries).unwrap();
            assert_eq!((now.state, now.total, now.with_library), ("ready", 3, false), "{name}");
            assert!(summed_up(&here).is_none(), "{name}: no library for the send waiting");
            assert_eq!(
                choose_with(&here, 8, Choice::All, parts(true, false, &[], false)),
                Err("the phone sent no library with these songs"),
                "{name}"
            );
            // Its songs come in as ever.
            assert_eq!(choose_from(&here.library, &here.board, 8, Choice::All, None), Ok(3));
            let now = here.take(&phone, &mut tries).unwrap();
            assert_eq!((now.state, now.taken, now.cleared), ("done", 3, true), "{name}");
            assert!(phone.left().is_empty() && phone.folder_gone.load(Ordering::Relaxed), "{name}");
            // Only the like the list carries (1.wav's); no plays, no playlists.
            assert_eq!(liked(&here), [("Song 1.wav".to_string(), true)], "{name}");
            assert!(plays(&here).is_empty() && playlists(&here).is_empty(), "{name}");
            assert_eq!(setting(&here, library::BROUGHT_SETTING), None, "{name}");
        }
    }

    #[test]
    fn a_library_the_phone_did_not_answer_for_is_read_again_and_offered_once_it_is() {
        let here = Here::asking("library-late");
        let phone = sending_library("Test phone");
        phone.library_unanswered.store(1, Ordering::Relaxed);
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.total, now.with_library), ("ready", 3, false), "its songs offered meanwhile");
        assert_eq!(here.told()["with_library"], false);
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.total, now.with_library), ("ready", 3, true));
        assert_eq!(here.told()["with_library"], true, "the window is told it came, on the card it shows");
        assert!(summed_up(&here).is_some());
        assert_eq!(phone.library_reads.load(Ordering::Relaxed), 2);
        // One that never answers is let go after a few reads: the send is its songs.
        let here = Here::asking("library-never");
        let phone = sending_library("Test phone");
        phone.library_unanswered.store(99, Ordering::Relaxed);
        let mut tries = Tries::default();
        for _ in 0..5 {
            assert!(!here.take(&phone, &mut tries).unwrap().with_library);
        }
        assert_eq!(phone.library_reads.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn a_send_of_no_songs_with_a_library_is_offered_and_brings_the_library_in() {
        let here = Here::asking("library-only");
        let phone = Phone::sending("Test phone", 8, &[], Vec::new());
        phone.sends_library(library_at(8).to_string().into_bytes());
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.total, now.with_library), ("ready", 0, true));
        let told = summed_up(&here).unwrap();
        assert_eq!(told["downloads"], json!({ "songs": 0, "bytes": 0, "here": 0 }));
        assert_eq!(told["likes"], json!({ "songs": 4, "downloaded": 0, "by": [] }));
        assert_eq!(told["albums"], json!([]));
        assert_eq!(choose_with(&here, 8, Choice::All, parts(true, true, &["app-2"], false)), Ok(0));
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.cleared), ("done", 0, true));
        let liked = liked(&here);
        assert_eq!(liked.len(), 4);
        assert!(liked.iter().all(|(_, file)| !file), "songs not sent come in to stream");
        assert_eq!(plays(&here), ["First of all", "Far Away"]);
        assert_eq!(playlists(&here), [("Gym".to_string(), 2)]);
        assert_eq!(*phone.removed.lock().unwrap(), [LIBRARY_FILE, LIST_FILE]);
        assert!(phone.folder_gone.load(Ordering::Relaxed));

        // The library here can't be written: nothing goes from the phone, and the send waits to be tried again.
        let here = Here::on_disk("library-only-full");
        keep_left_out(&here, left_out(&[], false, false, &["app-9", "app-2", "app-12"], false));
        here.library.lock().unwrap().refuse_writes().unwrap();
        let phone = Phone::sending("Test phone", 8, &[], Vec::new());
        phone.sends_library(library_at(8).to_string().into_bytes());
        let mut tries = Tries::default();
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.library), ("cannot_save", true));
        assert_eq!(phone.left(), [LIBRARY_FILE, LIST_FILE]);
        assert!(tries.held.contains(&8) && tries.tried.is_empty());
    }

    #[test]
    fn a_take_brings_in_the_parts_of_the_library_chosen_before_its_songs() {
        let here = had_two("library-take");
        let phone = sending_library("Test phone");
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        // A playlist that is not in the library: refused, and nothing kept or chosen.
        assert_eq!(
            choose_with(&here, 8, Choice::All, parts(true, false, &["app-404"], false)),
            Err("the playlists on the phone changed: look them over and take them again")
        );
        assert!(here.board.offer.lock().unwrap().is_some());
        assert_eq!(setting(&here, library::CHOICE_SETTING), None);
        let chose = parts(true, false, &["app-9"], false);
        assert_eq!(choose_with(&here, 8, Choice::All, chose.clone()), Ok(3));
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.had, now.cleared), ("done", 2, 1, true));
        // Its likes: 1.wav landed on the song its like made (one song, a file now), and Far Away streams.
        let liked = liked(&here);
        assert_eq!(liked.len(), 4);
        assert!(liked.contains(&("First of all".to_string(), true)), "{liked:?}");
        assert!(liked.contains(&("Far Away".to_string(), false)), "{liked:?}");
        assert!(!songs(&here).iter().any(|(title, _)| title == "Song 1.wav"));
        // Only the playlist chosen (all its songs, Far Away twice), and no plays.
        assert_eq!(playlists(&here), [("Daily Mix 1".to_string(), 6)]);
        assert!(plays(&here).is_empty());
        // The library went from the phone after the songs, before the list; what was left out is kept.
        assert_eq!(*phone.removed.lock().unwrap(), ["1.wav", "2.wav", "3.wav", LIBRARY_FILE, LIST_FILE]);
        assert!(phone.folder_gone.load(Ordering::Relaxed));
        let left = left_out(&[], false, false, &["app-2", "app-12"], false);
        assert_eq!(setting(&here, library::CHOICE_SETTING), Some(json!({ "Test phone": left }).to_string()));
        assert_eq!(setting(&here, library::BROUGHT_SETTING), Some(json!([["Test phone", 8]]).to_string()));
    }

    #[test]
    fn only_the_songs_that_are_files_here_come_in_when_only_those_are_asked_for() {
        let here = had_two("library-only-here");
        let phone = sending_library("Test phone");
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        let files = HashSet::from(["1.wav".to_string()]);
        let chose = parts(true, true, &["app-9", "app-2", "app-12"], true);
        assert_eq!(choose_with(&here, 8, Choice::These(files), chose), Ok(1));
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken), ("done", 1));
        // The songs sent now as files (1.wav's) and those that were files here (Song 0.wav, Song 3.wav): nothing that
        // only streams. 2.wav was not chosen, so neither is the song its YouTube id names.
        assert_eq!(
            liked(&here),
            [("First of all".to_string(), true), ("Song 0.wav".to_string(), true)]
        );
        assert_eq!(plays(&here), ["First of all"]);
        // The Daily Mix with its songs here; Gym with Song 3.wav; the mix that had no songs, made as it is.
        assert_eq!(
            playlists(&here),
            [("Daily Mix 1".to_string(), 3), ("Gym".to_string(), 1), ("Supermix".to_string(), 0)]
        );
        assert!(songs(&here).iter().all(|(_, file)| *file), "{:?}", songs(&here));
    }

    #[test]
    fn the_library_comes_in_once_for_a_send_however_often_it_is_tried() {
        let here = Here::asking("library-once");
        let phone = sending_library("Test phone");
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        assert_eq!(choose_with(&here, 8, Choice::All, parts(true, false, &[], false)), Ok(3));
        // 2.wav stops sending, and the phone does not answer again: the take stops after 1.wav, the library in.
        phone.stall("2.wav", 1);
        phone.answers_again.store(false, Ordering::Relaxed);
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken), ("stopped", 1));
        assert_eq!(liked(&here).len(), 4);
        assert_eq!(setting(&here, library::BROUGHT_SETTING), Some(json!([["Test phone", 8]]).to_string()));
        // Un-liked here meanwhile.
        let far = here.library.lock().unwrap().liked_at(0, 100).unwrap().into_iter().find(|t| t.title == "Far Away");
        here.library.lock().unwrap().set_liked(far.unwrap().id, false, 1).unwrap();
        // Plugged in again, and Stash started again too: the rest of the songs come in, the library does not come in
        // again (the like stays gone), and it is not read from the phone again.
        phone.answers_again.store(true, Ordering::Relaxed);
        let now = here.take_on(&Board::new(), &phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.cleared), ("done", 3, true));
        assert_eq!(liked(&here).len(), 3);
        assert_eq!(phone.library_reads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn the_library_goes_from_the_phone_before_the_list_however_the_send_ends() {
        // Nothing chosen: the library and the list go, and the songs stay for the phone to clear.
        let here = Here::asking("library-none-chosen");
        let phone = sending_library("Test phone");
        here.take(&phone, &mut Tries::default());
        let (at, choice) = chosen(&json!({ "sent_at_ms": 8, "files": [], "none": true })).unwrap();
        assert_eq!(choose_from(&here.library, &here.board, at, choice, None), Ok(0));
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.left), ("done", 3));
        assert_eq!(*phone.removed.lock().unwrap(), [LIBRARY_FILE, LIST_FILE]);
        assert_eq!(phone.left(), ["1.wav", "2.wav", "3.wav"]);
        // A send an earlier Stash took: its library goes, then its list, and nothing is read.
        let here = Here::new("library-taken-before");
        here.library.lock().unwrap().set_setting(TAKEN_SETTING, "8").unwrap();
        let phone = sending_library("Test phone");
        here.take(&phone, &mut Tries::default());
        assert_eq!(*phone.removed.lock().unwrap(), [LIBRARY_FILE, LIST_FILE]);
        assert!(phone.reads.lock().unwrap().is_empty());
        // One the phone does not let go of is its to clear: the list goes all the same, and the folder stays.
        let here = Here::new("library-stays");
        let phone = sending_library("Test phone");
        let no = "the phone did not answer (0x80004005)".to_string();
        phone.undeletable.lock().unwrap().insert(LIBRARY_FILE.to_string(), no);
        let now = here.take(&phone, &mut Tries::default()).unwrap();
        assert_eq!((now.state, now.taken, now.list_left, now.cleared), ("done", 3, false, true));
        assert_eq!(phone.left(), [LIBRARY_FILE]);
        assert!(!phone.folder_gone.load(Ordering::Relaxed));
    }

    #[test]
    fn a_later_send_brings_in_all_but_what_was_left_out_and_the_lists_the_phone_gained() {
        let here = Here::asking("library-later");
        let first = sending_library("Test phone");
        let mut tries = Tries::default();
        here.take(&first, &mut tries);
        assert_eq!(choose_with(&here, 8, Choice::All, parts(true, true, &["app-9"], false)), Ok(3));
        here.take(&first, &mut tries);
        assert_eq!(playlists(&here), [("Daily Mix 1".to_string(), 6)]);
        // The phone sends again (at 9): a song, and its library since: a new like, a song more on the Daily Mix, a new
        // Daily Mix, a new play.
        let mut library = library_at(9);
        let new = item("New Love", "Someone", json!({}));
        library["likes"].as_array_mut().unwrap().push(json!({ "item": new, "likedAt": 104 }));
        library["playlists"][0]["items"].as_array_mut().unwrap().push(new.clone());
        library["playlists"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "id": "app-20", "name": "Daily Mix 2", "from": "spotify", "items": [new] }));
        library["history"].as_array_mut().unwrap().push(json!({ "item": new, "playedAt": 302 }));
        let again = Phone::sending(
            "Test phone",
            9,
            &[("4.wav", wav())],
            vec![json!({ "file": "4.wav", "title": "Song 4", "artist": "Someone" })],
        );
        again.sends_library(library.to_string().into_bytes());
        // Offered as ever, with what was left out before.
        assert_eq!(here.take(&again, &mut tries).unwrap().state, "ready");
        assert_eq!(summed_up(&here).unwrap()["choice"], left_out(&[], false, true, &["app-2", "app-12"], false));
        // Its song chosen, with nothing said of the library: all of it but what was left out comes in, in step, the
        // new Daily Mix too. Gym and the Supermix stay out.
        assert_eq!(here.choose(9, &["4.wav"]), Ok(1));
        let now = here.take(&again, &mut tries).unwrap();
        assert_eq!((now.state, now.taken), ("done", 1));
        assert!(liked(&here).contains(&("New Love".to_string(), false)));
        assert_eq!(playlists(&here), [("Daily Mix 1".to_string(), 7), ("Daily Mix 2".to_string(), 1)]);
        assert_eq!(plays(&here), ["New Love", "First of all", "Far Away"]);
        assert_eq!(*again.removed.lock().unwrap(), ["4.wav", LIBRARY_FILE, LIST_FILE]);
        assert_eq!(setting(&here, library::BROUGHT_SETTING), Some(json!([["Test phone", 8], ["Test phone", 9]]).to_string()));
    }

    #[test]
    fn what_is_asked_of_the_library_is_read_as_meant() {
        assert_eq!(library::asked(&json!({ "sent_at_ms": 8, "all": true })), Ok(None));
        assert_eq!(library::asked(&json!({ "library": null })), Ok(None));
        assert_eq!(
            library::asked(&json!({ "library": { "likes": true, "playlists": ["b", "a", "b", 3] } })),
            Ok(Some(parts(true, false, &["a", "b"], false)))
        );
        assert_eq!(library::asked(&json!({ "library": {} })), Ok(Some(parts(false, false, &[], false))));
        // Likes by the services they were made on: each once, in their order.
        assert_eq!(
            library::asked(&json!({ "library": { "likes": ["youtube", "stash", "youtube"] } })),
            Ok(Some(liked_on(&["stash", "youtube"], &[])))
        );
        assert!(library::asked(&json!({ "library": true })).is_err());
        assert!(library::asked(&json!({ "library": { "likes": "yes" } })).is_err());
        assert!(library::asked(&json!({ "library": { "likes": ["tidal"] } })).is_err());
        assert!(library::asked(&json!({ "library": { "likes": [1] } })).is_err());
        assert!(library::asked(&json!({ "library": { "plays": "yes" } })).is_err());
        assert!(library::asked(&json!({ "library": { "playlists": "app-2" } })).is_err());
    }

    #[test]
    fn the_summary_says_where_likes_were_made_and_orders_the_lists_by_where_they_came_from() {
        let here = had_two("library-live-summary");
        let phone = sending_with("Test phone", live_at(8));
        here.take(&phone, &mut Tries::default());
        let told = summed_up(&here).unwrap();
        // Downloaded as ever: all but Far Away. Far Away says nowhere, so it counts as liked in Stash, as a take counts
        // it; SoundCloud has none.
        assert_eq!(
            told["likes"],
            json!({
                "songs": 4,
                "downloaded": 3,
                "by": [
                    { "service": "stash", "songs": 2, "downloaded": 1 },
                    { "service": "spotify", "songs": 1, "downloaded": 1 },
                    { "service": "youtube", "songs": 2, "downloaded": 2 },
                ],
            })
        );
        // The send's files liked: 1.wav (the ISRC liked in Stash) and 2.wav (the video liked on YouTube Music).
        assert_eq!(told["liked_files"], json!({ "1.wav": ["stash"], "2.wav": ["youtube"] }));
        let lists: Vec<(String, serde_json::Value, serde_json::Value)> = told["playlists"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["id"].as_str().unwrap().to_string(), p["from"].clone(), p["kind"].clone()))
            .collect();
        let at = |id: &str, from: Option<&str>, kind: Option<&str>| (id.to_string(), json!(from), json!(kind));
        assert_eq!(
            lists,
            [
                // Its own (one of an older phone, saying no kind), then its Stash Mixes.
                at("app-2", None, None),
                at("custom_7", None, Some("playlist")),
                at("stash-mix-1", None, Some("stash_mix")),
                // Spotify's: playlists, albums, mixes; then YouTube Music's; then SoundCloud's.
                at("spotify-6", Some("spotify"), Some("playlist")),
                at("spotify-album-5", Some("spotify"), Some("album")),
                at("app-9", Some("spotify"), Some("mix")),
                at("app-12", Some("youtube"), Some("mix")),
                at("youtube-radio-1", Some("youtube"), Some("radio")),
                at("soundcloud-77", Some("soundcloud"), Some("radio")),
            ]
        );
    }

    #[test]
    fn likes_chosen_by_service_come_in_from_there_and_from_services_new_since() {
        let here = had_two("library-likes-by-service");
        let phone = sending_with("Test phone", live_at(8));
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        assert_eq!(choose_with(&here, 8, Choice::All, liked_on(&["spotify"], &[])), Ok(3));
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "done");
        // Song 0.wav (liked on Spotify and YouTube Music) only: not the one liked in Stash, nor the video, nor Far Away
        // (it says nowhere, as if liked in Stash).
        assert_eq!(liked(&here), [("Song 0.wav".to_string(), true)]);
        // Kept: the services left out of those its likes were made on, and every list (none was chosen).
        let mut left = left_out(&["stash", "youtube"], false, false, &[], false);
        let every: Vec<serde_json::Value> =
            live_at(8)["playlists"].as_array().unwrap().iter().map(|p| p["id"].clone()).collect();
        left["playlists_left_out"] = json!(every);
        assert!(library::choice_kept(&here.library, "Test phone"));
        assert_eq!(setting(&here, library::CHOICE_SETTING), Some(json!({ "Test phone": left }).to_string()));
        // A later send: liked since on SoundCloud (new here), on YouTube Music (left out), in Stash and on Spotify.
        // The list's own like (Song 4) does not come in: it does not say where it was made.
        let mut later = live_at(9);
        let likes = later["likes"].as_array_mut().unwrap();
        for (title, on) in [("Cloud", json!(["soundcloud"])), ("Tube", json!(["youtube"])), ("Both", json!(["stash", "spotify"]))] {
            likes.push(json!({ "item": item(title, "Someone", json!({})), "likedAt": 110, "liked_on": on }));
        }
        let again = one_more("Test phone", 9, true, &later);
        here.take(&again, &mut tries);
        assert_eq!(here.choose(9, &["4.wav"]), Ok(1));
        assert_eq!(here.take(&again, &mut tries).unwrap().state, "done");
        let titles: Vec<String> = liked(&here).into_iter().map(|(title, _)| title).collect();
        assert_eq!(titles, ["Both", "Cloud", "Song 0.wav"]);
        assert!(playlists(&here).is_empty(), "every list was left out, and none is new");

        // Likes made in Stash: those that say so, and those that say nowhere.
        let here = had_two("library-likes-in-stash");
        let phone = sending_with("Test phone", live_at(8));
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        assert_eq!(choose_with(&here, 8, Choice::All, liked_on(&["stash"], &["custom_7"])), Ok(3));
        here.take(&phone, &mut tries);
        let titles: Vec<String> = liked(&here).into_iter().map(|(title, _)| title).collect();
        assert_eq!(titles, ["Far Away", "First of all"]);
        assert_eq!(playlists(&here), [("Mine".to_string(), 1)]);
    }

    #[test]
    fn likes_and_plays_a_phone_had_none_of_are_not_kept_out_when_it_makes_some() {
        let here = had_two("library-none-yet");
        let mut bare = live_at(8);
        bare["likes"] = json!([]);
        bare["history"] = json!([]);
        let phone = sending_with("Test phone", bare.clone());
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        // Nothing to tick of either, so a window sends them false.
        assert_eq!(choose_with(&here, 8, Choice::All, parts(false, false, &[], false)), Ok(3));
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "done");
        let every: Vec<&str> = bare["playlists"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        let left = left_out(&[], false, true, &every, false);
        assert_eq!(setting(&here, library::CHOICE_SETTING), Some(json!({ "Test phone": left }).to_string()));
        // Liked and played since: they come in.
        let again = one_more("Test phone", 9, true, &live_at(9));
        here.take(&again, &mut tries);
        assert_eq!(summed_up(&here).unwrap()["choice"], left);
        assert_eq!(here.choose(9, &["4.wav"]), Ok(1));
        assert_eq!(here.take(&again, &mut tries).unwrap().state, "done");
        let titles: Vec<String> = liked(&here).into_iter().map(|(title, _)| title).collect();
        assert_eq!(titles.len(), 5, "the four likes and the send's own: {titles:?}");
    }

    #[test]
    fn the_summary_says_which_lists_are_here_already() {
        let here = had_two("library-lists-here");
        let phone = sending_with("Test phone", live_at(8));
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        let none: Vec<bool> = summed_up(&here).unwrap()["playlists"].as_array().unwrap().iter().map(|p| p["here"].as_bool().unwrap()).collect();
        assert!(none.iter().all(|h| !h), "nothing is here before the first bring-in");
        assert!(choose_with(&here, 8, Choice::All, parts(true, true, &["custom_7", "spotify-6"], false)).is_ok());
        here.take(&phone, &mut tries);
        let again = one_more("Test phone", 9, true, &live_at(9));
        here.take(&again, &mut tries);
        let mut said: Vec<String> = summed_up(&here).unwrap()["playlists"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["here"] == true)
            .map(|p| p["id"].as_str().unwrap().to_string())
            .collect();
        said.sort();
        assert_eq!(said, ["custom_7", "spotify-6"]);
    }

    #[test]
    fn all_your_downloads_left_out_is_kept_and_a_choice_kept_before_it_brings_them() {
        let here = had_two("library-keep-downloads");
        let phone = sending_with("Test phone", live_at(8));
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        let asked = library::Asked { downloads: false, ..parts(true, true, &[], false) };
        assert!(choose_with(&here, 8, Choice::All, asked).is_ok());
        here.take(&phone, &mut tries);
        let kept: serde_json::Value = serde_json::from_str(&setting(&here, library::CHOICE_SETTING).unwrap()).unwrap();
        assert_eq!(kept["Test phone"]["downloads"], false);
        // A choice kept before All your downloads was: as if it was ticked.
        let mut old = left_out(&[], false, true, &[], false);
        old.as_object_mut().unwrap().remove("downloads");
        keep_left_out(&here, old);
        let again = one_more("Test phone", 9, true, &live_at(9));
        here.take(&again, &mut tries);
        assert_eq!(summed_up(&here).unwrap()["choice"]["downloads"], true);
    }

    #[test]
    fn a_choice_kept_in_the_earlier_shape_is_no_choice() {
        let here = Here::asking("library-old-choice");
        let old = json!({ "Test phone": { "likes": true, "plays": true, "playlists": ["app-9"], "only_here": false } });
        here.library.lock().unwrap().set_setting(library::CHOICE_SETTING, &old.to_string()).unwrap();
        assert!(!library::choice_kept(&here.library, "Test phone"));
        let phone = one_more("Test phone", 9, true, &library_at(9));
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        assert_eq!(summed_up(&here).unwrap()["choice"], serde_json::Value::Null);
        // Its song taken with nothing said of the library: none of it comes in, and the list's like does, as ever.
        assert_eq!(here.choose(9, &["4.wav"]), Ok(1));
        assert_eq!(here.take(&phone, &mut tries).unwrap().state, "done");
        assert_eq!(liked(&here), [("Song 4".to_string(), true)]);
        assert!(playlists(&here).is_empty() && plays(&here).is_empty());
        assert_eq!(phone.library_reads.load(Ordering::Relaxed), 1, "read once, to offer it");
    }

    /// The songs of the playlist here named `name`, in its order.
    fn playlist_songs(here: &Here, name: &str) -> Vec<String> {
        let library = here.library.lock().unwrap();
        let id = library.playlists().unwrap().into_iter().find(|p| p.name == name).unwrap().id;
        library.playlist_tracks_at(id, 0, 100).unwrap().into_iter().map(|t| t.title).collect()
    }

    /// A send of one song (4.wav, "Song 4", liked on the phone when `liked`) made at `sent_at_ms` on `phone`, with
    /// `library`.
    fn one_more(phone: &str, sent_at_ms: i64, liked: bool, library: &serde_json::Value) -> Arc<Phone> {
        let entry = json!({ "file": "4.wav", "title": "Song 4", "artist": "Someone", "liked": liked });
        let phone = Phone::sending(phone, sent_at_ms, &[("4.wav", wav())], vec![entry]);
        phone.sends_library(library.to_string().into_bytes());
        phone
    }

    /// The phone's library at `sent_at_ms` since [`library_at`]: Far Away and the rest no longer liked, New Love liked;
    /// the Daily Mix now New Love and Song 3.wav; Gym now New Love only.
    fn rotated_at(sent_at_ms: i64) -> serde_json::Value {
        let new = item("New Love", "Someone", json!({}));
        let three = item("Song 3.wav", "Other", json!({}));
        let mut library = library_at(sent_at_ms);
        library["likes"] = json!([{ "item": new, "likedAt": 104 }]);
        library["playlists"][0]["items"] = json!([new, three]);
        library["playlists"][1]["items"] = json!([new]);
        library
    }

    #[test]
    fn a_choice_kept_for_one_phone_is_not_brought_in_from_another() {
        let here = Here::asking("library-two-phones");
        let a = sending_library("Test phone");
        let mut tries = Tries::default();
        here.take(&a, &mut tries);
        assert_eq!(choose_with(&here, 8, Choice::All, parts(true, true, &["app-9"], false)), Ok(3));
        here.take(&a, &mut tries);
        let (playlists_a, liked_a, plays_a) = (playlists(&here), liked(&here), plays(&here));
        // Another phone, whose app-9 is another list: nothing was chosen for it, so nothing of its library comes in,
        // and the likes its list marks come in as they always did.
        let mut theirs = rotated_at(9);
        theirs["playlists"][0]["name"] = json!("Their Daily Mix");
        let b = one_more("Other phone", 9, true, &theirs);
        assert_eq!(here.take(&b, &mut tries).unwrap().state, "ready");
        assert_eq!(summed_up(&here).unwrap()["choice"], serde_json::Value::Null);
        assert!(library::choice_kept(&here.library, "Test phone"));
        assert!(!library::choice_kept(&here.library, "Other phone"));
        assert_eq!(here.choose(9, &["4.wav"]), Ok(1));
        assert_eq!(here.take(&b, &mut tries).unwrap().state, "done");
        assert_eq!(playlists(&here), playlists_a);
        assert_eq!(plays(&here), plays_a);
        let mut wanted = liked_a;
        wanted.push(("Song 4".to_string(), true));
        wanted.sort();
        assert_eq!(liked(&here), wanted);
    }

    #[test]
    fn a_kept_choice_follows_a_playlist_by_its_id_whatever_the_id_is() {
        // Ids the phone keeps the same from any Stash on it: a list synced from Spotify or YouTube Music by its id
        // there, its own by its id in the app.
        let (mix, own, supermix) = (
            "spotify-37i9dQZF1E39vTG3GurFPW",
            "custom_550e8400-e29b-41d4-a716-446655440000",
            "youtube-RDCLAK5uy_kmPRjHDECIcuVwnKsx2Ng7fyNgFKWNJFs",
        );
        let stable = |mut library: serde_json::Value| {
            for (at, id) in [mix, own, supermix].into_iter().enumerate() {
                library["playlists"][at]["id"] = json!(id);
            }
            library
        };
        let here = Here::asking("library-stable-ids");
        let phone = Phone::sending("Test phone", 8, &[], Vec::new());
        phone.sends_library(stable(library_at(8)).to_string().into_bytes());
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        let ids: Vec<serde_json::Value> =
            summed_up(&here).unwrap()["playlists"].as_array().unwrap().iter().map(|p| p["id"].clone()).collect();
        assert_eq!(ids, [json!(own), json!(mix), json!(supermix)]);
        assert_eq!(choose_with(&here, 8, Choice::All, parts(false, false, &[mix, own], false)), Ok(0));
        here.take(&phone, &mut tries);
        assert_eq!(playlists(&here), [("Daily Mix 1".to_string(), 6), ("Gym".to_string(), 2)]);
        // The other Stash on the same phone sends (same name, the same lists by the same ids): the choice holds.
        let other = one_more("Test phone", 9, false, &stable(rotated_at(9)));
        here.take(&other, &mut tries);
        assert_eq!(here.choose(9, &["4.wav"]), Ok(1));
        assert_eq!(here.take(&other, &mut tries).unwrap().state, "done");
        assert_eq!(playlist_songs(&here, "Daily Mix 1"), ["New Love", "Song 3.wav"]);
        assert_eq!(playlist_songs(&here, "Gym"), ["Song 3.wav", "Far Away", "New Love"]);
    }

    #[test]
    fn a_send_brought_in_is_not_brought_in_again_after_another_phones_was() {
        let here = Here::asking("library-brought");
        let a = sending_library("Test phone");
        let mut tries = Tries::default();
        here.take(&a, &mut tries);
        assert_eq!(choose_with(&here, 8, Choice::All, parts(true, false, &[], false)), Ok(3));
        // A's take stops after 1.wav, its library in.
        a.stall("2.wav", 1);
        a.answers_again.store(false, Ordering::Relaxed);
        assert_eq!(here.take(&a, &mut tries).unwrap().state, "stopped");
        // Another phone's send comes in meanwhile, library and all.
        let b = one_more("Other phone", 9, false, &library_at(9));
        here.take(&b, &mut tries);
        assert_eq!(choose_with(&here, 9, Choice::All, parts(true, false, &[], false)), Ok(1));
        assert_eq!(here.take(&b, &mut tries).unwrap().state, "done");
        assert_eq!(
            setting(&here, library::BROUGHT_SETTING),
            Some(json!([["Test phone", 8], ["Other phone", 9]]).to_string())
        );
        // Un-liked here; then A's send is tried again: its library does not come in again.
        let far = here.library.lock().unwrap().liked_at(0, 100).unwrap().into_iter().find(|t| t.title == "Far Away");
        here.library.lock().unwrap().set_liked(far.unwrap().id, false, 1).unwrap();
        a.answers_again.store(true, Ordering::Relaxed);
        assert_eq!(here.take(&a, &mut tries).unwrap().state, "done");
        assert!(!liked(&here).iter().any(|(title, _)| title == "Far Away"));
        // The last 50 sends are kept, each once.
        let mut library = Library::open_in_memory().unwrap();
        for at in 0..60 {
            library::mark_brought(&mut library, "P", at).unwrap();
        }
        library::mark_brought(&mut library, "P", 30).unwrap();
        let kept: Vec<(String, i64)> = serde_json::from_str(&library.setting(library::BROUGHT_SETTING).unwrap().unwrap()).unwrap();
        assert_eq!(kept.len(), 50);
        assert_eq!((kept[0].1, kept[49].1), (10, 30));
    }

    #[test]
    fn a_later_send_mirrors_the_playlists_synced_and_only_adds_to_its_own_and_to_the_likes() {
        let here = Here::asking("library-mirror");
        let first = sending_library("Test phone");
        let mut tries = Tries::default();
        here.take(&first, &mut tries);
        assert_eq!(choose_with(&here, 8, Choice::All, parts(true, false, &["app-9", "app-2"], false)), Ok(3));
        here.take(&first, &mut tries);
        assert_eq!(playlist_songs(&here, "Gym"), ["Song 3.wav", "Far Away"]);
        let liked_first = liked(&here);
        // The phone since: its Daily Mix changed its songs, its Gym lost both and gained New Love, and its likes are New
        // Love alone.
        let later = one_more("Test phone", 9, false, &rotated_at(9));
        here.take(&later, &mut tries);
        assert_eq!(here.choose(9, &["4.wav"]), Ok(1));
        assert_eq!(here.take(&later, &mut tries).unwrap().state, "done");
        assert_eq!(playlist_songs(&here, "Daily Mix 1"), ["New Love", "Song 3.wav"], "as the phone has it now");
        assert_eq!(playlist_songs(&here, "Gym"), ["Song 3.wav", "Far Away", "New Love"], "its own: only gains");
        let liked = liked(&here);
        assert!(liked_first.iter().all(|song| liked.contains(song)), "never un-liked here");
        assert!(liked.contains(&("New Love".to_string(), false)));

        // Only the songs here asked for: a synced list whose songs are none of them here now is emptied, as the phone
        // has none of them that are.
        let here = Here::asking("library-mirror-only-here");
        let first = sending_library("Test phone");
        let mut tries = Tries::default();
        here.take(&first, &mut tries);
        assert_eq!(choose_with(&here, 8, Choice::All, parts(false, false, &["app-9"], true)), Ok(3));
        here.take(&first, &mut tries);
        // The songs the send's files are: First of all (1.wav's ISRC), the video (2.wav's id), Song 3.wav.
        assert_eq!(playlist_songs(&here, "Daily Mix 1").len(), 3);
        let mut far_only = library_at(9);
        far_only["playlists"][0]["items"] = json!([item("Far Away", "Nobody", json!({}))]);
        let later = one_more("Test phone", 9, false, &far_only);
        here.take(&later, &mut tries);
        assert_eq!(here.choose(9, &["4.wav"]), Ok(1));
        here.take(&later, &mut tries);
        assert_eq!(playlists(&here), [("Daily Mix 1".to_string(), 0)]);
    }

    #[test]
    fn on_the_first_send_a_synced_playlist_here_already_is_made_what_the_phone_has() {
        let here = Here::asking("library-mirror-first");
        // Here already from another file (or the other Stash on the phone): the Daily Mix and Gym, by the same ids.
        let before = json!({ "kind": "stash-web-library", "v": 1, "playlists": [
            { "id": "app-9", "name": "Daily Mix 1", "items": [{ "title": "Old Song", "artist": "Someone" }] },
            { "id": "app-2", "name": "Gym", "items": [{ "title": "Old Song", "artist": "Someone" }] } ] });
        let incoming = import::web_library::read(before.to_string().as_bytes(), 1).unwrap();
        here.library.lock().unwrap().import(&incoming).unwrap();
        let phone = sending_library("Test phone");
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        assert_eq!(choose_with(&here, 8, Choice::All, parts(false, false, &["app-9", "app-2"], false)), Ok(3));
        here.take(&phone, &mut tries);
        assert_eq!(
            playlist_songs(&here, "Daily Mix 1"),
            ["Far Away", "First of all", "Far Away", "Song 2 (Official Video)", "Song 0.wav", "Song 3.wav"]
        );
        assert_eq!(playlist_songs(&here, "Gym"), ["Old Song", "Song 3.wav", "Far Away"]);
    }

    #[test]
    fn a_library_brought_in_without_its_likes_brings_no_like_with_the_songs() {
        let here = Here::asking("library-no-likes");
        here.take_all(true);
        assert_eq!(here.take(&Phone::with(&[("1.wav", wav())]), &mut Tries::default()).unwrap().taken, 1);
        here.take_all(false);
        // 1.wav, here already, is liked on the phone; only 2.wav is chosen, and the library without its likes.
        let phone = three("Test phone");
        phone.sends_library(library_at(8).to_string().into_bytes());
        let mut tries = Tries::default();
        here.take(&phone, &mut tries);
        assert!(here.board.offer.lock().unwrap().as_ref().unwrap().songs[0].liked);
        let files = HashSet::from(["2.wav".to_string()]);
        assert_eq!(choose_with(&here, 8, Choice::These(files), parts(false, true, &[], false)), Ok(1));
        let now = here.take(&phone, &mut tries).unwrap();
        assert_eq!((now.state, now.taken, now.had), ("done", 1, 1));
        assert!(phone.removed.lock().unwrap().contains(&"1.wav".to_string()), "its copy went all the same");
        assert!(liked(&here).is_empty());
        // Taken with the choice kept (no likes): a song the list marks liked comes in, not liked.
        let later = one_more("Test phone", 9, true, &library_at(9));
        here.take(&later, &mut tries);
        assert_eq!(here.choose(9, &["4.wav"]), Ok(1));
        assert_eq!(here.take(&later, &mut tries).unwrap().taken, 1);
        assert!(liked(&here).is_empty());
        // A phone nothing was chosen for: its list's likes come in, as ever.
        let other = Phone::sending(
            "Other phone",
            10,
            &[("5.wav", wav())],
            vec![json!({ "file": "5.wav", "title": "Song 5", "artist": "Someone", "liked": true })],
        );
        other.sends_library(library_at(10).to_string().into_bytes());
        here.take(&other, &mut tries);
        assert_eq!(here.choose(10, &["5.wav"]), Ok(1));
        assert_eq!(here.take(&other, &mut tries).unwrap().taken, 1);
        assert_eq!(liked(&here), [("Song 5".to_string(), true)]);
    }
}
