//! The equalizer through the player and the real engine: off is nothing at all, a filter does what the math says, a
//! file reads as Equalizer APO reads it, boosts never clip, and a change while a song plays glides in and out without a
//! click, a gap or the output opened again. Most of it plays to a file and measures the samples: as fast as the decoder
//! goes, or, to change something mid-song, from a local server that hands the song out at 8 times its pace. What needs
//! the clock runs on the silent output. Needs libmpv (see `common::library`); without it the
//! tests say so and pass. The corrections here are made up in the tests: no measured data is in the repository.

mod common;

use std::f64::consts::TAU;
use std::io::{BufRead, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{RATE, Wav, library, read_wav, scratch, signal, write_wav};
use stash_core::audio::Output;
use stash_core::eq::{self, Book, Chain, Eq, Tilt};
use stash_core::player::{Player, PlayerState, QueueItem, Repeat, Settings};

fn item(id: i64, locator: &str) -> QueueItem {
    QueueItem {
        id: Some(id),
        addon: None,
        title: format!("Tone {id}"),
        artist: "Sine".into(),
        album: Some("Waves".into()),
        locator: locator.to_string(),
        art: None,
        duration_ms: None,
        qid: 0,
    }
}

fn until(player: &Player, what: &str, wanted: impl Fn(&PlayerState) -> bool) -> PlayerState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = player.state();
        if wanted(&state) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "waited 30 s for: {what}; the player says {state:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The samples of a WAV the engine wrote, as floats: 16-bit, 32-bit or float.
fn floats(wav: &Wav) -> Vec<f32> {
    match (wav.format, wav.bits) {
        (3, 32) => wav
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
        (1, 16) => wav.samples().iter().map(|s| *s as f32 / 32768.0).collect(),
        (1, 32) => wav
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| i32::from_le_bytes(*b) as f32 / 2_147_483_648.0)
            .collect(),
        other => panic!("a WAV of format {other:?}"),
    }
}

/// A tone on each channel (`hz[c]`), interleaved, 16-bit, at `rate`.
fn tone(seconds: f64, rate: u32, hz: &[f64], amp: f64) -> Vec<i16> {
    let frames = (seconds * rate as f64) as usize;
    let mut out = Vec::with_capacity(frames * hz.len());
    for n in 0..frames {
        let t = n as f64 / rate as f64;
        for f in hz {
            out.push((amp * i16::MAX as f64 * (TAU * f * t).sin()) as i16);
        }
    }
    out
}

/// A 16-bit PCM WAV of any channel count.
fn write_wav_channels(path: &Path, rate: u32, channels: u16, interleaved: &[i16]) {
    let data = (interleaved.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2 * channels as u32).to_le_bytes());
    bytes.extend_from_slice(&(2 * channels).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for s in interleaved {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn to_floats(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|s| *s as f32 / 32768.0).collect()
}

/// The level of channel `c` of `got` against `base` over frames `from..to`, in dB.
fn level_db(got: &[f32], base: &[f32], channels: usize, c: usize, from: usize, to: usize) -> f64 {
    let rms = |x: &[f32]| {
        let to = to.min(x.len() / channels);
        let n = to.saturating_sub(from).max(1);
        ((from..to)
            .map(|f| (x[f * channels + c] as f64).powi(2))
            .sum::<f64>()
            / n as f64)
            .sqrt()
    };
    20.0 * (rms(got) / rms(base)).log10()
}

/// The largest sample-to-sample step on any channel between frames `a` and `b`, and where it is.
fn max_step(x: &[f32], channels: usize, a: usize, b: usize) -> (f32, usize) {
    let mut best = (0f32, 0usize);
    for f in a.max(1)..b.min(x.len() / channels) {
        for c in 0..channels {
            let d = (x[f * channels + c] - x[(f - 1) * channels + c]).abs();
            if d > best.0 {
                best = (d, f);
            }
        }
    }
    best
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0f32, |m, s| m.max(s.abs()))
}

/// What the equalizer does to a tone of `hz` in a song at `rate`: its filters for that rate, less the room it leaves.
fn expected_db(eq: &Eq, hz: f64, rate: u32) -> f64 {
    match Chain::plan(eq, Some(rate)) {
        None => 0.0,
        Some(chain) => {
            chain
                .slots
                .iter()
                .map(|s| s.band.response_db(hz, rate as f64))
                .sum::<f64>()
                + 20.0 * chain.gain.log10()
        }
    }
}

fn correction(text: &str) -> eq::Correction {
    eq::read_apo(text.as_bytes(), "Test Phones ParametricEQ.txt").unwrap()
}

