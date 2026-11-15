//! Measures how songs are found on add-ons against the labelled sets the Stash Android app is measured with:
//!
//! - the matcher sets (`data/download/src/test/resources/matcher/corpus-v1.json`, `corpus-v2.json.gz`): real library
//!   songs, the top results a public catalogue search gave for each, and a verdict per result (match, wrong_version,
//!   wrong_song, unsure) labelled under the apps' version rules;
//! - the YouTube titles (`core/common/src/test/resources/youtube/titles-v1.tsv`): library rows brought in from
//!   YouTube, and the song each one is.
//!
//! They live in the Android app's repository; point `STASH_ANDROID_REPO` at a checkout of it to run these. Without
//! it the tests say they were skipped. Each song is asked for the way the library asks (`youtube_title::asked_for`
//! for a row from YouTube) and picked with `matching::pick`.
//!
//! Per song the outcome is one of: correct (a match was picked), wrong (a wrong version or another song was picked:
//! the listener hears the wrong thing), missed (nothing was picked although a match was listed), or correctly
//! nothing. The numbers below are a ratchet: a change may not make them worse; lower them when it makes them better.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::PathBuf;

use serde::Deserialize;
use stash_core::addons::{Track, Wanted};
use stash_core::matching::pick;
use stash_core::youtube_title::{self, tokens};

/// (most wrong picks, most misses) allowed. 2026-11-15, v1 (777 songs): 3 / 73 at first; 5 / 21 once titles and
/// credits were read the way real catalogues write them and a video's length was no longer held against a result;
/// v2 (3,682 songs) 14 / 306 then. With YouTube rows read as the songs they name (and, failing that, as kept or the
/// other way round), Spotify's "(with …)" credits and unbracketed "feat." tails: v1 3 / 21, v2 17 / 142. With a result
/// credited the same way preferred to one that names the artist only somewhere in its credit, and versions written
/// after a credit, beside a title in another script or as a live album "From …" kept apart: v1 2 / 21, v2 17 / 142.
/// (The phone app's matcher: v1 8 / 18, v2 33 / 105.)
const V1_MAX: (usize, usize) = (2, 21);
const V2_MAX: (usize, usize) = (17, 142);
/// Of the labelled YouTube rows, how many (percent) are read as the right artist and title. The phone app: 91.7.
const MIN_TITLES_RIGHT: f64 = 91.6;

fn android_repo() -> Option<PathBuf> {
    let repo = std::env::var_os("STASH_ANDROID_REPO").map(PathBuf::from);
    if repo.is_none() {
        eprintln!("SKIPPED: STASH_ANDROID_REPO is not set");
    }
    repo
}

