//! The player, in real time but without a sound card: a queue plays through, and pause, seek, next and previous do
//! what they say. Needs libmpv (see `common::library`); without it the tests say so and pass.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{RATE, library, scratch, signal, write_wav};
use stash_core::audio::Output;
use stash_core::player::{Player, PlayerState, QueueItem};

fn item(id: i64, title: &str, path: &Path) -> QueueItem {
    QueueItem {
        id,
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
        (3, Some((1, "One")))
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
        s.index == Some(1) && s.track.as_ref().is_some_and(|t| t.id == 2)
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
    assert_eq!(state.error.as_deref(), Some("Gone could not be played"));
    assert_eq!(state.track.map(|t| t.id), Some(2));

    // A new queue while the old one is on the same place in its own (here: the second track of two, then the first
    // of one, then the first of another) is followed too.
    player.play(vec![item(3, "Again", &b)]).unwrap();
    until(&player, "the new queue's first track", |s| {
        s.status == "playing" && s.track.as_ref().is_some_and(|t| t.id == 3)
    });
    player.play(vec![item(4, "And again", &b)]).unwrap();
    until(&player, "another queue, first track to first track", |s| {
        s.status == "playing" && s.track.as_ref().is_some_and(|t| t.id == 4) && s.index == Some(0)
    });

    player.stop().unwrap();
    let stopped = until(&player, "stopped", |s| s.status == "idle");
    assert_eq!(stopped.queue_len, 0);
    let _ = std::fs::remove_dir_all(&dir);
}