fn with(correction: Option<eq::Correction>, bass: Tilt, treble: Tilt) -> Eq {
    Eq {
        on: true,
        bass,
        treble,
        correction,
    }
}

/// A file's locator for the queue.
fn here(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// The pace the local server hands songs out at, in bytes a second: 8 times that of 16-bit stereo at 44.1 kHz.
const SERVED_PER_SECOND: usize = 8 * 4 * RATE as usize;

/// Serves `files` over HTTP at `SERVED_PER_SECOND`, as "http://127.0.0.1:{port}/{n}.wav", answering byte ranges (for
/// seeks). A play to a file then runs long enough to be changed mid-song.
fn served(files: &[&Path]) -> Vec<String> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let files: Vec<PathBuf> = files.iter().map(|f| f.to_path_buf()).collect();
    let urls = (0..files.len())
        .map(|n| format!("http://127.0.0.1:{port}/{n}.wav"))
        .collect();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let files = files.clone();
            std::thread::spawn(move || {
                let _ = answer(stream, &files);
            });
        }
    });
    urls
}

fn answer(mut stream: TcpStream, files: &[PathBuf]) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let mut from = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some(range) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
            from = range
                .trim()
                .split('-')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
        }
    }
    let wanted = first
        .split_whitespace()
        .nth(1)
        .and_then(|path| path.trim_start_matches('/').strip_suffix(".wav"))
        .and_then(|n| n.parse::<usize>().ok());
    let Some(file) = wanted.and_then(|n| files.get(n)) else {
        return stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    };
    let bytes = std::fs::read(file)?;
    let from = from.min(bytes.len());
    let head = if from == 0 {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
    } else {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/wav\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {from}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len() - 1,
            bytes.len(),
            bytes.len() - from
        )
    };
    stream.write_all(head.as_bytes())?;
    for chunk in bytes[from..].chunks(SERVED_PER_SECOND / 50) {
        stream.write_all(chunk)?;
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

const CUT: &str = "Filter 1: ON PK Fc 441 Hz Gain -6.0 dB Q 1.41\n";
const BOOST6: &str = "Filter 1: ON PK Fc 441 Hz Gain 6.0 dB Q 1.41\n";

/// What a play to a file gave: its samples, its format (format tag, bits), channels and rate, and the player's last
/// state.
struct Played {
    out: Vec<f32>,
    format: (u16, u16),
    channels: usize,
    rate: u32,
    last: PlayerState,
}

/// A change at a point in the song: when the player's place reaches `at` seconds, it pauses, `act` runs, a moment
/// passes (the first commands go while paused), and it plays on.
type Act<'a> = (f64, &'a dyn Fn(&Player));

/// Plays `songs` in a row to the file `out`: `setup` before the first starts, then each of `acts` in turn.
fn play(
    library: &Path,
    out: &Path,
    songs: &[&str],
    setup: &dyn Fn(&Player),
    acts: &[Act],
) -> Played {
    play_with(library, out, songs, Settings::default(), setup, acts)
}

fn play_with(
    library: &Path,
    out: &Path,
    songs: &[&str],
    settings: Settings,
    setup: &dyn Fn(&Player),
    acts: &[Act],
) -> Played {
    let player =
        Player::start_with(library, &Output::PcmFile(out.to_path_buf()), settings).unwrap();
    setup(&player);
    let items = songs
        .iter()
        .enumerate()
        .map(|(i, p)| item(i as i64 + 1, p))
        .collect();
    player.play(items).unwrap();
    for (at, act) in acts {
        let wanted = (*at * 1000.0) as u64;
        until(&player, "the place to change at", |s| {
            s.status == "playing" && s.index == Some(0) && s.position_ms >= wanted
        });
        player.set_paused(true).unwrap();
        until(&player, "the pause", |s| s.status == "paused");
        act(&player);
        std::thread::sleep(Duration::from_millis(150));
        player.set_paused(false).unwrap();
    }
    let last = until(&player, "the end of the queue", |s| s.resting);
    drop(player);
    let wav = read_wav(out);
    Played {
        out: floats(&wav),
        format: (wav.format, wav.bits),
        channels: wav.channels as usize,
        rate: wav.rate,
        last,
    }
}

fn frames(p: &Played) -> usize {
    p.out.len() / p.channels
}

