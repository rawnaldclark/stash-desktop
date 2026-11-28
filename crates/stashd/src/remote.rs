//! The phone as the remote for what plays on this computer (`stash_core::link::remote`; sync-v1 §5.7): while this computer is
//! linked ([`crate::link`]) and online, it holds one live socket on its space. When a phone of the space has its socket open,
//! this computer says what it plays (its song, place, volume and the queue from the playing song on), at once on a change and
//! as a heartbeat; and it does what the phone asks: play, pause, skip, seek, volume, play the phone's songs here, add to the
//! queue, take songs out of it, move them, jump to one, shuffle and repeat. This computer keeps the queue: the phone is a remote.
//!
//! Commands are taken only from a phone whose key this computer verified, only for this socket's session (`sid`), and each
//! number once, in order: the server can't replay one later. Nothing is sent while no phone listens.

use std::collections::{HashMap, HashSet};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use stash_core::Library;
use stash_core::link::crypto::{Envelope, b64u, open, place, random_bytes, seal};
use stash_core::link::remote::{Cmd, Command, PhoneSong, StateFormat, StateIn, StateSong, read_cmd, state_doc};
use stash_core::player::{AddonTrack, Player, QueueItem, Repeat};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

use crate::away::Conn;
use crate::link::{Device, Space};
use crate::serve::State;

/// Heartbeats: often enough for the phone's progress bar while playing, rarely while paused.
const BEAT_PLAYING: Duration = Duration::from_secs(5);
const BEAT_PAUSED: Duration = Duration::from_secs(20);
/// Changes close together (a new song and its start) go as one state.
const SETTLE: Duration = Duration::from_millis(250);
/// The server closes a socket that says nothing for 90 s.
const PING: Duration = Duration::from_secs(30);
/// A position this far from where the last state put it is a seek: said at once.
const JUMP_MS: i64 = 2_000;
/// The queue around the playing song that goes in a state.
const WINDOW: usize = 50;

/// Close codes of the sync service that mean "don't come back": revoked, forbidden, replaced by another socket of this device.
const FINAL: [u16; 3] = [4401, 4403, 4409];

/// Runs until `stop`: connects while linked and online, and connects again after a drop (waiting longer each time).
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let mut wait = Duration::from_secs(2);
    while !stop.load(Ordering::Relaxed) {
        let linked = state.link.linked();
        let player = state.player.as_ref().ok();
        let (Some((device, space)), Some(player)) = (linked, player) else {
            nap(stop, Duration::from_secs(2));
            continue;
        };
        if state.offline.load(Ordering::Relaxed) {
            nap(stop, Duration::from_secs(5));
            continue;
        }
        match Session::open(state, player, device, space) {
            Ok(mut session) => {
                wait = Duration::from_secs(2);
                match session.serve(stop) {
                    Ended::Again => {}
                    Ended::Final(code) => {
                        eprintln!("remote: the sync service closed the socket ({code})");
                        if code == 4401 {
                            // This device was removed from the space, or the space is gone.
                            let _ = state.link.check();
                        }
                        // Not again for a while, unless the link changes (linked again, say).
                        nap_or_change(state, stop, Duration::from_secs(60));
                    }
                }
            }
            Err(why) => {
                eprintln!("remote: not connected: {why}");
                nap_or_change(state, stop, wait);
                wait = (wait * 2).min(Duration::from_secs(60));
            }
        }
    }
}

/// Waits `d`, or until the link changes.
fn nap_or_change(state: &State, stop: &AtomicBool, d: Duration) {
    let generation = state.link.changed.load(Ordering::SeqCst);
    let until = Instant::now() + d;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) && state.link.changed.load(Ordering::SeqCst) == generation {
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn nap(stop: &AtomicBool, d: Duration) {
    let until = Instant::now() + d;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(200));
    }
}

enum Ended {
    /// Dropped, or the link or the setting changed: connect again.
    Again,
    /// Closed for good with this code.
    Final(u16),
}

