//! Songs from Stash for Android over USB. "Send to computer" on the phone leaves its downloads in
//! `Download/Stash transfer/` with `stash-transfer.json` saying which song each file is; with the phone plugged in
//! ("File transfer"), this takes them into the library as downloaded songs and deletes the folder from the phone.
//!
//! Each song is found in the library as an import finds it (its YouTube or Spotify id, its ISRC, its words), or added;
//! its file goes into the downloads folder where Stash would have saved it, and its download is recorded as saved. A
//! song the library has as a file already is left as it is, and not read from the phone. A song liked on the phone is
//! liked here too.
//!
//! A phone's File transfer can stop mid-song and not start again by itself: a song that sends nothing for a while has
//! its read called off, and the phone is opened again and the song taken from the start (see
//! [`stash_core::phone::STALL`]). What each transfer did goes into `phone.log` beside the library.
//!
//! - `GET /api/phone`: what is happening: nothing, songs being taken from a phone (how many of how many), or what the
//!   last transfer did; and the phones plugged in now (with "File transfer" chosen), each with whether it has songs
//!   sent.
//! - `POST /api/phone/look`: looks at the phones now, and tries again a send that had songs not taken; answers the
//!   look's number, which `answered` in `GET /api/phone` reaches once it is done.
use std::collections::HashMap;
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

use crate::serve::{Reply, State, now_ms, reply_json};

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
    /// `ready` so far), "taking", "stalled" (the phone stopped sending mid-song: it is being asked again), "done", or
    /// "stopped" (unplugged while taking, or not answering again after stalling).
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
    /// The phone's folder was deleted.
    cleared: bool,
    at_ms: i64,
    error: Option<String>,
}

/// What the window is told, written by the transfer as it goes (and, for a song stopping, by the thread watching
/// its read).
struct Board {
    now: Mutex<Option<Now>>,
    #[cfg(test)]
    states: Mutex<Vec<&'static str>>,
}

impl Board {
    const fn new() -> Self {
        Board {
            now: Mutex::new(None),
            #[cfg(test)]
            states: Mutex::new(Vec::new()),
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
            // Listed but not answering (its USB connection dropped): said once, as a transfer cut short.
            if lost(&why) {
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
    let tries = tried.get(&list.sent_at_ms).copied().unwrap_or(0);
    if tries >= TRIES {
        return;
    }
    tried.insert(list.sent_at_ms, tries + 1);
    let now = take_songs(ctx, phone, folder, &list, stop);
    if now.taken + now.had + now.not_kept > 0 {
        tried.remove(&list.sent_at_ms);
    }
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

/// Takes the songs of `list` still on the phone, one at a time, each deleted from the phone once taken. A song that
/// stops coming has its read called off: the phone is opened again and the song taken again from its start (a song
/// that stops [`STALLS_PER_SONG`] times is left, and the rest go on). What the transfer came to.
fn take_songs(
    ctx: &Ctx,
    phone: &dyn Plugged,
    folder: Box<dyn Folder>,
    list: &List,
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
    // Songs whose file is gone were taken by an earlier try (each file is deleted from the phone once taken).
    let total = list.songs.len();
    let here: Vec<&Sent> = list
        .songs
        .iter()
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
        match folder.as_deref().map(|f| f.remove()) {
            Some(Ok(())) => now.cleared = true,
            Some(Err(e)) => {
                (ctx.log)(&format!("the transfer folder stayed on the phone: {e}"));
                let _ = ctx
                    .library
                    .lock()
                    .unwrap()
                    .set_setting(TAKEN_SETTING, &list.sent_at_ms.to_string());
            }
            None => {}
        }
    }
    now.state = "done";
    now.song = None;
    now.at_ms = now_ms();
    (ctx.log)(&format!(
        "{} taken, {} here already, {} not kept (WebM), {} not taken, in {:.0} s",
        now.taken,
        now.had,
        now.not_kept,
        now.failed,
        started.elapsed().as_secs_f64()
    ));
    ctx.board.set(now.clone());
    (ctx.edited)();
    now
}

/// Looks for a phone with a transfer on it, every few seconds, and takes it.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let mut looked: HashMap<String, Instant> = HashMap::new();
    let mut sent: HashMap<String, bool> = HashMap::new();
    let mut tried: HashMap<i64, u32> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        let asked = ASKED.load(Ordering::Relaxed);
        if asked > ANSWERED.load(Ordering::Relaxed) {
            looked.clear();
            tried.clear();
        }
        if let Ok(phones) = stash_core::phone::phones() {
            // A phone unplugged before anything was taken: nothing to say.
            {
                let mut now = BOARD.now.lock().unwrap();
                let gone = now.as_ref().is_some_and(|n| {
                    matches!(n.state, "connected" | "preparing")
                        && !phones.iter().any(|p| Some(&p.name) == n.phone.as_ref())
                });
                if gone {
                    *now = None;
                }
            }
            looked.retain(|id, _| phones.iter().any(|p| &p.id == id));
            sent.retain(|id, _| phones.iter().any(|p| &p.id == id));
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
                    // Songs in the folder with no list yet: the phone is still copying them out.
                    let ready = transfer.as_ref().map_or(0, |t| t.files.len());
                    if !has {
                        tell(
                            &phone.name,
                            if ready > 0 { "preparing" } else { "connected" },
                            ready,
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
/// far). What a transfer said last (taking, done, stopped) stays until the phone has something new to say: a phone
/// still plugged in after its songs were taken is not "connected" again over the result.
#[cfg(windows)]
fn tell(phone: &str, what: &'static str, ready: usize) {
    let mut now = BOARD.now.lock().unwrap();
    let same = now.as_ref().filter(|n| n.phone.as_deref() == Some(phone));
    if what == "connected" && same.is_some_and(|n| matches!(n.state, "taking" | "done" | "stopped"))
    {
        return;
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
}

#[cfg(windows)]
fn seen_inside(name: &str, has: bool) {
    for (n, h) in SEEN.lock().unwrap().iter_mut() {
        if n == name {
            *h = Some(has);
        }
    }
}

pub(crate) fn route(_state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    match (request.method(), path) {
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

    /// A phone in memory: its transfer folder's files, which of them stop sending (how many more times), whether it
    /// answers when opened again, and every read and delete asked of it.
    #[derive(Default)]
    struct Phone {
        files: Mutex<Vec<(String, Vec<u8>)>>,
        stalls: Mutex<HashMap<String, u32>>,
        answers_again: AtomicBool,
        reads: Mutex<Vec<String>>,
        opened_again: AtomicU64,
    }

    impl Phone {
        fn with(songs: &[(&str, Vec<u8>)]) -> Arc<Phone> {
            let phone = Phone::default();
            let mut files: Vec<(String, Vec<u8>)> =
                songs.iter().map(|(n, b)| (n.to_string(), b.clone())).collect();
            let list = json!({
                "stash_transfer": 1,
                "sent_at_ms": 7,
                "songs": songs.iter().map(|(n, _)| json!({ "file": n, "title": format!("Song {n}"), "artist": "Someone" })).collect::<Vec<_>>(),
            });
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
            "Test phone"
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
        fn new(name: &str) -> Here {
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

        fn take(&self, phone: &Arc<Phone>, tried: &mut HashMap<i64, u32>) -> Option<Now> {
            let (edited, note) = (|| {}, |_: &str| {});
            let ctx = Ctx {
                library: &self.library,
                board: &self.board,
                edited: &edited,
                log: &note,
            };
            take(&ctx, phone, Box::new(Open(Arc::clone(phone))), tried, &AtomicBool::new(false));
            self.board.now.lock().unwrap().clone()
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
}