#[test]
fn off_or_flat_the_samples_go_through_untouched() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-off");
    let input = signal(RATE as usize * 4);
    let join = 2 * RATE as usize + 123;
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write_wav(&a, RATE, &input[..join * 2]);
    write_wav(&b, RATE, &input[join * 2..]);
    let kept_off = Eq {
        on: false,
        ..with(Some(correction(CUT)), Tilt::More, Tilt::Less)
    };
    let on_nothing = with(None, Tilt::AsIs, Tilt::AsIs);
    for (name, eq) in [
        ("off over a kept correction", kept_off),
        ("on with nothing set", on_nothing),
    ] {
        let out = dir.join(format!("{}.wav", name.replace(' ', "-")));
        let filters = Mutex::new(Vec::new());
        let played = play(
            &library,
            &out,
            &[here(&a), here(&b)],
            &|player: &Player| {
                player.set_eq(eq.clone()).unwrap();
                filters.lock().unwrap().push(player.filters());
            },
            &[],
        );
        assert_eq!(
            filters.lock().unwrap()[0],
            "",
            "{name}: the engine's filter list"
        );
        assert_eq!(
            (played.format, played.channels, played.rate),
            ((1, 16), 2, RATE),
            "{name}: the output is the songs' own format"
        );
        assert!(!played.last.eq.on || name.starts_with("on"), "{name}");
        let got: Vec<i16> = played.out.iter().map(|s| (s * 32768.0) as i16).collect();
        common::assert_same_samples(&got, &input, join);
    }
}

#[test]
fn a_cut_does_what_the_math_says() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-cut");
    let input = signal(RATE as usize * 3);
    let song = dir.join("song.wav");
    write_wav(&song, RATE, &input);
    let eq = with(Some(correction(CUT)), Tilt::AsIs, Tilt::AsIs);
    let played = play(
        &library,
        &dir.join("out.wav"),
        &[here(&song)],
        &|player: &Player| player.set_eq(eq.clone()).unwrap(),
        &[],
    );
    assert_eq!(
        played.format,
        (3, 32),
        "the equalizer hands the output floats"
    );
    assert_eq!(
        frames(&played),
        input.len() / 2,
        "no sample added or dropped"
    );
    let base = to_floats(&input);
    let (from, to) = (RATE as usize / 2, frames(&played));
    let left = level_db(&played.out, &base, 2, 0, from, to);
    let right = level_db(&played.out, &base, 2, 1, from, to);
    let band = correction(CUT).bands[0];
    assert!(
        eq.room_db().abs() < 1e-9,
        "a cut leaves no room: {}",
        eq.room_db()
    );
    assert!(
        (left + 6.0).abs() < 0.02,
        "441 Hz is cut by 6 dB: {left:.3}"
    );
    let want = band.response_db(663.0, RATE as f64);
    assert!(
        (right - want).abs() < 0.02,
        "663 Hz: {right:.3}, the math says {want:.3}"
    );
    assert!(played.last.signal.is_none(), "nothing plays at the end");
}

#[test]
fn a_file_plays_as_equalizer_apo_reads_it() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-import");
    let text = "Preamp: -4.5 dB\r\n\
                Filter 1: ON PK Fc 250 Hz Gain -4.0 dB Q 1.2\r\n\
                Filter 2: ON PK Fc 1000 Hz Gain 3.0 dB BW Oct 1.5\r\n\
                Filter 3: ON LSC Fc 90 Hz Gain 3.5 dB Q 0.71\r\n\
                Filter 4: ON HSC 6 dB Fc 7000 Hz Gain -2.5 dB\r\n\
                Filter 5: ON LS Fc 150 Hz Gain -1.5 dB Q 0.8\r\n";
    let corr = correction(text);
    assert_eq!(corr.bands.len(), 5);
    let eq = with(Some(corr), Tilt::AsIs, Tilt::AsIs);
    let fcs = [250.0, 1000.0, 90.0, 7000.0, 150.0];
    let songs: Vec<_> = fcs
        .iter()
        .map(|hz| {
            let path = dir.join(format!("tone-{hz}.wav"));
            write_wav(&path, RATE, &tone(2.0, RATE, &[*hz, *hz], 0.3));
            path
        })
        .collect();
    let paths: Vec<&str> = songs.iter().map(|p| here(p)).collect();
    let played = play(
        &library,
        &dir.join("out.wav"),
        &paths,
        &|player: &Player| player.set_eq(eq.clone()).unwrap(),
        &[],
    );
    let each = 2 * RATE as usize;
    assert_eq!(
        frames(&played),
        each * fcs.len(),
        "no sample added or dropped"
    );
    for (i, hz) in fcs.iter().enumerate() {
        let base = to_floats(&tone(2.0, RATE, &[*hz, *hz], 0.3));
        let got = &played.out[i * each * 2..(i + 1) * each * 2];
        let level = level_db(got, &base, 2, 0, RATE as usize / 2, each);
        let want = expected_db(&eq, *hz, RATE);
        assert!(
            (level - want).abs() < 0.05,
            "{hz} Hz: {level:.3} dB, the filters say {want:.3}"
        );
    }
}

