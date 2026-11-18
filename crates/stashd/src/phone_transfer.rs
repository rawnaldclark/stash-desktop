//! Songs from Stash for Android over USB. "Send to computer" on the phone leaves its downloads in
//! `Download/Stash transfer/` with `stash-transfer.json` saying which song each file is; with the phone plugged in
//! ("File transfer"), this takes them into the library as downloaded songs and deletes the folder from the phone.
//!
//! Each song is found in the library as an import finds it (its YouTube or Spotify id, its ISRC, its words), or added;
//! its file goes into the downloads folder where Stash would have saved it, and its download is recorded as saved. A
//! song the library has as a file already is left as it is. A song liked on the phone is liked here too.
//!
//! - `GET /api/phone`: what is happening: nothing, songs being taken from a phone (how many of how many), or what the
//!   last transfer did; and the phones plugged in now (with "File transfer" chosen), each with whether it has songs
//!   sent.
//! - `POST /api/phone/look`: looks at the phones now, and tries again a send that had songs not taken; answers the
//!   look's number, which `answered` in `GET /api/phone` reaches once it is done.
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use stash_core::downloads::{self, Song};
use stash_core::import;
use stash_core::lists::SavedDownload;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, now_ms, reply_json};

/// How often the phones are looked at, and how often again a phone with no transfer on it.
const LOOK_EVERY: Duration = Duration::from_secs(2);
const PHONE_AGAIN: Duration = Duration::from_secs(3);
/// The largest list read.
const LIST_CAP: u64 = 16 * 1024 * 1024;
/// A transfer taken whole, but its folder could not be deleted from the phone: it is not taken twice.
const TAKEN_SETTING: &str = "phone.taken";
/// A transfer with songs not taken is tried again (the songs taken count as here already), this many times while Stash
/// runs; plugging the phone in again after a restart tries again too.
const TRIES: u32 = 3;

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
    /// `ready` so far), "taking", "done", or "stopped" (unplugged while taking).
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

static NOW: Mutex<Option<Now>> = Mutex::new(None);
/// The phones plugged in at the last look: their names, and whether each had songs sent (None: not looked inside yet).
static SEEN: Mutex<Vec<(String, Option<bool>)>> = Mutex::new(Vec::new());
/// Looks asked for (Settings' "Look for my phone"), and the last one answered: the window waits for its number.
static ASKED: AtomicU64 = AtomicU64::new(0);
static ANSWERED: AtomicU64 = AtomicU64::new(0);