/// What the last state said, to tell a change from a heartbeat.
#[derive(PartialEq, Clone)]
struct Said {
    status: &'static str,
    /// The engine's codec and sample rate: they arrive once the song starts, and the phone shows them.
    format: Option<(String, Option<u32>)>,
    qid: Option<u64>,
    queue_version: u64,
    shuffle: bool,
    repeat: &'static str,
    volume: u32,
    from: Option<String>,
}

struct Session<'s> {
    state: &'s State,
    player: &'s Player,
    device: Device,
    space: Space,
    socket: WebSocket<Conn>,
    sid: String,
    /// The link's change counter when this socket opened: another value means connect again.
    generation: u64,
    phones: HashSet<String>,
    /// The last command number taken from each phone, this session.
    last_n: HashMap<String, i64>,
    said: Option<(Said, i64, Instant)>,
    /// A state is due at this time (a change, or a command just done).
    due: Option<Instant>,
    pinged: Instant,
    /// The phone's place in a song it handed over, to seek to once the song has started here (until the deadline).
    seek_after_start: Option<(u64, Instant)>,
}

fn sync_ws(space_id: &str) -> String {
    let base = std::env::var("STASH_SYNC_URL").unwrap_or_else(|_| "https://sync.stashfm.app".into());
    let ws = base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    format!("{ws}/v1/spaces/{space_id}/ws")
}

/// Opens a WebSocket to `address` (ws or wss), reads timing out every 250 ms.
fn connect(address: &str) -> Result<WebSocket<Conn>, String> {
    let url = url::Url::parse(address).map_err(|_| "a wrong address".to_string())?;
    let tls = url.scheme() == "wss";
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or("the link service could not be found: is this computer online?")?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| format!("can't reach the link service: {e}"))?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = tcp.set_nodelay(true);
    let conn = if tls { Conn::tls(&host, tcp).map_err(|e| e.to_string())? } else { Conn::Plain(tcp) };
    let request = address.into_client_request().map_err(|e| e.to_string())?;
    match tungstenite::client::client(request, conn) {
        Ok((socket, _)) => {
            let _ = socket.get_ref().tcp().set_read_timeout(Some(Duration::from_millis(250)));
            Ok(socket)
        }
        Err(e) => Err(format!("the link service refused the socket: {e}")),
    }
}

fn sid() -> String {
    b64u(&random_bytes(12))
}