#[test]
fn boosts_leave_room_and_nothing_clips() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-room");
    let input = signal(RATE as usize * 3);
    let song = dir.join("song.wav");
    write_wav(&song, RATE, &input);
    let eq = with(
        Some(correction(
            "Filter 1: ON PK Fc 441 Hz Gain 12.0 dB Q 1.41\n",
        )),
        Tilt::AsIs,
        Tilt::AsIs,
    );
    assert!((eq.room_db() - 13.0).abs() < 0.02, "room {}", eq.room_db());
    let played = play(
        &library,
        &dir.join("out.wav"),
        &[here(&song)],
        &|player: &Player| player.set_eq(eq.clone()).unwrap(),
        &[],
    );
    let left: Vec<f32> = played
        .out
        .iter()
        .step_by(2)
        .skip(RATE as usize / 2)
        .copied()
        .collect();
    let want = 0.6 * 10f64.powf(-1.0 / 20.0);
    assert!(
        (peak(&left) as f64 - want).abs() < 0.001,
        "the 441 Hz peak is {} (+12 dB less 13 dB of room: {want:.4})",
        peak(&left)
    );
    assert!(peak(&played.out) < 0.9999, "no sample at full scale");
    assert_eq!(played.last.eq.softer_db, 13.0);

    // ReplayGain comes after the equalizer: the same, softer.
    let tagged = dir.join("tagged.wav");
    std::fs::copy(&song, &tagged).unwrap();
    {
        use lofty::config::WriteOptions;
        use lofty::file::{AudioFile, TaggedFileExt};
        use lofty::tag::{ItemKey, Tag, TagType};
        let mut file = lofty::read_from_path(&tagged).unwrap();
        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_text(ItemKey::ReplayGainTrackGain, "-6.02 dB".into());
        file.insert_tag(tag);
        file.save_to_path(&tagged, WriteOptions::default()).unwrap();
    }
    let evened = play(
        &library,
        &dir.join("evened.wav"),
        &[here(&tagged)],
        &|player: &Player| {
            player.set_replaygain("track").unwrap();
            player.set_eq(eq.clone()).unwrap();
        },
        &[],
    );
    let left: Vec<f32> = evened
        .out
        .iter()
        .step_by(2)
        .skip(RATE as usize / 2)
        .copied()
        .collect();
    assert!(
        (peak(&left) as f64 - want / 2.0).abs() < 0.002,
        "evened out: {}",
        peak(&left)
    );
}

/// The song the changes mid-song are made in, served at 8 times its pace (its address, and its samples): long enough
/// for every glide and wait to land well before its end.
fn long_song(dir: &Path, seconds: usize) -> (String, Vec<i16>) {
    let input = signal(RATE as usize * seconds);
    let song = dir.join("long.wav");
    write_wav(&song, RATE, &input);
    (served(&[&song]).remove(0), input)
}

/// The steps of the input itself: the bound a glide must stay near.
fn own_step(input: &[i16]) -> f32 {
    max_step(&to_floats(input), 2, 1, input.len() / 2).0
}

#[test]
fn turned_on_mid_song_it_glides_in_without_a_click_or_a_reopen() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-on-mid");
    let (song, input) = long_song(&dir, 30);
    let eq = with(None, Tilt::More, Tilt::AsIs);
    let played = play(
        &library,
        &dir.join("out.wav"),
        &[&song],
        &|_: &Player| {},
        &[(1.0, &|player: &Player| player.set_eq(eq.clone()).unwrap())],
    );
    assert_eq!(
        frames(&played),
        input.len() / 2,
        "no sample added or dropped"
    );
    assert_eq!(played.format, (1, 16), "the output stays as it opened");
    let base = to_floats(&input);
    let first = played
        .out
        .iter()
        .zip(&base)
        .position(|(g, b)| (g - b).abs() > 1.5 / 32768.0)
        .expect("the equalizer changed the samples")
        / 2;
    assert!(
        first > RATE as usize / 2,
        "nothing changed before the change: frame {first}"
    );
    let (step, at) = max_step(&played.out, 2, 1, frames(&played));
    assert!(
        step <= 1.2 * own_step(&input),
        "the largest step is {step} at frame {at} (the song's own: {})",
        own_step(&input)
    );
    assert!(peak(&played.out) < 0.9999, "no sample at full scale");
    let end = frames(&played);
    for (c, hz) in [(0, 441.0), (1, 663.0)] {
        let level = level_db(&played.out, &base, 2, c, end - RATE as usize, end);
        let want = expected_db(&eq, hz, RATE);
        assert!(
            (level - want).abs() < 0.05,
            "{hz} Hz at the end: {level:.3}, want {want:.3}"
        );
    }
}