fn set(now: Now) {
    *NOW.lock().unwrap() = Some(now);
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

/// Takes one song: its row, its file put where Stash saves songs, its download recorded.
fn take_one(
    state: &State,
    transfer: &stash_core::phone::Transfer,
    file: &stash_core::phone::PhoneFile,
    sent: &Sent,
) -> Result<Took, String> {
    let song = as_song(sent);
    let id = {
        let mut library = state.library.lock().unwrap();
        let id = *library
            .adopt_songs(std::slice::from_ref(&song))
            .map_err(|e| e.to_string())?
            .first()
            .ok_or("the song was not kept")?;
        if sent.liked {
            let _ = library.set_liked(id, true, now_ms());
        }
        id
    };
    if transfer.head(file, WEBM.len())?.starts_with(&WEBM) {
        return Ok(Took::NotKept);
    }
    let (root, number) = {
        let mut library = state.library.lock().unwrap();
        if !library
            .download_arriving(id, now_ms())
            .map_err(|e| e.to_string())?
        {
            return Ok(Took::Had);
        }
        (
            crate::saving::folder(&library),
            library.track_number(id).ok().flatten(),
        )
    };
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let part = root.join(format!(".stash-phone-{id}.part"));
    let failed = |why: String| {
        let _ = std::fs::remove_file(&part);
        let _ = state.library.lock().unwrap().download_failed(id, &why);
        why
    };
    transfer.copy(file, &part).map_err(failed)?;
    let mut head = vec![0u8; 64];
    let kind = std::fs::File::open(&part)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .ok()
        .and_then(|n| downloads::kind_of(&head[..n]))
        .unwrap_or("audio");
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
    let kept = state
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

/// Takes a phone's transfer, if it has a whole one (its list is there) not taken before.
/// A failed song counts as done, for the count shown.
fn continue_failed(now: &mut Now, file: &stash_core::phone::PhoneFile) {
    now.done += 1;
    now.bytes_done += file.size;
    set(now.clone());
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

fn take(
    state: &State,
    phone: &stash_core::phone::Phone,
    transfer: &stash_core::phone::Transfer,
    tried: &mut HashMap<i64, u32>,
    stop: &AtomicBool,
) {
    let Some(list_file) = transfer
        .files
        .iter()
        .find(|f| f.name == stash_core::phone::LIST)
    else {
        return;
    };
    let bytes = match transfer.bytes(list_file, LIST_CAP) {
        Ok(bytes) => bytes,
        Err(why) => {
            // Listed but not answering (its USB connection dropped): said once, as a transfer cut short.
            if lost(&why) {
                let mut now = NOW.lock().unwrap();
                let said = now.as_ref().is_some_and(|n| {
                    n.state == "stopped" && n.phone.as_deref() == Some(phone.name.as_str())
                });
                if !said {
                    *now = Some(Now {
                        state: "stopped",
                        phone: Some(phone.name.clone()),
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
    let taken_before = state
        .library
        .lock()
        .unwrap()
        .setting(TAKEN_SETTING)
        .ok()
        .flatten();
    if taken_before.as_deref() == Some(list.sent_at_ms.to_string().as_str()) {
        return;
    }
    let tries = tried.entry(list.sent_at_ms).or_default();
    if *tries >= TRIES {
        return;
    }
    *tries += 1;
    let files: HashMap<&str, &stash_core::phone::PhoneFile> = transfer
        .files
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();
    // Songs whose file is gone were taken by an earlier try (each file is deleted from the phone once taken).
    let total = list.songs.len();
    let here: Vec<&Sent> = list
        .songs
        .iter()
        .filter(|s| files.contains_key(s.file.as_str()))
        .collect();
    let mut now = Now {
        state: "taking",
        phone: Some(phone.name.clone()),
        total,
        done: total - here.len(),
        bytes_total: here.iter().map(|s| files[s.file.as_str()].size).sum(),
        at_ms: now_ms(),
        ..Default::default()
    };
    set(now.clone());
    eprintln!(
        "phone: taking {} songs ({} taken before)",
        here.len(),
        now.done
    );
    let mut in_a_row = 0;
    for sent in here {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let file = files[sent.file.as_str()];
        now.song = Some(format!("{} · {}", sent.title.trim(), sent.artist.trim()));
        set(now.clone());
        match take_one(state, transfer, file, sent) {
            Ok(took) => {
                match took {
                    Took::Taken => now.taken += 1,
                    Took::Had => now.had += 1,
                    Took::NotKept => now.not_kept += 1,
                }
                // Off the phone: what is left there is what is still to come (the phone counts it).
                let _ = transfer.remove_file(file);
            }
            Err(why) => {
                // The phone gone (unplugged, File transfer turned off, its USB connection dropped: Windows can list
                // a phone for a while after it stops answering): stop, and say so; plugged in again, the rest is
                // taken. Songs failing one after another are taken as the phone gone too.
                in_a_row += 1;
                let gone = lost(&why)
                    || in_a_row >= LOST_AFTER
                    || !stash_core::phone::phones()
                        .map(|p| p.iter().any(|p| p.id == phone.id))
                        .unwrap_or(true);
                if gone {
                    now.state = "stopped";
                    now.song = None;
                    now.at_ms = now_ms();
                    eprintln!("phone: lost the phone after {} songs: {why}", now.done);
                    set(now);
                    state.edited();
                    return;
                }
                now.failed += 1;
                eprintln!("phone: a song was not taken: {why}");
                continue_failed(&mut now, file);
                continue;
            }
        }
        in_a_row = 0;
        now.done += 1;
        now.bytes_done += file.size;
        set(now.clone());
        // The library pages read the new files as they come.
        if now.done.is_multiple_of(10) {
            state.edited();
        }
    }
    // Everything not taken stays on the phone, to be tried again.
    if now.failed == 0 {
        match transfer.remove() {
            Ok(()) => now.cleared = true,
            Err(e) => {
                eprintln!("phone: the transfer folder stayed on the phone: {e}");
                let _ = state
                    .library
                    .lock()
                    .unwrap()
                    .set_setting(TAKEN_SETTING, &list.sent_at_ms.to_string());
            }
        }
    }
    now.state = "done";
    now.song = None;
    now.at_ms = now_ms();
    eprintln!(
        "phone: {} taken, {} here already, {} not kept (WebM), {} not taken",
        now.taken, now.had, now.not_kept, now.failed
    );
    set(now);
    state.edited();
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
                let mut now = NOW.lock().unwrap();
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
                        take(state, &phone, &transfer, &mut tried, stop);
                        // Taken and deleted from the phone: it has no songs sent now.
                        let cleared = NOW.lock().unwrap().as_ref().is_some_and(|n| {
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
    let mut now = NOW.lock().unwrap();
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
            let now = NOW.lock().unwrap().clone().unwrap_or(Now {
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
}
