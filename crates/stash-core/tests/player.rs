//! The player, in real time but without a sound card: a queue plays through, and pause, seek, next and previous do
//! what they say. Needs libmpv (see `common::library`); without it the tests say so and pass.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{RATE, library, read_wav, scratch, signal, write_wav};
use stash_core::audio::Output;
use stash_core::player::{
    AddonTrack, Failure, Kept, Located, Player, PlayerState, QueueItem, Repeat, Resolver, Settings,
};

fn item(id: i64, title: &str, path: &Path) -> QueueItem {
    QueueItem {
        id: Some(id),
        addon: None,
        title: title.into(),
        artist: "Sine".into(),
        album: Some("Waves".into()),
        locator: path.to_string_lossy().into_owned(),
        art: None,
        duration_ms: None,
        qid: 0,
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

    // Left alone from the last track, the queue runs out and the player rests on its last track: the queue
    // stays, nothing plays, and the window still has a song to show.
    player.next().unwrap();
    player.next().unwrap();
    until(&player, "the last track", |s| s.index == Some(2));
    let done = until(&player, "the end of the queue", |s| s.resting);
    assert_eq!(
        (done.status, done.index, done.position_ms, done.queue_len),
        ("paused", Some(2), 0, 3)
    );
    assert_eq!(done.track.as_ref().and_then(|t| t.id), Some(3));
    assert!(done.signal.is_none());
    // Next has nowhere to go and changes nothing; a seek moves the place play will go on from.
    player.next().unwrap();
    player.seek_to(0.2).unwrap();
    let moved = player.state();
    assert_eq!(
        (moved.resting, moved.index, moved.position_ms),
        (true, Some(2), 200)
    );
    // Previous goes to the track before the one it rests on, and plays.
    player.previous().unwrap();
    until(&player, "the track before the last", |s| {
        s.status == "playing" && !s.resting && s.index == Some(1)
    });
    // Songs added to a resting queue are only added: nothing starts by itself.
    until(&player, "the end of the queue again", |s| s.resting);
    player.enqueue(vec![item(1, "One", &a)], false);
    std::thread::sleep(Duration::from_millis(300));
    let grown = player.state();
    assert_eq!(
        (grown.resting, grown.index, grown.queue_len),
        (true, Some(2), 4)
    );
    // Now there is a next one.
    player.next().unwrap();
    until(&player, "the added track", |s| {
        s.status == "playing" && s.index == Some(3)
    });

    // A queue kept from last time comes back as it was left: nothing plays, and play goes on from the second in.
    player.stop().unwrap();
    player.restore(Kept {
        items: vec![item(1, "One", &a), item(2, "Two", &b), item(3, "Three", &c)],
        place: 1,
        seconds: 1.0,
        from: Some("Waves".into()),
        shuffle: false,
        original: None,
        repeat: Repeat::All,
    });
    let back = player.state();
    assert_eq!(
        (
            back.status,
            back.resting,
            back.index,
            back.position_ms,
            back.queue_len
        ),
        ("paused", true, Some(1), 1000, 3)
    );
    assert_eq!((back.from.as_deref(), back.repeat), (Some("Waves"), "all"));
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        player.state().resting,
        "a kept queue does not start by itself"
    );
    player.set_paused(false).unwrap();
    let on = until(&player, "the kept track from where it was left", |s| {
        s.status == "playing" && !s.resting && s.index == Some(1) && s.position_ms >= 900
    });
    assert!(on.position_ms < 1500, "{}", on.position_ms);
    let (_, kept, original) = player.keep();
    assert_eq!((kept.len(), original), (3, None));
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
                    ..Default::default()
                }),
                None => Err(Failure {
                    message: "the add-on does not have it".into(),
                    miss: true,
                    ..Default::default()
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
        art: None,
        duration_ms: None,
        qid: 0,
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

    until(&player, "the end of the queue", |s| s.resting);
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
    let ended = until(&player, "the end", |s| s.resting);
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
    let state = until(&player, "the failure", |s| s.resting && s.error.is_some());
    // It rests on the track that failed, so play tries that one again.
    assert_eq!(
        state.track.as_ref().map(|t| t.title.as_str()),
        Some("Broken")
    );
    assert_eq!(
        state.error.as_deref(),
        Some("Broken could not be played: the add-on could not be reached")
    );
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(addon.asked(), ["broken"]);
    assert!(player.state().resting);

    // A player without add-ons says so instead of trying.
    let plain = Player::start(&library, &Output::Silent).unwrap();
    plain.play(vec![from_addon("a", "A")]).unwrap();
    let state = until(&plain, "the refusal", |s| s.resting && s.error.is_some());
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
    until(&player, "the end of the queue", |s| s.resting);
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

#[test]
fn the_volume_the_queue_and_a_jump_into_it() {
    let Some(library) = library() else { return };
    let dir = scratch("player-basics");
    let paths: Vec<_> = (0..4).map(|n| dir.join(format!("{n}.wav"))).collect();
    for path in &paths {
        write_wav(path, RATE, &signal(RATE as usize * 3 / 2));
    }
    let player = Player::start(&library, &Output::Silent).unwrap();
    assert_eq!(
        (player.state().volume, player.state().exclusive),
        (100, false)
    );
    player.set_volume(35.4).unwrap();
    assert_eq!(player.state().volume, 35);
    player.set_volume(250.0).unwrap();
    assert_eq!(player.state().volume, 100);
    // Exclusive mode can be asked for with nothing playing; it holds for what plays next.
    player.set_exclusive(true).unwrap();
    assert!(player.state().exclusive);
    player.set_exclusive(false).unwrap();

    player
        .play(
            paths
                .iter()
                .enumerate()
                .map(|(n, path)| item(n as i64 + 1, &format!("Track {}", n + 1), path))
                .collect(),
        )
        .unwrap();
    until(&player, "the first track", |s| {
        s.status == "playing" && s.index == Some(0)
    });
    assert_eq!(
        player
            .queue_at(0, 10)
            .iter()
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>(),
        ["Track 1", "Track 2", "Track 3", "Track 4"]
    );
    assert_eq!(
        player
            .queue_at(2, 1)
            .iter()
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>(),
        ["Track 3"]
    );
    // A jump plays from that place on; the queue stays whole.
    assert!(player.jump(2));
    let third = until(&player, "the third track after the jump", |s| {
        s.status == "playing" && s.index == Some(2)
    });
    assert_eq!(
        (third.queue_len, third.track.map(|t| t.title)),
        (4, Some("Track 3".to_string()))
    );
    until(&player, "the fourth by itself", |s| {
        s.status == "playing" && s.index == Some(3)
    });
    assert!(!player.jump(9));
    // And back: a jump to an earlier place.
    assert!(player.jump(0));
    until(&player, "the first again", |s| {
        s.status == "playing" && s.index == Some(0)
    });
    drop(player);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_queue_is_edited_while_it_plays() {
    let Some(library) = library() else { return };
    let dir = scratch("player-edit");
    let paths: Vec<_> = (0..6).map(|n| dir.join(format!("{n}.wav"))).collect();
    for path in &paths {
        write_wav(path, RATE, &signal(RATE as usize * 3 / 2));
    }
    let track = |n: usize| item(n as i64, &format!("T{n}"), &paths[n]);
    let player = Player::start(&library, &Output::Silent).unwrap();
    let titles = || {
        player
            .queue_at(0, 50)
            .iter()
            .map(|t| t.title.clone())
            .collect::<Vec<_>>()
    };
    let now = |s: &PlayerState| s.track.as_ref().map(|t| t.title.clone());

    player
        .play_from(
            vec![track(0), track(1), track(2)],
            1,
            Some("A playlist".into()),
        )
        .unwrap();
    let playing = until(&player, "the second track, where it was started", |s| {
        s.status == "playing" && s.index == Some(1)
    });
    assert_eq!(
        (
            now(&playing),
            playing.from.as_deref(),
            playing.shuffle,
            playing.repeat
        ),
        (Some("T1".into()), Some("A playlist"), false, "off")
    );
    let version = playing.queue_version;
    // Every place in the queue has a number of its own.
    let qids: Vec<u64> = player.queue_at(0, 50).iter().map(|t| t.qid).collect();
    assert!(qids.iter().all(|q| *q > 0) && qids[0] != qids[1] && qids[1] != qids[2]);

    // Play next goes right after the playing track; add to the end goes last.
    player.enqueue(vec![track(3)], true);
    player.enqueue(vec![track(4)], false);
    assert_eq!(titles(), ["T0", "T1", "T3", "T2", "T4"]);
    let edited = player.state();
    assert!(edited.queue_version > version);
    assert_eq!((edited.index, now(&edited)), (Some(1), Some("T1".into())));
    // A track before the playing one is taken out: the playing one is one place up, and still playing.
    assert!(player.remove_at(0));
    assert_eq!(
        (player.state().index, now(&player.state())),
        (Some(0), Some("T1".into()))
    );
    // The last is moved to right after it.
    assert!(player.move_item(3, 1));
    assert_eq!(titles(), ["T1", "T4", "T3", "T2"]);
    assert!(!player.move_item(1, 1) && !player.move_item(9, 0) && !player.remove_at(9));
    // What plays next is what the queue now says, not what the engine was handed before the edits.
    let next = until(&player, "the moved track, by itself", |s| {
        s.status == "playing" && s.index == Some(1)
    });
    assert_eq!(now(&next), Some("T4".into()));
    // The playing track is taken out: the one after it plays.
    assert!(player.remove_at(1));
    let after = until(&player, "the track after the removed one", |s| {
        s.status == "playing" && now(s) == Some("T3".into())
    });
    assert_eq!(
        (after.index, titles()),
        (Some(1), vec!["T1".to_string(), "T3".into(), "T2".into()])
    );

    // The queue set to start over: after the last track comes the first.
    player.set_repeat(Repeat::All);
    assert_eq!(player.state().repeat, "all");
    assert!(player.jump(2));
    until(&player, "the last track", |s| {
        s.status == "playing" && s.index == Some(2)
    });
    until(&player, "the first track again, by itself", |s| {
        s.status == "playing" && s.index == Some(0)
    });
    player.set_repeat(Repeat::Off);

    // Shuffle keeps the playing track and its place, reorders what follows, and gives the order back.
    player
        .play_from((0..6).map(track).collect(), 0, None)
        .unwrap();
    until(&player, "the first of six", |s| {
        s.status == "playing" && s.index == Some(0)
    });
    let ordered = titles();
    player.set_shuffle(true);
    let shuffled = titles();
    assert_eq!(
        (
            shuffled[0].as_str(),
            player.state().shuffle,
            player.state().index
        ),
        ("T0", true, Some(0))
    );
    let mut sorted = shuffled.clone();
    sorted.sort();
    assert_eq!(sorted, ordered);
    player.set_shuffle(false);
    assert_eq!(titles(), ordered);
    // With shuffle on, a new queue starts with the chosen track and holds every track once.
    player.set_shuffle(true);
    player
        .play_from((0..6).map(track).collect(), 4, None)
        .unwrap();
    let chosen = until(&player, "the chosen track first", |s| {
        s.status == "playing" && s.index == Some(0)
    });
    assert_eq!(now(&chosen), Some("T4".into()));
    let mut all = titles();
    all.sort();
    assert_eq!(all, ordered);
    player.set_shuffle(false);
    assert_eq!(titles(), ordered);
    assert_eq!(now(&player.state()), Some("T4".into()));

    // Added to a player with nothing playing, a track plays.
    player.stop().unwrap();
    until(&player, "stopped", |s| {
        s.status == "idle" && s.queue_len == 0
    });
    player.enqueue(vec![track(5)], false);
    until(&player, "the added track playing", |s| {
        s.status == "playing" && now(s) == Some("T5".into())
    });
    drop(player);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tracks_are_taken_out_wherever_they_are_in_one_go() {
    let Some(library) = library() else { return };
    let dir = scratch("player-retain");
    let paths: Vec<_> = (0..6).map(|n| dir.join(format!("{n}.wav"))).collect();
    for path in &paths {
        write_wav(path, RATE, &signal(RATE as usize * 3 / 2));
    }
    let track = |n: usize| item(n as i64, &format!("T{n}"), &paths[n]);
    let player = Player::start(&library, &Output::Silent).unwrap();
    let titles = || {
        player
            .queue_at(0, 50)
            .iter()
            .map(|t| t.title.clone())
            .collect::<Vec<_>>()
    };
    let now = |s: &PlayerState| s.track.as_ref().map(|t| t.title.clone());
    let without =
        |out: &'static [&'static str]| move |t: &QueueItem| !out.contains(&t.title.as_str());

    player
        .play_from((0..6).map(track).collect(), 2, None)
        .unwrap();
    until(&player, "the third track", |s| {
        s.status == "playing" && s.index == Some(2)
    });
    // Tracks before and after the playing one go; it plays on, a place up.
    assert_eq!(player.retain(without(&["T0", "T3"])), 2);
    assert_eq!(titles(), ["T1", "T2", "T4", "T5"]);
    assert_eq!(
        (player.state().index, now(&player.state())),
        (Some(1), Some("T2".into()))
    );
    assert_eq!(player.retain(|_| true), 0);
    // The playing track goes: the next one kept plays.
    assert_eq!(player.retain(without(&["T2", "T4"])), 2);
    let next = until(&player, "the next track kept", |s| {
        s.status == "playing" && now(s) == Some("T5".into())
    });
    assert_eq!(
        (next.index, titles()),
        (Some(1), vec!["T1".to_string(), "T5".into()])
    );
    // The playing track goes and none is kept after it: playback ends, the rest of the queue stays.
    assert_eq!(player.retain(without(&["T5"])), 1);
    let ended = until(&player, "playback ended", |s| s.status == "idle");
    assert_eq!((ended.queue_len, titles()), (1, vec!["T1".to_string()]));

    // The queue played out, the player still showing its last track: it goes, and nothing is left to show.
    player.play_from(vec![track(0), track(1)], 1, None).unwrap();
    until(&player, "the queue played out", |s| {
        s.resting && now(s) == Some("T1".into())
    });
    assert_eq!(player.retain(without(&["T1"])), 1);
    let gone = until(&player, "nothing to show", |s| s.track.is_none());
    assert_eq!((gone.status, titles()), ("idle", vec!["T0".to_string()]));

    // A queue kept from last time rests on a track (nothing plays yet): it goes, and the player rests on the next track
    // kept, from its start. Play then starts that one.
    player.stop().unwrap();
    player.restore(Kept {
        items: (0..4).map(track).collect(),
        place: 1,
        seconds: 1.0,
        from: None,
        shuffle: false,
        original: None,
        repeat: Repeat::Off,
    });
    assert_eq!(player.retain(without(&["T0", "T1"])), 2);
    let rests = player.state();
    assert_eq!(
        (rests.resting, now(&rests), rests.position_ms, titles()),
        (
            true,
            Some("T2".into()),
            0,
            vec!["T2".to_string(), "T3".into()]
        )
    );
    player.set_paused(false).unwrap();
    until(&player, "the track it rested on, playing", |s| {
        s.status == "playing" && now(s) == Some("T2".into())
    });
    drop(player);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_sound_outputs_are_listed_and_one_is_chosen() {
    let Some(library) = library() else { return };
    let player = Player::start(&library, &Output::Silent).unwrap();
    let devices = player.devices();
    // The system's own choice is always there, first.
    assert_eq!(devices[0].name, "auto", "{devices:?}");
    assert!(devices.iter().all(|d| !d.description.contains("  ")));
    assert_eq!(player.device(), "auto");
    let last = devices.last().unwrap().name.clone();
    player.set_device(&last).unwrap();
    assert_eq!(player.device(), last);
    player.set_device("auto").unwrap();
    assert_eq!(player.device(), "auto");
}

#[test]
fn replaygain_evens_a_song_out_and_the_signal_says_the_samples_are_scaled() {
    let Some(library) = library() else { return };
    let dir = scratch("replaygain");
    let song = dir.join("tagged.wav");
    write_wav(&song, RATE, &signal(RATE as usize * 3));
    // A track gain of -6.02 dB in the file's tags: half as loud, sample for sample.
    {
        use lofty::config::WriteOptions;
        use lofty::file::{AudioFile, TaggedFileExt};
        use lofty::tag::{ItemKey, Tag, TagType};
        let mut file = lofty::read_from_path(&song).unwrap();
        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_text(ItemKey::ReplayGainTrackGain, "-6.02 dB".into());
        file.insert_tag(tag);
        file.save_to_path(&song, WriteOptions::default()).unwrap();
    }
    let peak = |path: &Path| {
        read_wav(path)
            .samples()
            .iter()
            .map(|s| (*s as i32).abs())
            .max()
            .unwrap_or(0)
    };
    // Played to a file, as fast as the decoder goes: off, then by song.
    let played = |mode: &str, out: &Path| {
        let player = Player::start(&library, &Output::PcmFile(out.to_path_buf())).unwrap();
        player.set_replaygain(mode).unwrap();
        player.play(vec![item(1, "Tagged", &song)]).unwrap();
        until(&player, "the end of the queue", |s| s.resting);
    };
    let (plain, evened) = (dir.join("plain.wav"), dir.join("evened.wav"));
    played("off", &plain);
    played("track", &evened);
    let (loud, soft) = (peak(&plain), peak(&evened));
    let ratio = soft as f64 / loud as f64;
    assert!(
        (ratio - 0.5).abs() < 0.02,
        "peaks {loud} and {soft}: ratio {ratio}"
    );

    // In real time: the signal says when the samples are scaled, by ReplayGain or by the volume, changed mid-song.
    let player = Player::start(&library, &Output::Silent).unwrap();
    player.set_replaygain("track").unwrap();
    player.play(vec![item(1, "Tagged", &song)]).unwrap();
    let on = until(&player, "the song playing", |s| {
        s.signal.as_ref().is_some_and(|g| g.codec.is_some())
    });
    assert_eq!(
        (on.replaygain, on.signal.as_ref().map(|g| g.scaled)),
        ("track", Some(true))
    );
    player.set_replaygain("off").unwrap();
    until(&player, "nothing scaling the samples", |s| {
        s.replaygain == "off" && s.signal.as_ref().is_some_and(|g| !g.scaled)
    });
    player.set_volume(80.0).unwrap();
    until(&player, "the volume scaling the samples", |s| {
        s.signal.as_ref().is_some_and(|g| g.scaled)
    });
}

#[test]
fn a_song_whose_file_is_gone_is_asked_for_through_the_add_ons() {
    let Some(library) = library() else { return };
    let dir = scratch("gone-file");
    let here = dir.join("here.wav");
    write_wav(&here, RATE, &signal(RATE as usize));
    let gone = dir.join("gone.wav");
    let asked: Arc<Mutex<Vec<Option<i64>>>> = Arc::new(Mutex::new(Vec::new()));
    let resolver: Resolver = {
        let (asked, here) = (Arc::clone(&asked), here.clone());
        Arc::new(move |item: &QueueItem| {
            asked.lock().unwrap().push(item.id);
            Ok(Located {
                locator: here.to_string_lossy().into_owned(),
                tier: Some("lossless".into()),
                addon: Some("somewhere".into()),
            })
        })
    };
    let settings = Settings {
        resolver: Some(resolver),
        ..Settings::default()
    };
    let player = Player::start_with(&library, &Output::Silent, settings).unwrap();
    player.play(vec![item(7, "Gone", &gone)]).unwrap();
    let state = until(
        &player,
        "the song playing from where the add-ons said",
        |s| s.status == "playing" && s.duration_ms > 0,
    );
    assert_eq!(state.served_by.as_deref(), Some("somewhere"));
    assert_eq!(*asked.lock().unwrap(), vec![Some(7)]);
}