#[test]
fn turned_off_mid_song_it_glides_out_and_the_samples_come_back_exact() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-off-mid");
    let (song, input) = long_song(&dir, 30);
    let on = with(None, Tilt::More, Tilt::AsIs);
    let off = Eq {
        on: false,
        ..on.clone()
    };
    let after = Mutex::new(String::new());
    let out = dir.join("out.wav");
    let player = Player::start(&library, &Output::PcmFile(out.clone())).unwrap();
    player.set_eq(on.clone()).unwrap();
    player.play(vec![item(1, &song)]).unwrap();
    until(&player, "the song playing", |s| {
        s.status == "playing" && s.position_ms > 1000
    });
    player.set_paused(true).unwrap();
    until(&player, "the pause", |s| s.status == "paused");
    player.set_eq(off).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    player.set_paused(false).unwrap();
    until(&player, "the end", |s| s.resting);
    *after.lock().unwrap() = player.filters();
    // The next song clears what is left.
    player.play(vec![item(2, &song)]).unwrap();
    until(&player, "the engine on the second song", |s| {
        s.signal.as_ref().is_some_and(|g| g.codec.is_some()) || s.resting
    });
    let cleared = player.filters();
    player.stop().unwrap();
    drop(player);
    let after = after.into_inner().unwrap();
    assert!(
        after.contains("@frames") && !after.contains("@eq:"),
        "after the glide out: {after}"
    );
    assert_eq!(cleared, "", "the next song plays with no filter at all");

    let got = floats(&read_wav(&out));
    let base = to_floats(&input);
    // The first song: from where the last step landed on, the samples are the song's own, exactly.
    let first = &got[..input.len()];
    let last_changed = first
        .iter()
        .zip(&base)
        .rposition(|(g, b)| g != b)
        .expect("the equalizer was heard")
        / 2;
    assert!(
        last_changed < input.len() / 2 - 10 * RATE as usize,
        "the samples are exact again well before the end (last changed at frame {last_changed})"
    );
    let (step, at) = max_step(first, 2, 1, input.len() / 2);
    assert!(
        step <= 1.2 * own_step(&input),
        "the largest step is {step} at frame {at}"
    );
}

#[test]
fn tone_changes_up_and_down_mid_song_never_click_or_clip() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-tones-mid");
    let (song, input) = long_song(&dir, 40);
    let more = with(None, Tilt::More, Tilt::AsIs);
    let less = with(None, Tilt::Less, Tilt::AsIs);
    let less_bright = with(None, Tilt::Less, Tilt::More);
    let played = play(
        &library,
        &dir.join("out.wav"),
        &[&song],
        &|_: &Player| {},
        &[
            (1.0, &|p: &Player| p.set_eq(more.clone()).unwrap()),
            (12.0, &|p: &Player| p.set_eq(less.clone()).unwrap()),
            (24.0, &|p: &Player| p.set_eq(less_bright.clone()).unwrap()),
        ],
    );
    assert_eq!(
        frames(&played),
        input.len() / 2,
        "no sample added or dropped"
    );
    let (step, at) = max_step(&played.out, 2, 1, frames(&played));
    assert!(
        step <= 1.2 * own_step(&input),
        "the largest step is {step} at frame {at}"
    );
    assert!(peak(&played.out) < 0.9999, "no sample at full scale");
    let base = to_floats(&input);
    let end = frames(&played);
    for (c, hz) in [(0, 441.0), (1, 663.0)] {
        let level = level_db(&played.out, &base, 2, c, end - RATE as usize, end);
        let want = expected_db(&less_bright, hz, RATE);
        assert!(
            (level - want).abs() < 0.05,
            "{hz} Hz at the end: {level:.3}, want {want:.3}"
        );
    }
}

#[test]
fn a_new_correction_mid_song_goes_through_flat_without_a_click() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-shape-mid");
    let (song, input) = long_song(&dir, 30);
    let first = with(Some(correction(CUT)), Tilt::AsIs, Tilt::AsIs);
    let second = with(
        Some(correction(
            "Filter 1: ON PK Fc 1000 Hz Gain 3.0 dB Q 1.0\nFilter 2: ON PK Fc 663 Hz Gain -4.0 dB Q 2.0\n",
        )),
        Tilt::AsIs,
        Tilt::AsIs,
    );
    let played = play(
        &library,
        &dir.join("out.wav"),
        &[&song],
        &|p: &Player| p.set_eq(first.clone()).unwrap(),
        &[(2.0, &|p: &Player| p.set_eq(second.clone()).unwrap())],
    );
    assert_eq!(
        frames(&played),
        input.len() / 2,
        "no sample added or dropped"
    );
    assert_eq!(played.format, (3, 32), "the output stays as it opened");
    let (step, at) = max_step(&played.out, 2, 1, frames(&played));
    assert!(
        step <= 1.2 * own_step(&input),
        "the largest step is {step} at frame {at}"
    );
    assert!(peak(&played.out) < 0.9999, "no sample at full scale");
    let base = to_floats(&input);
    let end = frames(&played);
    for (c, hz) in [(0, 441.0), (1, 663.0)] {
        let level = level_db(&played.out, &base, 2, c, end - RATE as usize, end);
        let want = expected_db(&second, hz, RATE);
        assert!(
            (level - want).abs() < 0.05,
            "{hz} Hz at the end: {level:.3}, want {want:.3}"
        );
    }
}

