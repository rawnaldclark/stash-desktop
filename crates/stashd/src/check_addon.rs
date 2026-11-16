//! `stashd check-addon`: asks an add-on what Stash would ask it, once each, and says for each whether the answer keeps
//! the contract, as its maker would want to know before giving out its link. One search, one "do you have this
//! song", one stream and one download (the first bytes of each), and lyrics, for the first song the search finds.
//! The add-on's link is never printed: a link can carry its maker's or a listener's key.

use std::io::Read;
use std::time::{Duration, Instant};

use stash_core::addons::{Addon, Delivery, Dialect, StreamInfo, StreamRequest, Track, Wanted};
use stash_core::downloads;

/// How a check went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// It works, but not as well as it could: what Stash does about it is said.
    Warn,
    /// It does not keep the contract: Stash cannot use this part of it.
    Fail,
    /// Not asked: the add-on does not offer it, or there was nothing to ask it with.
    Skipped,
}

/// One check: what was asked, how it went, and why, in words.
#[derive(Debug, Clone)]
pub struct Line {
    pub verdict: Verdict,
    pub what: &'static str,
    pub words: String,
}

/// Slower than this, an answer is said to be slow: the listener waits on a search, and on a stream before a song
/// starts.
const SLOW: Duration = Duration::from_secs(3);

