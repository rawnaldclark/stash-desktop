//! The player, in real time but without a sound card: a queue plays through, and pause, seek, next and previous do
//! what they say. Needs libmpv (see `common::library`); without it the tests say so and pass.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{RATE, library, scratch, signal, write_wav};
use stash_core::audio::Output;
use stash_core::player::{
    AddonTrack, Failure, Located, Player, PlayerState, QueueItem, Resolver, Settings,
};

fn item(id: i64, title: &str, path: &Path) -> QueueItem {
    QueueItem {
        id: Some(id),
        addon: None,
        title: title.into(),
        artist: "Sine".into(),
        album: Some("Waves".into()),
        locator: path.to_string_lossy().into_owned(),
    }
}

/// Waits until the player's state satisfies `wanted`, and returns that state.
fn until(player: &Player, what: &str, wanted: impl Fn(&PlayerState) -> bool) -> PlayerState {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let state = player.state();
        if wanted(&state) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "waited 8 s for: {what}; the player says {state:?}"
        );
        std::thread::sleep(Duration::from_millis(15));
    }
}

#[test]
fn a_queue_plays_through_and_the_controls_do_what_they_say() {
    let Some(library) = library() else { return };
    let dir = scratch("player");
    let (a, b, c) = (dir.join("a.wav"), dir.join("b.wav"), dir.join("c.wav"));
    write_wav(&a, RATE, &signal(RATE as usize * 3 / 2)); // 1.5 s
    write_wav(&b, RATE, &signal(RATE as usize * 3 / 2));
    write_wav(&c, RATE, &signal(RATE as usize / 2)); // 0.5 s
    let player = Player::start(&library, &Output::Silent).unwrap();
    assert_eq!(
        (player.state().status, player.state().queue_len),
        ("idle", 0)
    );

    player
        .play(vec![
            item(1, "One", &a),
            item(2, "Two", &b),
            item(3, "Three", &c),
        ])
        .unwrap();
    let first = until(&player, "the first track playing", |s| {
        s.status == "playing" && s.index == Some(0) && s.duration_ms > 0
    });
    assert_eq!(
        (
            first.queue_len,
            first.track.as_ref().map(|t| (t.id, t.title.as_str()))
        ),
        (3, Some((Some(1), "One")))
    );
    assert_eq!(first.duration_ms, 1500);
    // What the sound is: the file's own rate all the way through, on the silent output.
    let path = until(&player, "the signal path", |s| {
        s.signal
            .as_ref()
            .is_some_and(|g| g.output.sample_rate.is_some())
    })
    .signal
    .unwrap();
    assert_eq!(
        (
            path.decoded.sample_rate,
            path.output.sample_rate,
            path.resampled,
            path.driver.as_deref()
        ),
        (Some(RATE), Some(RATE), false, Some("null"))
    );
    assert!(
        path.codec.as_deref().is_some_and(|c| c.starts_with("pcm")),
        "{:?}",
        path.codec
    );

    // Pause holds the position; resuming moves it again.
    player.set_paused(true).unwrap();
    let paused = until(&player, "paused", |s| s.status == "paused");
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        player.state().position_ms.abs_diff(paused.position_ms) <= 30,
        "the position moved while paused"
    );
    // A seek while paused lands where it was asked.
    player.seek_to(1.0).unwrap();
    until(&player, "the seek", |s| {
        (750..=1100).contains(&s.position_ms)
    });
    player.set_paused(false).unwrap();
    until(&player, "playing again", |s| s.status == "playing");

    // Next and previous move through the queue; previous on the first track starts it over.
    player.next().unwrap();
    until(&player, "the second track", |s| {
        s.index == Some(1) && s.track.as_ref().is_some_and(|t| t.id == Some(2))
    });
    player.previous().unwrap();
    until(&player, "back on the first", |s| s.index == Some(0));
    player.seek_to(1.0).unwrap();
    until(&player, "one second in", |s| s.position_ms >= 900);
    player.previous().unwrap();
    until(&player, "the first track from its start", |s| {
        s.index == Some(0) && s.position_ms < 500
    });

    // Left alone from the last track, the queue runs out and the player is idle.
    player.next().unwrap();
    player.next().unwrap();
    until(&player, "the last track", |s| s.index == Some(2));
    let done = until(&player, "the end of the queue", |s| s.status == "idle");
    assert_eq!((done.index, done.track, done.position_ms), (None, None, 0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_track_that_cannot_be_played_is_named_and_the_queue_goes_on() {
    let Some(library) = library() else { return };
    let dir = scratch("player-missing");
    let b = dir.join("b.wav");
    write_wav(&b, RATE, &signal(RATE as usize));
    let player = Player::start(&library, &Output::Silent).unwrap();

    player
        .play(vec![
            item(1, "Gone", &dir.join("not-there.wav")),
            item(2, "Here", &b),
        ])
        .unwrap();
    let state = until(&player, "the second track after the first failed", |s| {
        s.index == Some(1) && s.status == "playing"
    });
    assert!(
        state
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("Gone could not be played: loading failed")),
        "{:?}",
        state.error
    );
    assert_eq!(state.track.and_then(|t| t.id), Some(2));

    // A new queue while the old one is on the same place in its own (here: the second track of two, then the first
    // of one, then the first of another) is followed too.
    player.play(vec![item(3, "Again", &b)]).unwrap();
    until(&player, "the new queue's first track", |s| {
        s.status == "playing" && s.track.as_ref().is_some_and(|t| t.id == Some(3))
    });
    player.play(vec![item(4, "And again", &b)]).unwrap();
    until(&player, "another queue, first track to first track", |s| {
        s.status == "playing"
            && s.track.as_ref().is_some_and(|t| t.id == Some(4))
            && s.index == Some(0)
    });

    player.stop().unwrap();
    let stopped = until(&player, "stopped", |s| s.status == "idle");
    assert_eq!(stopped.queue_len, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An add-on for the player: it knows tracks by the files they play from, does not have "missing", cannot be reached
/// for "broken", and writes down what it was asked and when.
struct FakeAddon {
    files: HashMap<&'static str, PathBuf>,
    delay: Duration,
    asked: Mutex<Vec<(String, Instant)>>,
}

impl FakeAddon {
    fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|(track, _)| track.clone())
            .collect()
    }

    fn resolver(self: &Arc<Self>) -> Resolver {
        let addon = Arc::clone(self);
        Arc::new(move |item: &QueueItem| {
            let wanted = item.addon.as_ref().expect("an add-on's track");
            addon
                .asked
                .lock()
                .unwrap()
                .push((wanted.track.clone(), Instant::now()));
            std::thread::sleep(addon.delay);
            match addon.files.get(wanted.track.as_str()) {
                Some(path) => Ok(Located {
                    locator: path.to_string_lossy().into_owned(),
                    tier: Some("lossless".into()),
                    addon: Some(wanted.addon.clone()),
                }),
                None if wanted.track == "broken" => Err(Failure {
                    message: "the add-on could not be reached".into(),
                    miss: false,
                }),
                None => Err(Failure {
                    message: "the add-on does not have it".into(),
                    miss: true,
                }),
            }
        })
    }
}

fn from_addon(track: &str, title: &str) -> QueueItem {
    QueueItem {
        id: None,
        title: title.into(),
        artist: "Sine".into(),
        album: None,
        addon: Some(AddonTrack {
            addon: "fake".into(),
            track: track.into(),
        }),
        locator: String::new(),
    }
}

/// Three files of 1.5 s and an add-on that serves them as "a", "b" and "c".
fn fake_addon(name: &str, delay: Duration) -> (Arc<FakeAddon>, PathBuf) {
    let dir = scratch(name);
    let mut files = HashMap::new();
    for track in ["a", "b", "c"] {
        let path = dir.join(format!("{track}.wav"));
        write_wav(&path, RATE, &signal(RATE as usize * 3 / 2));
        files.insert(track, path);
    }
    (
        Arc::new(FakeAddon {
            files,
            delay,
            asked: Mutex::new(Vec::new()),
        }),
        dir,
    )
}

#[test]
fn an_add_ons_track_is_asked_for_late_and_one_it_does_not_have_is_passed_over() {
    let Some(library) = library() else { return };
    let (addon, dir) = fake_addon("player-addon-late", Duration::from_millis(150));
    let settings = Settings {
        resolver: Some(addon.resolver()),
        ask_ahead_seconds: 0.6,
        ..Settings::default()
    };
    let player = Player::start_with(&library, &Output::Silent, settings).unwrap();

    player
        .play(vec![
            from_addon("a", "A"),
            from_addon("missing", "Missing"),
            from_addon("b", "B"),
        ])
        .unwrap();
    // While the add-on is being asked, the track is named and nothing plays yet.
    let loading = player.state();
    assert_eq!(
        (
            loading.status,
            loading.index,
            loading.track.as_ref().map(|t| t.title.as_str())
        ),
        ("loading", Some(0), Some("A"))
    );
    let first = until(&player, "the add-on's first track playing", |s| {
        s.status == "playing" && s.index == Some(0)
    });
    assert_eq!(first.tier.as_deref(), Some("lossless"));
    assert_eq!(
        first
            .track
            .as_ref()
            .and_then(|t| t.addon.as_ref())
            .map(|a| (a.addon.as_str(), a.track.as_str())),
        Some(("fake", "a"))
    );
    // The track after it has not been asked for: 1.5 s are left, and it is asked for at 0.6 s.
    assert_eq!(addon.asked(), ["a"]);

    // The queue moves on by itself, past the track the add-on does not have, and says why that one was left out.
    let second = until(&player, "the third track, the second being missing", |s| {
        s.status == "playing" && s.index == Some(2)
    });
    assert_eq!(second.track.as_ref().map(|t| t.title.as_str()), Some("B"));
    assert_eq!(
        second.error.as_deref(),
        Some("Missing could not be played: the add-on does not have it")
    );
    assert_eq!(addon.asked(), ["a", "missing", "b"]);
    let asked = addon.asked.lock().unwrap().clone();
    let waited = asked[1].1.duration_since(asked[0].1);
    assert!(
        waited >= Duration::from_millis(600),
        "the next track was asked for {waited:?} after the first, long before it was needed"
    );

    until(&player, "the end of the queue", |s| s.status == "idle");
    drop(player);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn next_and_previous_do_not_wait_for_the_engine_to_have_the_track() {
    let Some(library) = library() else { return };
    let (addon, dir) = fake_addon("player-addon-skip", Duration::from_millis(20));
    let settings = Settings {
        resolver: Some(addon.resolver()),
        ask_ahead_seconds: 0.2,
        ..Settings::default()
    };
    let player = Player::start_with(&library, &Output::Silent, settings).unwrap();
    let file = dir.join("file.wav");
    write_wav(&file, RATE, &signal(RATE as usize * 3 / 2));

    // A file first, then the add-on's tracks: one queue holds both.
    player
        .play(vec![
            item(7, "File", &file),
            from_addon("a", "A"),
            from_addon("b", "B"),
            from_addon("c", "C"),
        ])
        .unwrap();
    until(&player, "the file playing", |s| {
        s.status == "playing" && s.index == Some(0)
    });
    assert_eq!(addon.asked(), Vec::<String>::new());

    player.next().unwrap();
    until(&player, "A, asked for when skipped to", |s| {
        s.status == "playing" && s.index == Some(1)
    });
    player.next().unwrap();
    let b = until(&player, "B", |s| {
        s.status == "playing" && s.index == Some(2)
    });
    assert_eq!(b.track.as_ref().map(|t| t.title.as_str()), Some("B"));
    // Back to an add-on's track asks the add-on again (the link it gave before may have run out).
    player.previous().unwrap();
    until(&player, "A again", |s| {
        s.status == "playing" && s.index == Some(1)
    });
    assert_eq!(addon.asked(), ["a", "b", "a"]);
    // Back from there is the file.
    player.previous().unwrap();
    until(&player, "the file again", |s| {
        s.status == "playing" && s.index == Some(0) && s.tier.is_none()
    });

    // Next past the last track ends playback and keeps the queue.
    for place in 1..=3 {
        player.next().unwrap();
        until(&player, "the next track", |s| {
            s.status == "playing" && s.index == Some(place)
        });
    }
    player.next().unwrap();
    let ended = until(&player, "the end", |s| s.status == "idle");
    assert_eq!(ended.queue_len, 4);
    drop(player);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trouble_with_the_add_on_is_said_and_the_queue_is_not_marched_through() {
    let Some(library) = library() else { return };
    let (addon, dir) = fake_addon("player-addon-trouble", Duration::from_millis(20));
    let settings = Settings {
        resolver: Some(addon.resolver()),
        ask_ahead_seconds: 30.0,
        ..Settings::default()
    };
    let player = Player::start_with(&library, &Output::Silent, settings).unwrap();

    player
        .play(vec![from_addon("broken", "Broken"), from_addon("a", "A")])
        .unwrap();
    let state = until(&player, "the failure", |s| {
        s.status == "idle" && s.error.is_some()
    });
    assert_eq!(
        state.error.as_deref(),
        Some("Broken could not be played: the add-on could not be reached")
    );
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(addon.asked(), ["broken"]);
    assert_eq!(player.state().status, "idle");

    // A player without add-ons says so instead of trying.
    let plain = Player::start(&library, &Output::Silent).unwrap();
    plain.play(vec![from_addon("a", "A")]).unwrap();
    let state = until(&plain, "the refusal", |s| {
        s.status == "idle" && s.error.is_some()
    });
    assert_eq!(
        state.error.as_deref(),
        Some("A could not be played: this player has no add-ons")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_play_counts_once_half_the_track_was_really_played() {
    let Some(library) = library() else { return };
    let dir = scratch("player-plays");
    let (a, b, c) = (dir.join("a.wav"), dir.join("b.wav"), dir.join("c.wav"));
    for path in [&a, &b, &c] {
        write_wav(path, RATE, &signal(RATE as usize * 3 / 2)); // 1.5 s: half is 0.75 s
    }
    let counted: Arc<Mutex<Vec<(String, i64)>>> = Arc::new(Mutex::new(Vec::new()));
    let told = Arc::clone(&counted);
    let settings = Settings {
        on_play: Some(Arc::new(move |item: &QueueItem, started_at_ms| {
            told.lock()
                .unwrap()
                .push((item.title.clone(), started_at_ms))
        })),
        // The rule's floor of 30 s would leave nothing to count in a test: here half the track decides.
        counts_between_seconds: (0.2, 240.0),
        ..Settings::default()
    };
    let player = Player::start_with(&library, &Output::Silent, settings).unwrap();
    let titles = || {
        counted
            .lock()
            .unwrap()
            .iter()
            .map(|(title, _)| title.clone())
            .collect::<Vec<_>>()
    };
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    player
        .play(vec![
            item(1, "One", &a),
            item(2, "Two", &b),
            item(3, "Three", &c),
        ])
        .unwrap();
    until(&player, "the first track playing", |s| {
        s.status == "playing" && s.index == Some(0) && s.position_ms > 100
    });
    // Paused, time passes but nothing is heard: the play does not count.
    player.set_paused(true).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(titles(), Vec::<String>::new());
    player.set_paused(false).unwrap();
    // Played past its half, it counts, once, with the time it started.
    until(&player, "the second track", |s| {
        s.index == Some(1) && s.status == "playing"
    });
    assert_eq!(titles(), ["One"]);
    let started = counted.lock().unwrap()[0].1;
    assert!(
        (before..before + 1_500).contains(&started),
        "the play is stamped with its start, not with when it counted"
    );
    // Skipped before its half, the second does not count; the third, played through, does.
    player.next().unwrap();
    until(&player, "the end of the queue", |s| s.status == "idle");
    assert_eq!(titles(), ["One", "Three"]);
    drop(player);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stream_that_does_not_start_is_asked_for_once_more() {
    let Some(library) = library() else { return };
    let dir = scratch("player-retry");
    let good = dir.join("good.wav");
    write_wav(&good, RATE, &signal(RATE as usize * 3 / 2));
    // An add-on whose first link for "flaky" leads nowhere and whose second is good; "dead" never has a good one.
    let asked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let resolver: Resolver = {
        let (asked, good, nowhere) = (Arc::clone(&asked), good.clone(), dir.join("nowhere.wav"));
        Arc::new(move |item: &QueueItem| {
            let track = item
                .addon
                .as_ref()
                .expect("an add-on's track")
                .track
                .clone();
            let earlier = asked
                .lock()
                .unwrap()
                .iter()
                .filter(|t| **t == track)
                .count();
            asked.lock().unwrap().push(track.clone());
            let path = if track == "dead" || (track == "flaky" && earlier == 0) {
                &nowhere
            } else {
                &good
            };
            Ok(Located {
                locator: path.to_string_lossy().into_owned(),
                tier: None,
                addon: Some("fake".into()),
            })
        })
    };
    let settings = Settings {
        resolver: Some(resolver),
        ask_ahead_seconds: 0.3,
        ..Settings::default()
    };
    let player = Player::start_with(&library, &Output::Silent, settings).unwrap();

    player
        .play(vec![
            from_addon("flaky", "Flaky"),
            from_addon("dead", "Dead"),
            from_addon("fine", "Fine"),
        ])
        .unwrap();
    // The first link fails in the engine; the stream is asked for again and plays, with nothing left to say.
    let flaky = until(
        &player,
        "the flaky track playing after its second link",
        |s| s.status == "playing" && s.index == Some(0) && s.position_ms > 100,
    );
    assert_eq!(flaky.error, None);
    assert_eq!(asked.lock().unwrap().clone(), ["flaky", "flaky"]);
    // A stream that fails twice is left: the queue moves on and says why, in the engine's words.
    let fine = until(&player, "the third track, the second being dead", |s| {
        s.status == "playing" && s.index == Some(2)
    });
    assert!(
        fine.error
            .as_deref()
            .is_some_and(|e| e.starts_with("Dead could not be played: loading failed")),
        "{:?}",
        fine.error
    );
    assert_eq!(
        asked
            .lock()
            .unwrap()
            .iter()
            .filter(|t| *t == "dead")
            .count(),
        2
    );
    drop(player);
    let _ = std::fs::remove_dir_all(&dir);
}
