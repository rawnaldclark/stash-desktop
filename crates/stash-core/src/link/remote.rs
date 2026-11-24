//! Remote control on the wire (stash-player `docs/sync-v1.md` §5.7), for a computer that keeps its own queue: what it says it
//! plays (`stash-remote-state`, with the cap `queue`) and what the phone asks of it (`stash-remote-cmd`). Both are sealed like
//! any sync-v1 document and travel over the space's live socket. Readers refuse another kind or a newer `v`, ignore unknown
//! fields and check every value.
//!
//! A computer with the cap `queue` keeps the queue: the phone is its remote. Beyond v1's play, pause, next, prev, seek and
//! volume, the phone may `start` a queue of its songs here, `add` songs (next, or at the end), `remove`, `move` and `skipTo`
//! items of this computer's queue (named by the `iid`s the state gives them), and set `shuffle` and `repeat`.

use serde_json::{Map, Value, json};

use super::crypto::CryptoError;

pub const STATE_KIND: &str = "stash-remote-state";
pub const CMD_KIND: &str = "stash-remote-cmd";
pub const VERSION: i64 = 1;
/// The cap a computer that keeps its own queue says it has.
pub const CAP_QUEUE: &str = "queue";
/// At most this many items of the queue go in a state (the one playing and those after it), cut to fit.
pub const MAX_ITEMS: usize = 50;
/// At most this many songs are taken from one command.
pub const MAX_SONGS: usize = 100;
/// A state's sealed text must stay under the server's frame cap.
pub const MAX_STATE_BYTES: usize = 12 * 1024;

type Result<T> = std::result::Result<T, CryptoError>;

fn bad(what: &'static str) -> CryptoError {
    CryptoError(what)
}

/// A socket session's id (8–32 of `A–Z a–z 0–9 _ -`).
pub fn is_sid(s: &str) -> bool {
    (8..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// An item id: 1–64 of `A–Z a–z 0–9 _ - . : #`.
pub fn is_item_id(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-.:#".contains(&b))
}

/// Text as a song carries it: control characters dropped, whitespace runs as one space, trimmed, at most `max` characters.
fn clean(v: Option<&Value>, max: usize) -> Option<String> {
    let s: String = v?.as_str()?.chars().filter(|c| !c.is_control()).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then(|| s.chars().take(max).collect())
}

/// A song the phone hands over (§2.1, the library file's Song): what this computer needs to find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneSong {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    /// Upper-cased, 12 letters and digits.
    pub isrc: Option<String>,
    /// The YouTube video id, from `refs.youtube`.
    pub youtube: Option<String>,
    /// The first https cover.
    pub art: Option<String>,
}

/// One Song read by §2.1's rules; None when it won't read or is `phoneOnly` (a song only the phone can play is never taken).
pub fn read_song(v: &Value) -> Option<PhoneSong> {
    if v.get("phoneOnly").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let title = clean(v.get("title"), 300)?;
    let artist = clean(v.get("artist"), 300)?;
    let isrc = v
        .get("isrc")
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase)
        .filter(|i| i.len() == 12 && i.bytes().all(|b| b.is_ascii_alphanumeric()));
    let youtube = v
        .get("refs")
        .and_then(|r| r.get("youtube"))
        .and_then(Value::as_str)
        .filter(|id| (6..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        .map(str::to_string);
    let art = v.get("artwork").and_then(Value::as_array).and_then(|a| {
        a.iter()
            .filter_map(|x| x.get("url").and_then(Value::as_str))
            .find(|u| u.starts_with("https://") && u.len() <= 2048 && !u.chars().any(|c| c.is_control() || c == ' '))
            .map(str::to_string)
    });
    Some(PhoneSong {
        title,
        artist,
        album: clean(v.get("album"), 300),
        duration_ms: v.get("durationMs").and_then(Value::as_u64).filter(|ms| *ms > 0 && *ms < 24 * 3_600_000),
        isrc,
        youtube,
        art,
    })
}

/// What the phone asks.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Play,
    Pause,
    Next,
    Prev,
    Stop,
    Seek { position_ms: u64 },
    /// 0 to 1.
    Volume { volume: f64 },
    /// Play these songs here, from `at`, at `position_ms`; this computer keeps them as its queue. `play`: start playing.
    Start { songs: Vec<PhoneSong>, at: usize, position_ms: u64, play: bool, from: Option<String> },
    /// Songs after the one playing (`next`, in this order), or at the end.
    Add { songs: Vec<PhoneSong>, next: bool },
    Remove { iid: String },
    /// Moves an item to place `to` of the whole queue.
    Move { iid: String, to: usize },
    SkipTo { iid: String },
    Shuffle { on: bool },
    /// `off`, `all` or `one`.
    Repeat { mode: String },
}

/// A command as read: its session, its number (each once, in order) and what it asks.
#[derive(Debug, Clone, PartialEq)]
pub struct Cmd {
    pub sid: String,
    pub n: i64,
    pub command: Command,
}

fn header<'a>(doc: &'a Value, kind: &str) -> Result<&'a Map<String, Value>> {
    let d = doc.as_object().ok_or_else(|| bad("not a document"))?;
    if d.get("kind").and_then(Value::as_str) != Some(kind) {
        return Err(bad("another kind"));
    }
    let v = d.get("v").and_then(Value::as_i64).ok_or_else(|| bad("bad version"))?;
    if v < 1 {
        return Err(bad("bad version"));
    }
    if v > VERSION {
        return Err(bad("newer version"));
    }
    Ok(d)
}