/// Checks the add-on at `link`, searching it for `query`.
pub fn check(link: &str, query: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut say = |verdict: Verdict, what: &'static str, words: String| {
        lines.push(Line {
            verdict,
            what,
            words,
        })
    };
    let started = Instant::now();
    let addon = match Addon::from_link(link) {
        Ok(addon) => addon,
        Err(e) => {
            say(Verdict::Fail, "manifest", e.in_words());
            return lines;
        }
    };
    let m = &addon.manifest;
    say(
        Verdict::Ok,
        "manifest",
        format!(
            "{} {} ({}): {}, {}; offers {} (read in {})",
            m.name,
            m.version,
            m.id,
            match m.dialect {
                Dialect::Eclipse => "made for Eclipse, read through the contract's mapping",
                _ => "Stash's own contract",
            },
            addon.reach().words(),
            m.resources.join(", "),
            took(started)
        ),
    );
    let known = ["search", "resolve", "stream", "download", "lyrics"];
    let unknown: Vec<&str> = m
        .resources
        .iter()
        .map(String::as_str)
        .filter(|r| !known.contains(r))
        .collect();
    if !unknown.is_empty() {
        say(
            Verdict::Warn,
            "manifest",
            format!(
                "Stash does not know {}: they are never asked",
                unknown.join(", ")
            ),
        );
    }
    if !addon.offers_resource("stream") {
        say(
            Verdict::Warn,
            "manifest",
            "it offers no streams: nothing can be played from it".into(),
        );
    }

    // Search: the song the rest is asked about.
    let mut first: Option<Track> = None;
    if addon.offers_resource("search") {
        let asked = Instant::now();
        match addon.search(query, 10) {
            Ok(tracks) => {
                let time = asked.elapsed();
                let empty = tracks
                    .iter()
                    .filter(|t| {
                        t.id.trim().is_empty()
                            || t.title.trim().is_empty()
                            || t.artist.trim().is_empty()
                    })
                    .count();
                let mut ids: Vec<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
                ids.sort_unstable();
                ids.dedup();
                let timed = tracks.iter().filter(|t| t.duration_ms.is_some()).count();
                let covered = tracks.iter().filter(|t| t.art().is_some()).count();
                if tracks.is_empty() {
                    say(
                        Verdict::Warn,
                        "search",
                        format!(
                            "nothing found for \u{201c}{query}\u{201d} (try --query): the rest is not asked"
                        ),
                    );
                } else if empty > 0 {
                    say(
                        Verdict::Fail,
                        "search",
                        format!(
                            "{empty} of {} without an id, a title or an artist",
                            tracks_of(tracks.len())
                        ),
                    );
                } else {
                    say(
                        Verdict::Ok,
                        "search",
                        format!(
                            "{} for \u{201c}{query}\u{201d} in {}; {timed} with a length, {covered} with a cover Stash can show",
                            tracks_of(tracks.len()),
                            ms(time)
                        ),
                    );
                    first = tracks.first().cloned();
                }
                if tracks.len() > 10 {
                    say(
                        Verdict::Warn,
                        "search",
                        format!("{} tracks for 10 asked", tracks.len()),
                    );
                }
                if ids.len() < tracks.len() {
                    say(
                        Verdict::Warn,
                        "search",
                        "two tracks have the same id: Stash cannot tell them apart".into(),
                    );
                }
                if !tracks.is_empty() && timed < tracks.len() {
                    say(
                        Verdict::Warn,
                        "search",
                        format!(
                            "{} without a length: Stash matches a song by its length too",
                            tracks_of(tracks.len() - timed)
                        ),
                    );
                }
                if time > SLOW {
                    say(Verdict::Warn, "search", format!("slow: {}", ms(time)));
                }
            }
            Err(e) => say(Verdict::Fail, "search", e.in_words()),
        }
    } else {
        say(Verdict::Skipped, "search", "not offered".into());
    }

    let wanted = first.as_ref().map(|t| Wanted {
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        duration_ms: t.duration_ms,
        isrc: t.isrc.clone(),
        explicit: t.explicit,
    });
    let nothing = || "no song to ask about: search found none".to_string();

    // "Do you have this song": the song its own search gave.
    if !addon.offers_resource("resolve") {
        say(Verdict::Skipped, "resolve", "not offered".into());
    } else if let Some(wanted) = &wanted {
        let asked = Instant::now();
        match addon.resolve(wanted, None) {
            Ok(Some(found)) => say(
                Verdict::Ok,
                "resolve",
                format!(
                    "found \u{201c}{}\u{201d} again{} in {}",
                    wanted.title,
                    if Some(&found.id) == first.as_ref().map(|t| &t.id) {
                        " (the same track)"
                    } else {
                        ", as another track"
                    },
                    took(asked)
                ),
            ),
            Ok(None) => say(
                Verdict::Warn,
                "resolve",
                format!(
                    "did not find \u{201c}{}\u{201d}, a song its own search gave",
                    wanted.title
                ),
            ),
            Err(e) => say(Verdict::Fail, "resolve", e.in_words()),
        }
    } else {
        say(Verdict::Skipped, "resolve", nothing());
    }

    // A stream, and its first bytes.
    let id = first.as_ref().map(|t| t.id.clone());
    if !addon.offers_resource("stream") {
        say(Verdict::Skipped, "stream", "not offered".into());
    } else if let Some(id) = &id {
        let asked = Instant::now();
        match addon.stream(id, &StreamRequest::default()) {
            Ok(info) => {
                let time = asked.elapsed();
                let (verdict, words) = first_bytes(&addon, &info);
                say(
                    verdict,
                    "stream",
                    format!("{} in {}: {words}", quality(&info), ms(time)),
                );
                if time > SLOW {
                    say(
                        Verdict::Warn,
                        "stream",
                        format!("slow: {} before a song can start", ms(time)),
                    );
                }
            }
            Err(e) => say(Verdict::Fail, "stream", e.in_words()),
        }
    } else {
        say(Verdict::Skipped, "stream", nothing());
    }

    // A download, and its first bytes. An Eclipse add-on's download is its stream.
    if !addon.offers_download() {
        say(Verdict::Skipped, "download", "not offered".into());
    } else if addon.manifest.dialect == Dialect::Eclipse {
        say(
            Verdict::Skipped,
            "download",
            "an add-on made for Eclipse is saved from its stream".into(),
        );
    } else if let Some(id) = &id {
        let asked = Instant::now();
        match addon.download(id, &StreamRequest::default()) {
            Ok(info) => {
                let time = asked.elapsed();
                let (verdict, words) = if info.manifest == Delivery::None {
                    first_bytes(&addon, &info)
                } else {
                    (
                        Verdict::Fail,
                        format!(
                            "a {} download: Stash saves one whole file",
                            info.manifest.as_str()
                        ),
                    )
                };
                say(
                    verdict,
                    "download",
                    format!("{} in {}: {words}", quality(&info), ms(time)),
                );
            }
            Err(e) => say(Verdict::Fail, "download", e.in_words()),
        }
    } else {
        say(Verdict::Skipped, "download", nothing());
    }

    // Lyrics.
    if !addon.offers_resource("lyrics") {
        say(Verdict::Skipped, "lyrics", "not offered".into());
    } else if let Some(wanted) = &wanted {
        let asked = Instant::now();
        match addon.lyrics(id.as_deref(), wanted) {
            Ok(Some(lyrics)) => say(
                Verdict::Ok,
                "lyrics",
                match (&lyrics.synced, &lyrics.plain) {
                    (Some(lines), _) => format!("{} timed lines in {}", lines.len(), took(asked)),
                    (None, Some(text)) => {
                        format!("{} lines of text in {}", text.lines().count(), took(asked))
                    }
                    (None, None) => format!("an empty answer in {}", took(asked)),
                },
            ),
            Ok(None) => say(
                Verdict::Ok,
                "lyrics",
                format!(
                    "none for \u{201c}{}\u{201d} (allowed) in {}",
                    wanted.title,
                    took(asked)
                ),
            ),
            Err(e) => say(Verdict::Fail, "lyrics", e.in_words()),
        }
    } else {
        say(Verdict::Skipped, "lyrics", nothing());
    }
    lines
}