#[derive(Deserialize)]
struct Corpus {
    version: u32,
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    source: String,
    query: Query,
    candidates: Vec<Candidate>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Query {
    artist: String,
    title: String,
    album: Option<String>,
    duration_ms: Option<u64>,
    isrc: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    cid: String,
    title: String,
    #[serde(default)]
    version: String,
    artist: String,
    #[serde(default)]
    album: String,
    #[serde(default)]
    duration_sec: u64,
    verdict: String,
}

/// Scores one set; returns (wrong, missed) after printing every wrong pick and miss and the totals.
fn score(corpus: &Corpus) -> (usize, usize) {
    let (mut correct, mut wrong, mut missed, mut nothing, mut unsure) = (0, 0, 0, 0, 0);
    let mut by_source: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    for item in &corpus.items {
        let as_kept = Wanted {
            title: item.query.title.clone(),
            artist: item.query.artist.clone(),
            album: item.query.album.clone(),
            duration_ms: item.query.duration_ms,
            isrc: item.query.isrc.clone(),
            explicit: None,
        };
        let (wanted, also) = if item.source == "YOUTUBE" {
            youtube_title::asked_for(as_kept)
        } else {
            (as_kept, Vec::new())
        };
        // A catalogue names a version beside the title ("Song", "Live From 2025"); an add-on's title carries it.
        let found: Vec<Track> = item
            .candidates
            .iter()
            .map(|c| Track {
                id: c.cid.clone(),
                title: if c.version.trim().is_empty() {
                    c.title.clone()
                } else {
                    format!("{} {}", c.title, c.version)
                },
                artist: c.artist.clone(),
                album: (!c.album.is_empty()).then(|| c.album.clone()),
                duration_ms: (c.duration_sec > 0).then_some(c.duration_sec * 1000),
                isrc: None,
                explicit: None,
                max_tier: None,
                artwork: Vec::new(),
            })
            .collect();
        let tally = by_source.entry(item.source.clone()).or_default();
        tally[2] += 1;
        let asked = format!(
            "({}) {} - {} [{}s]",
            item.source,
            wanted.artist,
            wanted.title,
            wanted.duration_ms.unwrap_or(0) / 1000
        );
        // As `Described::pick` does.
        let picked = std::iter::once(&wanted)
            .chain(&also)
            .find_map(|way| pick(way, &found))
            .map(|t| item.candidates.iter().find(|c| c.cid == t.id).unwrap());
        let listed = item.candidates.iter().find(|c| c.verdict == "match");
        match (picked, listed) {
            (Some(c), _) if c.verdict == "match" => correct += 1,
            (Some(c), _) if c.verdict == "unsure" => unsure += 1,
            (Some(c), _) => {
                wrong += 1;
                tally[0] += 1;
                println!(
                    "WRONG {asked} picked {} - {} {} [{}s] ({})",
                    c.artist, c.title, c.version, c.duration_sec, c.verdict
                );
            }
            (None, Some(should)) => {
                missed += 1;
                tally[1] += 1;
                println!(
                    "MISSED {asked} | match: {} - {} {} [{}s]",
                    should.artist, should.title, should.version, should.duration_sec
                );
            }
            (None, None) => nothing += 1,
        }
    }
    println!(
        "matcher corpus v{}: {} songs: {correct} correct, {wrong} wrong, {missed} missed, {nothing} correctly nothing, {unsure} unsure; [wrong, missed, songs] by source {by_source:?}",
        corpus.version,
        corpus.items.len()
    );
    (wrong, missed)
}

fn check(name: &str, corpus: &Corpus, (max_wrong, max_missed): (usize, usize)) {
    let (wrong, missed) = score(corpus);
    assert!(
        wrong <= max_wrong,
        "{name}: {wrong} wrong picks (at most {max_wrong})"
    );
    assert!(
        missed <= max_missed,
        "{name}: {missed} misses (at most {max_missed})"
    );
}

#[test]
fn the_matcher_picks_the_right_song_from_real_search_results() {
    let Some(repo) = android_repo() else { return };
    let sets = repo.join("data/download/src/test/resources/matcher");
    let v1: Corpus =
        serde_json::from_slice(&std::fs::read(sets.join("corpus-v1.json")).unwrap()).unwrap();
    let mut v2 = String::new();
    flate2::read::GzDecoder::new(std::fs::File::open(sets.join("corpus-v2.json.gz")).unwrap())
        .read_to_string(&mut v2)
        .unwrap();
    let v2: Corpus = serde_json::from_str(&v2).unwrap();
    check("v1", &v1, V1_MAX);
    check("v2", &v2, V2_MAX);
}

#[test]
fn youtube_rows_are_read_as_the_songs_they_are() {
    let Some(repo) = android_repo() else { return };
    let rows =
        std::fs::read_to_string(repo.join("core/common/src/test/resources/youtube/titles-v1.tsv"))
            .unwrap();
    let (mut total, mut right) = (0usize, 0usize);
    for line in rows
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let fields: Vec<&str> = line.split('\t').collect();
        let (title, artist, want_artist, want_title) = (fields[0], fields[1], fields[2], fields[3]);
        let parsed = youtube_title::parse(title, artist);
        let want: HashSet<String> = tokens(want_artist);
        let got = tokens(&parsed.artist);
        let artist_ok = got == want || (!want.is_empty() && got.is_superset(&want));
        let title_ok = tokens(&parsed.title) == tokens(want_title);
        total += 1;
        if artist_ok && title_ok {
            right += 1;
        } else if std::env::var_os("STASH_SHOW_TITLES").is_some() {
            println!(
                "{title} | {artist} -> {} | {} (want {want_artist} | {want_title})",
                parsed.artist, parsed.title
            );
        }
    }
    let percent = 100.0 * right as f64 / total as f64;
    println!("youtube titles v1: {total} rows, {percent:.1}% artist and title right");
    assert!(
        percent >= MIN_TITLES_RIGHT,
        "{percent:.1}% < {MIN_TITLES_RIGHT}%"
    );
}