/// The songs of a `start` or `add`: each item `{ id, song }`; a song that won't read is left out.
fn songs_of(d: &Map<String, Value>) -> Vec<PhoneSong> {
    d.get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().take(MAX_SONGS).filter_map(|i| i.get("song").and_then(read_song)).collect())
        .unwrap_or_default()
}

/// `stash-remote-cmd` v1, as a computer that keeps its queue reads it. A command it doesn't know is an error the caller
/// ignores (a newer phone).
pub fn read_cmd(doc: &Value) -> Result<Cmd> {
    let d = header(doc, CMD_KIND)?;
    let sid = d.get("sid").and_then(Value::as_str).filter(|s| is_sid(s)).ok_or_else(|| bad("bad sid"))?;
    let n = d.get("n").and_then(Value::as_i64).filter(|n| *n >= 0).ok_or_else(|| bad("bad n"))?;
    let iid = || d.get("iid").and_then(Value::as_str).filter(|s| is_item_id(s)).map(str::to_string).ok_or_else(|| bad("bad iid"));
    let ms = |name: &str| d.get(name).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).map(|x| x as u64);
    let command = match d.get("cmd").and_then(Value::as_str).unwrap_or_default() {
        "play" => Command::Play,
        "pause" => Command::Pause,
        "next" => Command::Next,
        "prev" => Command::Prev,
        "stop" => Command::Stop,
        "seek" => Command::Seek { position_ms: ms("positionMs").ok_or_else(|| bad("bad positionMs"))? },
        "volume" => Command::Volume {
            volume: d.get("volume").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad("bad volume"))?.clamp(0.0, 1.0),
        },
        "start" => {
            let songs = songs_of(d);
            let at = d.get("at").and_then(Value::as_u64).unwrap_or(0) as usize;
            Command::Start {
                at: at.min(songs.len().saturating_sub(1)),
                songs,
                position_ms: ms("positionMs").unwrap_or(0),
                play: d.get("play").and_then(Value::as_bool).unwrap_or(true),
                from: clean(d.get("from"), 300),
            }
        }
        "add" => Command::Add { songs: songs_of(d), next: d.get("next").and_then(Value::as_bool).unwrap_or(false) },
        "remove" => Command::Remove { iid: iid()? },
        "move" => Command::Move { iid: iid()?, to: d.get("to").and_then(Value::as_u64).ok_or_else(|| bad("bad to"))? as usize },
        "skipTo" => Command::SkipTo { iid: iid()? },
        "shuffle" => Command::Shuffle { on: d.get("on").and_then(Value::as_bool).ok_or_else(|| bad("bad on"))? },
        "repeat" => {
            let mode = d.get("mode").and_then(Value::as_str).filter(|m| matches!(*m, "off" | "all" | "one"));
            Command::Repeat { mode: mode.ok_or_else(|| bad("bad mode"))?.to_string() }
        }
        _ => return Err(bad("unknown command")),
    };
    Ok(Cmd { sid: sid.to_string(), n, command })
}

/// A song as a state carries it: what the phone's card and Up next show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSong {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    /// An https cover; anything else is left out.
    pub art: Option<String>,
}

fn song_out(s: &StateSong) -> Value {
    let mut g = json!({ "title": s.title, "artist": s.artist });
    if let Some(a) = &s.album {
        g["album"] = json!(a);
    }
    if let Some(ms) = s.duration_ms.filter(|ms| *ms > 0) {
        g["durationMs"] = json!(ms);
    }
    if let Some(art) = s.art.as_ref().filter(|a| a.starts_with("https://")) {
        g["artwork"] = json!([{ "url": art }]);
    }
    g
}

/// What this computer plays, for the phone.
#[derive(Debug, Clone, PartialEq)]
pub struct StateIn {
    pub sid: String,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// 0 to 1.
    pub volume: f64,
    pub song: Option<StateSong>,
    /// The playing item's id.
    pub iid: Option<String>,
    /// The playing item's place in the whole queue, and the queue's length.
    pub at: usize,
    pub length: usize,
    /// The playing item and those after it, `(iid, song)`.
    pub items: Vec<(String, StateSong)>,
    pub shuffle: bool,
    pub repeat: String,
    pub from: Option<String>,
    /// Finding where the song plays from.
    pub loading: bool,
}

