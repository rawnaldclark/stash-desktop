//! Gapless playback, measured: two files cut from one continuous signal must come out of the engine as that signal,
//! sample for sample, with nothing added or dropped at the join and the audio output opened once.
//!
//! Needs libmpv (see `common::library`). Without it the tests say so and pass, so a checkout without the engine still
//! builds and tests the rest.

mod common;

use std::fs;

use common::{
    RATE, assert_same_samples, library, play_to_file, read_wav, scratch, signal, write_wav,
};
use stash_core::audio::EndReason;

#[test]
fn two_files_cut_from_one_signal_play_as_that_signal_sample_for_sample() {
    let Some(library) = library() else { return };
    let dir = scratch("gapless");
    // The cut is not on a zero crossing or a block boundary: an inserted gap or a lost frame cannot hide.
    let (first, second) = (30_011usize, 40_007usize);
    let whole = signal(first + second);
    let (a, b, out) = (dir.join("a.wav"), dir.join("b.wav"), dir.join("out.wav"));
    write_wav(&a, RATE, &whole[..first * 2]);
    write_wav(&b, RATE, &whole[first * 2..]);

    let played = play_to_file(
        &library,
        &[&a.to_string_lossy(), &b.to_string_lossy()],
        &out,
    );
    assert_eq!(
        played.ended,
        [EndReason::Eof, EndReason::Eof],
        "log:\n{}",
        played.log_text()
    );
    assert_eq!(
        played.output_opened,
        1,
        "the audio output was opened once for both tracks; log:\n{}",
        played.log_text()
    );

    let wav = read_wav(&out);
    assert_eq!(
        (wav.format, wav.channels, wav.rate, wav.bits),
        (1, 2, RATE, 16),
        "the output is the files' own format: nothing was converted"
    );
    assert_same_samples(&wav.samples(), &whole, first);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_new_sample_rate_reopens_the_output_instead_of_resampling() {
    let Some(library) = library() else { return };
    let dir = scratch("rate-change");
    let (a, b, out) = (dir.join("a.wav"), dir.join("b.wav"), dir.join("out.wav"));
    write_wav(&a, 44_100, &signal(22_050));
    write_wav(&b, 96_000, &signal(48_000));

    let played = play_to_file(
        &library,
        &[&a.to_string_lossy(), &b.to_string_lossy()],
        &out,
    );
    assert_eq!(
        played.ended,
        [EndReason::Eof, EndReason::Eof],
        "log:\n{}",
        played.log_text()
    );
    assert_eq!(
        played.output_opened,
        2,
        "one output per format; log:\n{}",
        played.log_text()
    );
    // The file output starts over when it is reopened, so it holds the second track alone, at its own rate.
    let wav = read_wav(&out);
    assert_eq!((wav.rate, wav.data.len() / 4), (96_000, 48_000));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_cannot_be_opened_ends_with_an_error_and_the_next_one_plays() {
    let Some(library) = library() else { return };
    let dir = scratch("missing");
    let (missing, b, out) = (
        dir.join("not-there.wav"),
        dir.join("b.wav"),
        dir.join("out.wav"),
    );
    write_wav(&b, RATE, &signal(4_410));

    let played = play_to_file(
        &library,
        &[&missing.to_string_lossy(), &b.to_string_lossy()],
        &out,
    );
    assert!(
        matches!(played.ended[0], EndReason::Error(_)),
        "{:?}",
        played.ended
    );
    assert_eq!(played.ended[1], EndReason::Eof);
    assert_eq!(read_wav(&out).data.len() / 4, 4_410);
    let _ = fs::remove_dir_all(&dir);
}