#[test]
fn the_next_song_and_a_seek_start_with_the_whole_curve() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-seams");
    let a_in = signal(RATE as usize * 10);
    let b_in = signal(RATE as usize * 4);
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write_wav(&a, RATE, &a_in);
    write_wav(&b, RATE, &b_in);
    let bright = with(None, Tilt::AsIs, Tilt::More);
    let deep = with(None, Tilt::More, Tilt::AsIs);
    // A change while A plays, then B: B is exactly B played with the new curve from its start.
    let urls = served(&[&a, &b]);
    let changed = play(
        &library,
        &dir.join("changed.wav"),
        &[&urls[0], &urls[1]],
        &|p: &Player| p.set_eq(bright.clone()).unwrap(),
        &[(1.0, &|p: &Player| p.set_eq(deep.clone()).unwrap())],
    );
    let from_start = play(
        &library,
        &dir.join("from-start.wav"),
        &[here(&a), here(&b)],
        &|p: &Player| p.set_eq(deep.clone()).unwrap(),
        &[],
    );
    let at = a_in.len();
    assert_eq!(changed.out.len(), from_start.out.len());
    assert!(
        changed.out[at..] == from_start.out[at..],
        "B with the curve changed during A is B with that curve from its start"
    );

    // A seek while the change still waits for its glide: from the seek on, the whole curve.
    let boost = with(Some(correction(BOOST6)), Tilt::AsIs, Tilt::AsIs);
    let song = dir.join("song.wav");
    let input = signal(RATE as usize * 20);
    write_wav(&song, RATE, &input);
    let url = served(&[&song]).remove(0);
    let sought = play(
        &library,
        &dir.join("sought.wav"),
        &[&url],
        &|_: &Player| {},
        &[(1.0, &|p: &Player| {
            p.set_eq(boost.clone()).unwrap();
            std::thread::sleep(Duration::from_millis(150));
            p.seek_to(10.0).unwrap();
        })],
    );
    let after = input.len() / 2 - 10 * RATE as usize;
    let seek = frames(&sought) - after;
    let got = &sought.out[seek * 2..];
    let base = to_floats(&input[10 * RATE as usize * 2..]);
    let level = level_db(got, &base, 2, 0, RATE as usize / 10, RATE as usize);
    let want = expected_db(&boost, 441.0, RATE);
    assert!(
        (level - want).abs() < 0.02,
        "just after the seek, 441 Hz is at {level:.3} dB: the whole curve is {want:.3}"
    );
}