/// How good a stream is, as the add-on says.
fn quality(info: &StreamInfo) -> String {
    let mut parts = vec![info.tier.as_str().to_string()];
    if let Some(codec) = &info.codec {
        parts.push(codec.clone());
    }
    if let (Some(rate), Some(depth)) = (info.sample_rate, info.bit_depth) {
        parts.push(format!("{depth}-bit {:.1} kHz", f64::from(rate) / 1000.0));
    }
    parts.join(" ")
}

/// What the start of a stream's file is, fetched as the core fetches it (held to the add-on's reach, from a range).
fn first_bytes(addon: &Addon, info: &StreamInfo) -> (Verdict, String) {
    let upstream = match downloads::open_stream(&info.url, addon.reach(), Some("bytes=0-")) {
        Ok(upstream) => upstream,
        Err(
            downloads::Trouble::Failed(why)
            | downloads::Trouble::Held(why, _)
            | downloads::Trouble::Away(why)
            | downloads::Trouble::Busy(why, _)
            | downloads::Trouble::Later(why, _)
            | downloads::Trouble::Refused(why, _),
        ) => return (Verdict::Fail, why),
        Err(downloads::Trouble::Cancelled) => return (Verdict::Fail, "cancelled".into()),
    };
    if info.manifest != Delivery::None {
        // A playlist of pieces: read as the relay reads it.
        let mut text = String::new();
        let _ = upstream
            .body
            .take(4 * 1024 * 1024)
            .read_to_string(&mut text);
        let wanted = match info.manifest {
            Delivery::Hls => crate::pieces::Kind::Hls,
            _ => crate::pieces::Kind::Dash,
        };
        return match crate::pieces::kind_of(&text) {
            Some(kind) if kind == wanted => (
                Verdict::Ok,
                format!(
                    "a {} playlist; the engine reads it and its pieces through the core (the pieces are not fetched here)",
                    info.manifest.as_str()
                ),
            ),
            _ => (
                Verdict::Fail,
                format!(
                    "the add-on said a {} playlist, and its address gives something else",
                    info.manifest.as_str()
                ),
            ),
        };
    }
    if !(200..300).contains(&upstream.status) {
        return (
            Verdict::Fail,
            format!("its address answered {}", upstream.status),
        );
    }
    let mut head = Vec::with_capacity(64);
    let _ = upstream.body.take(64).read_to_end(&mut head);
    let Some(kind) = downloads::kind_of(&head) else {
        return (
            Verdict::Fail,
            "what its address gives does not start as an audio file Stash knows".into(),
        );
    };
    let size = upstream
        .length
        .map(|n| format!(" of {:.1} MB", n as f64 / 1_000_000.0))
        .unwrap_or_default();
    if upstream.encoded {
        return (
            Verdict::Warn,
            format!("a {kind} file{size}, sent compressed: Stash cannot seek in it or keep a copy"),
        );
    }
    if !upstream.ranges && upstream.status != 206 {
        return (
            Verdict::Warn,
            format!(
                "a {kind} file{size}; its server does not answer ranges, so a song cannot be picked up where a broken line left it"
            ),
        );
    }
    (Verdict::Ok, format!("a {kind} file{size}, ranges answered"))
}

/// "1 track", "2 tracks".
fn tracks_of(n: usize) -> String {
    if n == 1 {
        "1 track".into()
    } else {
        format!("{n} tracks")
    }
}

fn took(since: Instant) -> String {
    ms(since.elapsed())
}

fn ms(time: Duration) -> String {
    format!("{} ms", time.as_millis())
}