/// The state document: v1's fields, the cap `queue`, and the queue window (cut until the document fits [`MAX_STATE_BYTES`]).
pub fn state_doc(s: &StateIn) -> Value {
    let mut d = json!({
        "kind": STATE_KIND,
        "v": VERSION,
        "sid": s.sid,
        "needsGesture": false,
        "playing": s.playing,
        "positionMs": s.position_ms,
        "durationMs": s.duration_ms,
        "rate": 1,
        "volume": (s.volume.clamp(0.0, 1.0) * 1000.0).round() / 1000.0,
        "canSeek": s.song.is_some(),
        "caps": [CAP_QUEUE],
        "shuffle": s.shuffle,
        "repeat": s.repeat,
    });
    if let Some(song) = &s.song {
        d["song"] = song_out(song);
        d["phase"] = json!(if s.loading { "loading" } else if s.playing { "playing" } else { "paused" });
    }
    if let Some(iid) = &s.iid {
        d["iid"] = json!(iid);
    }
    if let Some(from) = &s.from {
        d["from"] = json!(from);
    }
    let mut count = s.items.len().min(MAX_ITEMS);
    loop {
        let items: Vec<Value> = s.items[..count].iter().map(|(iid, song)| json!({ "iid": iid, "song": song_out(song) })).collect();
        d["queue"] = json!({ "at": s.at, "length": s.length, "items": items });
        if count == 0 || serde_json::to_vec(&d).map(|b| b.len()).unwrap_or(0) <= MAX_STATE_BYTES {
            return d;
        }
        count /= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(title: &str) -> Value {
        json!({ "title": title, "artist": "Frank Ocean", "album": "Blonde", "durationMs": 184000, "isrc": "usum71607009",
                "refs": { "youtube": "uxpDa-c-4Mc" }, "artwork": [{ "url": "http://x/a.jpg" }, { "url": "https://x/b.jpg" }] })
    }

    #[test]
    fn songs_read_by_the_library_file_s_rules_and_phone_only_ones_are_never_taken() {
        let s = read_song(&song("Pink + White")).unwrap();
        assert_eq!(s.isrc.as_deref(), Some("USUM71607009"));
        assert_eq!(s.youtube.as_deref(), Some("uxpDa-c-4Mc"));
        assert_eq!(s.art.as_deref(), Some("https://x/b.jpg"));
        assert!(read_song(&json!({ "title": "A", "artist": "B", "phoneOnly": true })).is_none());
        assert!(read_song(&json!({ "title": "  ", "artist": "B" })).is_none());
    }

    #[test]
    fn commands_read_with_their_arguments_and_bad_ones_are_refused() {
        let base = |extra: Value| {
            let mut d = json!({ "kind": CMD_KIND, "v": 1, "sid": "abcdefgh12", "n": 3 });
            d.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            d
        };
        let c = read_cmd(&base(json!({ "cmd": "add", "next": true, "items": [{ "id": "p1", "song": song("Ivy") }, { "id": "p2", "song": { "title": "x" } }] }))).unwrap();
        assert_eq!(c.n, 3);
        assert!(matches!(c.command, Command::Add { ref songs, next: true } if songs.len() == 1 && songs[0].title == "Ivy"));
        let c = read_cmd(&base(json!({ "cmd": "start", "at": 9, "positionMs": 1500, "items": [{ "id": "p1", "song": song("Ivy") }] }))).unwrap();
        assert!(matches!(c.command, Command::Start { at: 0, position_ms: 1500, play: true, .. }));
        assert_eq!(read_cmd(&base(json!({ "cmd": "move", "iid": "q12", "to": 4 }))).unwrap().command, Command::Move { iid: "q12".into(), to: 4 });
        assert_eq!(read_cmd(&base(json!({ "cmd": "volume", "volume": 3 }))).unwrap().command, Command::Volume { volume: 1.0 });
        assert!(read_cmd(&base(json!({ "cmd": "repeat", "mode": "twice" }))).is_err());
        assert!(read_cmd(&base(json!({ "cmd": "remove", "iid": "bad id!" }))).is_err());
        assert!(read_cmd(&base(json!({ "cmd": "dance" }))).is_err());
        let mut newer = base(json!({ "cmd": "play" }));
        newer["v"] = json!(2);
        assert_eq!(read_cmd(&newer).unwrap_err(), CryptoError("newer version"));
    }

    #[test]
    fn a_state_says_the_queue_and_is_cut_to_fit() {
        let one = StateSong { title: "T".repeat(250), artist: "A".repeat(250), album: None, duration_ms: Some(1000), art: Some("https://x/a.jpg".into()) };
        let s = StateIn {
            sid: "abcdefgh12".into(),
            playing: true,
            position_ms: 5,
            duration_ms: 1000,
            volume: 0.5,
            song: Some(one.clone()),
            iid: Some("q1".into()),
            at: 3,
            length: 200,
            items: (0..50).map(|i| (format!("q{i}"), one.clone())).collect(),
            shuffle: false,
            repeat: "all".into(),
            from: Some("Liked songs".into()),
            loading: false,
        };
        let d = state_doc(&s);
        assert_eq!(d["caps"], json!(["queue"]));
        assert_eq!(d["queue"]["at"], 3);
        assert_eq!(d["queue"]["length"], 200);
        let n = d["queue"]["items"].as_array().unwrap().len();
        assert!(n > 0 && n < 50, "{n}");
        assert!(serde_json::to_vec(&d).unwrap().len() <= MAX_STATE_BYTES);
        assert_eq!(d["phase"], "playing");
    }
}