#[test]
fn when_repeat_one_starts_the_song_over_the_whole_curve_is_written() {
    // The engine builds the graph again from the list as written when it starts the song over, and a change made while
    // it played lives only in commands: so the list is written whole then (a seam), not glided to again from flat.
    let Some(library) = library() else { return };
    let dir = scratch("eq-repeat");
    let song = dir.join("song.wav");
    write_wav(&song, RATE, &signal(RATE as usize * 2));
    let boost = with(Some(correction(BOOST6)), Tilt::AsIs, Tilt::AsIs);
    let player = Player::start(&library, &Output::Silent).unwrap();
    player.set_repeat(Repeat::One);
    player.play(vec![item(1, here(&song))]).unwrap();
    until(&player, "the song playing", |s| {
        s.signal.as_ref().is_some_and(|g| g.codec.is_some()) && s.position_ms > 300
    });
    player.set_eq(boost).unwrap();
    let at_flat = "equalizer@c0=f=441.000:t=q:w=1.4100:g=0.000";
    let whole = "equalizer@c0=f=441.000:t=q:w=1.4100:g=6.000";
    assert!(
        player.filters().contains(at_flat),
        "written at flat, then glided: {}",
        player.filters()
    );
    until(&player, "near the song's end", |s| s.position_ms > 1500);
    until(&player, "the song started over", |s| s.position_ms < 1000);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !player.filters().contains(whole) {
        assert!(
            Instant::now() < deadline,
            "the whole curve was not written: {}",
            player.filters()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    player.set_repeat(Repeat::Off);
    player.stop().unwrap();
}

#[test]
fn a_gapless_join_with_the_equalizer_on() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-join");
    let input = signal(RATE as usize * 6);
    let join = 3 * RATE as usize + 123;
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    write_wav(&a, RATE, &input[..join * 2]);
    write_wav(&b, RATE, &input[join * 2..]);
    let eq = with(Some(correction(CUT)), Tilt::AsIs, Tilt::AsIs);
    let played = play(
        &library,
        &dir.join("out.wav"),
        &[here(&a), here(&b)],
        &|p: &Player| p.set_eq(eq.clone()).unwrap(),
        &[],
    );
    assert_eq!(
        frames(&played),
        input.len() / 2,
        "no sample added or dropped"
    );
    let steady = max_step(&played.out, 2, RATE as usize, 2 * RATE as usize).0;
    let (step, at) = max_step(&played.out, 2, join - 64, join + 64);
    println!(
        "EQ JOIN: the largest step at the join is {step:.4} at frame {at} (steady {steady:.4}; join at {join})"
    );
    assert!(step <= 0.35, "the join's step grew: {step}");
}

#[test]
fn a_filter_above_what_a_song_holds_is_left_out_for_it() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-nyquist");
    let rate = 32_000;
    let input = tone(3.0, rate, &[441.0, 441.0], 0.5);
    let song = dir.join("song.wav");
    write_wav_channels(&song, rate, 2, &input);
    let eq = with(
        Some(correction(
            "Filter 1: ON PK Fc 16000 Hz Gain 6.0 dB Q 1.41\n",
        )),
        Tilt::AsIs,
        Tilt::AsIs,
    );
    let chain = Chain::plan(&eq, Some(rate)).unwrap();
    assert!(
        chain.slots.iter().all(|s| s.id != "c0"),
        "16 kHz is left out at 32 kHz"
    );
    let played = play(
        &library,
        &dir.join("out.wav"),
        &[here(&song)],
        &|p: &Player| p.set_eq(eq.clone()).unwrap(),
        &[],
    );
    assert!(played.out.iter().all(|s| s.is_finite()), "no NaN");
    assert!(peak(&played.out) > 0.1, "not silent");
    let level = level_db(
        &played.out,
        &to_floats(&input),
        2,
        0,
        rate as usize / 2,
        frames(&played),
    );
    let want = 20.0 * chain.gain.log10();
    assert!(
        (level - want).abs() < 0.1,
        "441 Hz: {level:.3}, the room alone is {want:.3}"
    );
}

#[test]
fn every_rate_and_every_channel_is_filtered() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-rates");
    let eq = with(
        Some(correction(
            "Filter 1: ON PK Fc 441 Hz Gain -6.0 dB Q 1.41\nFilter 2: ON PK Fc 3000 Hz Gain 3.0 dB Q 1.0\n",
        )),
        Tilt::More,
        Tilt::AsIs,
    );
    let check = |rate: u32, hz: &[f64]| {
        let channels = hz.len();
        let input = tone(2.0, rate, hz, 0.3);
        let song = dir.join(format!("song-{rate}-{channels}.wav"));
        write_wav_channels(&song, rate, channels as u16, &input);
        let played = play(
            &library,
            &dir.join(format!("out-{rate}-{channels}.wav")),
            &[here(&song)],
            &|p: &Player| p.set_eq(eq.clone()).unwrap(),
            &[],
        );
        assert_eq!(
            (played.channels, played.rate),
            (channels, rate),
            "the song's channels and rate"
        );
        let base = to_floats(&input);
        for (c, f) in hz.iter().enumerate() {
            let level = level_db(
                &played.out,
                &base,
                channels,
                c,
                rate as usize / 2,
                frames(&played),
            );
            let want = expected_db(&eq, *f, rate);
            assert!(
                (level - want).abs() < 0.05,
                "{rate} Hz, channel {c} ({f} Hz): {level:.3}, want {want:.3}"
            );
        }
    };
    for rate in [8_000, 22_050, 44_100, 48_000, 96_000, 192_000] {
        check(rate, &[441.0, 663.0]);
    }
    check(44_100, &[441.0]);
    check(48_000, &[441.0, 663.0, 882.0, 1323.0, 220.0, 330.0]);
}