impl<'s> Session<'s> {
    fn open(state: &'s State, player: &'s Player, device: Device, space: Space) -> Result<Session<'s>, String> {
        let generation = state.link.changed.load(Ordering::SeqCst);
        let mut socket = connect(&sync_ws(&space.id))?;
        socket
            .send(Message::text(json!({ "t": "hello", "device": device.id, "token": device.token }).to_string()))
            .map_err(|e| e.to_string())?;
        Ok(Session {
            state,
            player,
            device,
            space,
            socket,
            sid: sid(),
            generation,
            phones: HashSet::new(),
            last_n: HashMap::new(),
            said: None,
            due: None,
            pinged: Instant::now(),
            seek_after_start: None,
        })
    }

    fn serve(&mut self, stop: &AtomicBool) -> Ended {
        loop {
            if stop.load(Ordering::Relaxed)
                || self.state.offline.load(Ordering::Relaxed)
                || self.state.link.changed.load(Ordering::SeqCst) != self.generation
            {
                let _ = self.socket.close(None);
                return Ended::Again;
            }
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    if let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) {
                        self.frame(&frame);
                    }
                }
                Ok(Message::Close(close)) => {
                    let code = close.map(|c| u16::from(c.code)).unwrap_or(1000);
                    return if FINAL.contains(&code) { Ended::Final(code) } else { Ended::Again };
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Ended::Again,
            }
            if self.pinged.elapsed() >= PING {
                self.pinged = Instant::now();
                if self.socket.send(Message::text("ping")).is_err() {
                    return Ended::Again;
                }
            }
            if let Some((ms, until)) = self.seek_after_start {
                let s = self.player.state();
                if s.status == "playing" || (s.status == "paused" && s.duration_ms > 0) {
                    let _ = self.player.seek_to(ms as f64 / 1000.0);
                    self.seek_after_start = None;
                } else if Instant::now() > until {
                    self.seek_after_start = None;
                }
            }
            if !self.phones.is_empty() && self.state_due() && self.send_state().is_err() {
                return Ended::Again;
            }
        }
    }

    // ---------------------------------------------------------------- frames

    fn frame(&mut self, f: &Value) {
        match f["t"].as_str().unwrap_or_default() {
            "ready" => {
                self.phones = f["peers"]
                    .as_array()
                    .map(|peers| {
                        peers
                            .iter()
                            .filter(|p| p["type"] == "phone")
                            .filter_map(|p| p["device"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(epoch) = f["epoch"].as_i64() {
                    self.catch_up(epoch);
                }
                self.due = Some(Instant::now());
            }
            "peer" => {
                let device = f["device"].as_str().unwrap_or_default().to_string();
                if f["type"] != "phone" {
                    return;
                }
                if f["on"].as_bool() == Some(true) {
                    // A phone came (back): it starts a new session with this computer, so it hears the state at once.
                    self.phones.insert(device.clone());
                    self.last_n.remove(&device);
                    self.due = Some(Instant::now());
                } else {
                    self.phones.remove(&device);
                }
            }
            "cmd" => {
                let from = f["from"].as_str().unwrap_or_default().to_string();
                if let Ok(env) = serde_json::from_value::<Envelope>(f["env"].clone())
                    && let Some(cmd) = self.open_cmd(&from, &env)
                {
                    self.perform(&from, cmd);
                }
            }
            "epoch" => {
                if let Some(epoch) = f["epoch"].as_i64() {
                    self.catch_up(epoch);
                }
            }
            // A rotation the server says happened: walk the keys and say the state again. (A rotation that is due is the
            // phone's to make; a command to a phone that went needs nothing.)
            "error" if f["code"] == "epoch" => {
                if let Some(epoch) = f["epoch"].as_i64() {
                    self.catch_up(epoch);
                    self.due = Some(Instant::now());
                }
            }
            _ => {}
        }
    }

    /// Walks this computer's keys up to `epoch` (a rotation happened).
    fn catch_up(&mut self, epoch: i64) {
        if epoch <= self.space.epoch {
            return;
        }
        let mut space = self.space.clone();
        if let Ok(true) = self.state.link.catch_up(&self.device, &mut space, epoch) {
            self.state.link.put_space(space.clone());
            self.space = space;
            // Keeping the new key changes nothing else: this socket carries on.
            self.generation = self.state.link.changed.load(Ordering::SeqCst);
        }
    }

    /// A command, opened and checked: from a phone this computer verified, for this session, a number not seen yet.
    fn open_cmd(&mut self, from: &str, env: &Envelope) -> Option<Cmd> {
        let verified = |space: &Space| space.roster.get(from).is_some_and(|r| r.kind == "phone");
        if !verified(&self.space) {
            // A phone linked since this computer last read the space: read it again, once.
            let _ = self.state.link.check();
            if let Some((_, space)) = self.state.link.linked() {
                self.space = space;
            }
            if !verified(&self.space) {
                return None;
            }
        }
        let key = self.space.data_key_at(env.e)?;
        let doc = open(&key, &self.space.id, &place::remote_cmd(from, &self.device.id), env).ok()?;
        let cmd = read_cmd(&doc).ok()?;
        if cmd.sid != self.sid || self.last_n.get(from).is_some_and(|n| cmd.n <= *n) {
            return None;
        }
        self.last_n.insert(from.to_string(), cmd.n);
        Some(cmd)
    }

    // ---------------------------------------------------------------- the state

    fn said_now(&self) -> (Said, i64) {
        let s = self.player.state();
        let said = Said {
            status: s.status,
            format: s.signal.as_ref().and_then(|g| g.codec.clone().map(|c| (c, g.decoded.sample_rate))),
            qid: s.track.as_ref().map(|t| t.qid),
            queue_version: s.queue_version,
            shuffle: s.shuffle,
            repeat: s.repeat,
            volume: s.volume,
            from: s.from.clone(),
        };
        (said, s.position_ms as i64)
    }

    /// A state is due: a change the phone shows, a jump in the position, a command just done, or the heartbeat.
    fn state_due(&mut self) -> bool {
        let now = Instant::now();
        let (said, position) = self.said_now();
        let Some((last, last_position, at)) = &self.said else { return true };
        if &said != last {
            // A change: said once things settle.
            let due = *self.due.get_or_insert(now + SETTLE);
            return now >= due;
        }
        if let Some(due) = self.due {
            return now >= due;
        }
        let playing = said.status == "playing";
        let expected = last_position + if playing { at.elapsed().as_millis() as i64 } else { 0 };
        if (position - expected).abs() > JUMP_MS {
            return true;
        }
        at.elapsed() >= if playing { BEAT_PLAYING } else { BEAT_PAUSED }
    }

    /// What the playing song is as it plays: the engine's codec and rate, and a file's bit depth from the library (a stream's
    /// only when the engine decodes 16-bit samples).
    fn format_of(&self, s: &stash_core::player::PlayerState) -> Option<StateFormat> {
        let track = s.track.as_ref()?;
        let signal = s.signal.as_ref()?;
        let file_bits = match (track.addon.is_none(), track.id) {
            (true, Some(id)) => self.state.library.lock().unwrap().bit_depth_of(id).ok().flatten(),
            _ => None,
        };
        StateFormat::of(signal.codec.as_deref()?, signal.decoded.sample_rate, signal.decoded.format.as_deref(), file_bits)
    }

    fn send_state(&mut self) -> Result<(), Box<tungstenite::Error>> {
        let s = self.player.state();
        let index = s.index.unwrap_or(0);
        let window = if s.track.is_some() { self.player.queue_at(index, WINDOW) } else { Vec::new() };
        let song = |item: &QueueItem| StateSong {
            title: item.title.clone(),
            artist: item.artist.clone(),
            album: item.album.clone(),
            duration_ms: item.duration_ms,
            art: item.art.clone().filter(|a| a.starts_with("https://")),
        };
        let input = StateIn {
            sid: self.sid.clone(),
            playing: s.status == "playing" || s.status == "loading",
            position_ms: s.position_ms,
            duration_ms: s.duration_ms,
            volume: s.volume as f64 / 100.0,
            song: s.track.as_ref().map(song),
            iid: s.track.as_ref().map(|t| iid(t.qid)),
            at: index,
            length: s.queue_len,
            items: window.iter().map(|i| (iid(i.qid), song(i))).collect(),
            shuffle: s.shuffle,
            repeat: s.repeat.to_string(),
            from: s.from.clone(),
            loading: s.status == "loading",
            format: self.format_of(&s),
        };
        let (said, position) = self.said_now();
        self.said = Some((said, position, Instant::now()));
        self.due = None;
        let Some(key) = self.space.data_key() else { return Ok(()) };
        let Ok(env) = seal(&key, &self.space.id, self.space.epoch, &place::remote(&self.device.id), &state_doc(&input)) else {
            return Ok(());
        };
        self.socket.send(Message::text(json!({ "t": "state", "env": env }).to_string())).map_err(Box::new)
    }

    // ---------------------------------------------------------------- commands

    fn perform(&mut self, from: &str, cmd: Cmd) {
        let p = self.player;
        // Tuned in to a session, the DJ chooses what plays: refused as a command that did not work is (the phone is
        // sent the state as it is).
        let refused = self.state.sessions.listening() && crate::sessions::the_djs_command(&cmd.command);
        let result: anyhow::Result<()> = match cmd.command {
            _ if refused => Err(anyhow::anyhow!(crate::sessions::TUNED_IN)),
            Command::Play => p.set_paused(false).map_err(Into::into),
            Command::Pause => p.set_paused(true).map_err(Into::into),
            Command::Next => p.next().map_err(Into::into),
            Command::Prev => p.previous().map_err(Into::into),
            Command::Stop => p.stop().map_err(Into::into),
            Command::Seek { position_ms } => p.seek_to(position_ms as f64 / 1000.0).map_err(Into::into),
            Command::Volume { volume } => p.set_volume((volume * 100.0).round()).map_err(Into::into),
            Command::Start { songs, at, position_ms, play, from: list } => self.start(&songs, at, position_ms, play, list),
            Command::Add { songs, next } => {
                let (items, _) = items_for(&self.state.library.lock().unwrap(), &songs, 0);
                if !items.is_empty() {
                    p.enqueue(items, next);
                }
                Ok(())
            }
            Command::Remove { iid } => {
                if let Some(place) = self.place_of(&iid) {
                    p.remove_at(place);
                }
                Ok(())
            }
            Command::Move { iid, to } => {
                if let Some(place) = self.place_of(&iid) {
                    p.move_item(place, to);
                }
                Ok(())
            }
            Command::SkipTo { iid } => {
                if let Some(place) = self.place_of(&iid) {
                    p.jump(place);
                }
                Ok(())
            }
            Command::Shuffle { on } => {
                p.set_shuffle(on);
                Ok(())
            }
            Command::Repeat { mode } => {
                p.set_repeat(Repeat::from_word(&mode));
                Ok(())
            }
        };
        if let Err(e) = result {
            eprintln!("remote: a command from {from} did not work: {e}");
        }
        // The phone hears what came of it at once.
        self.due = Some(Instant::now() + Duration::from_millis(100));
    }

    fn start(&mut self, songs: &[PhoneSong], at: usize, position_ms: u64, play: bool, from: Option<String>) -> anyhow::Result<()> {
        let (items, start) = items_for(&self.state.library.lock().unwrap(), songs, at);
        if items.is_empty() {
            return Ok(());
        }
        self.player.play_from(items, start, from)?;
        if !play {
            self.player.set_paused(true)?;
        }
        // The phone's place in the song: once the song has started here.
        self.seek_after_start = (position_ms > 2_000).then(|| (position_ms, Instant::now() + Duration::from_secs(15)));
        Ok(())
    }

    /// The place in the queue of the item `iid` names: looked for near the playing song first.
    fn place_of(&self, iid_text: &str) -> Option<usize> {
        let qid: u64 = iid_text.strip_prefix('q')?.parse().ok()?;
        let s = self.player.state();
        let near = s.index.unwrap_or(0).saturating_sub(5);
        if let Some(i) = self.player.queue_at(near, WINDOW + 10).iter().position(|i| i.qid == qid) {
            return Some(near + i);
        }
        self.player.queue_at(0, s.queue_len).iter().position(|i| i.qid == qid)
    }
}

/// An item's id on the wire: its place-in-queue number.
fn iid(qid: u64) -> String {
    format!("q{qid}")
}

/// Songs another device describes (the phone's, a session's) as this computer's queue items, and the place `at` lands
/// on among those kept: each song is the library's when the library has it (by ISRC, else by its words), else YouTube
/// Music's by its id (asked of the listener's add-ons first), else found by its description. Songs the listener chose
/// not to play are left out (one song asked for by itself still plays). Shared by the phone's remote and
/// `POST /api/player/songs` ([`crate::sessions`]).
pub(crate) fn items_for(library: &Library, songs: &[PhoneSong], at: usize) -> (Vec<QueueItem>, usize) {
    let mut items = Vec::with_capacity(songs.len());
    let mut start = 0;
    for (place, song) in songs.iter().enumerate() {
        if place == at {
            start = items.len();
        }
        let row = library.same_song(&song.title, &song.artist, song.isrc.as_deref()).ok().flatten();
        let item = match row {
            Some(id) => library.queue_items(&[id]).ok().and_then(|mut v| v.pop()),
            None => Some(QueueItem {
                id: None,
                title: song.title.clone(),
                artist: song.artist.clone(),
                album: song.album.clone(),
                addon: song
                    .youtube
                    .as_ref()
                    .map(|yt| AddonTrack { addon: stash_core::youtube::ID.to_string(), track: yt.clone() }),
                locator: String::new(),
                art: song.art.clone(),
                duration_ms: song.duration_ms,
                qid: 0,
            }),
        };
        items.extend(item);
    }
    match crate::blocking::leave_out(library, items, start) {
        Ok(Some((items, start))) => (items, start),
        _ => (Vec::new(), 0),
    }
}