#[test]
fn a_list_the_engine_cannot_build_turns_the_equalizer_off_and_the_queue_plays_on() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-fail");
    let songs: Vec<_> = (0..3)
        .map(|i| {
            let path = dir.join(format!("song-{i}.wav"));
            write_wav(&path, RATE, &signal(RATE as usize));
            path
        })
        .collect();
    let paths: Vec<&str> = songs.iter().map(|p| here(p)).collect();
    let heard: Arc<Mutex<Vec<Book>>> = Arc::default();
    let settings = Settings {
        on_equalizers: Some({
            let heard = Arc::clone(&heard);
            Arc::new(move |book: &Book| heard.lock().unwrap().push(book.clone()))
        }),
        ..Settings::default()
    };
    let deep = with(None, Tilt::More, Tilt::AsIs);
    let played = play_with(
        &library,
        &dir.join("out.wav"),
        &paths,
        settings,
        &|p: &Player| {
            p.set_eq(deep.clone()).unwrap();
            p.set_filters_unchecked("@eq:lavfi=[nosuchfilter]");
        },
        &[],
    );
    assert_eq!(
        frames(&played),
        3 * RATE as usize,
        "all three songs played, whole"
    );
    assert!(!played.last.eq.on, "the equalizer was turned off");
    assert!(
        played
            .last
            .eq
            .trouble
            .as_deref()
            .is_some_and(|t| t.contains("could not be used")),
        "and says why: {:?}",
        played.last.eq.trouble
    );
    assert!(
        played.last.errors <= 1,
        "the songs were not at fault: {} errors",
        played.last.errors
    );
    let heard = heard.lock().unwrap();
    assert!(
        heard.last().is_some_and(|book| !book.get("auto").on),
        "the book kept says it is off"
    );
}

#[test]
fn in_real_time_the_signal_says_when_the_equalizer_changes_the_samples() {
    let Some(library) = library() else { return };
    let dir = scratch("eq-silent");
    let song = dir.join("song.wav");
    write_wav(&song, RATE, &signal(RATE as usize * 20));
    let deep = with(None, Tilt::More, Tilt::AsIs);
    let off = Eq {
        on: false,
        ..deep.clone()
    };
    let player = Player::start(&library, &Output::Silent).unwrap();
    player.play(vec![item(1, here(&song))]).unwrap();
    until(&player, "the song playing", |s| {
        s.signal.as_ref().is_some_and(|g| g.codec.is_some())
    });
    let within = |what: &str, seconds: f64, wanted: &dyn Fn(&PlayerState) -> bool| {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        loop {
            let state = player.state();
            assert!(
                state.signal.as_ref().is_none_or(|g| !g.scaled),
                "scaled stays off"
            );
            if wanted(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "not within {seconds} s: {what}; {state:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    player.set_eq(deep.clone()).unwrap();
    let on = within("equalized", 1.5, &|s| {
        s.signal.as_ref().is_some_and(|g| g.equalized)
    });
    assert_eq!(on.eq.softer_db, 5.0);
    assert_eq!(on.eq.describe.as_deref(), Some("more bass"));
    player.set_eq(off.clone()).unwrap();
    within("not equalized", 1.5, &|s| {
        s.signal.as_ref().is_some_and(|g| !g.equalized)
    });
    // Off while paused: nothing plays, so nothing glides; it goes once the song plays on.
    player.set_eq(deep.clone()).unwrap();
    within("equalized again", 1.5, &|s| {
        s.signal.as_ref().is_some_and(|g| g.equalized)
    });
    player.set_paused(true).unwrap();
    within("the pause", 1.5, &|s| s.status == "paused");
    player.set_eq(off).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        player.filters().contains("@eq:"),
        "paused, the equalizer waits: {}",
        player.filters()
    );
    player.set_paused(false).unwrap();
    within("gone after the pause", 2.0, &|_| {
        !player.filters().contains("@eq:")
    });
}

#[test]
fn each_output_keeps_its_own() {
    let Some(library) = library() else { return };
    let player = Player::start(&library, &Output::Silent).unwrap();
    let mut book = Book::default();
    book.put(
        "wasapi/{a}",
        "Headphones",
        with(None, Tilt::More, Tilt::AsIs),
    );
    player.set_equalizers(book);
    assert_eq!(player.filters(), "", "the system's choice has none");
    player.set_device("wasapi/{a}").unwrap();
    assert!(
        player.filters().contains("@eq:"),
        "the headphones have theirs: {}",
        player.filters()
    );
    let state = player.state();
    assert_eq!(
        (state.eq.output.name.as_str(), state.eq.on),
        ("wasapi/{a}", true)
    );
    player.set_device("auto").unwrap();
    assert_eq!(player.filters(), "", "and the system's choice none again");
    let state = player.state();
    assert!(state.eq.system_choice && !state.eq.on, "{:?}", state.eq);
}
