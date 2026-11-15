//! `stashd`: the Stash desktop core without a window. For now it makes a test library and measures it; it will
//! grow the audio engine, the add-on client and the local API that every shell talks to.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::json;
use stash_core::{Library, synth};

#[derive(Parser)]
#[command(name = "stashd", version, about = "The Stash desktop core")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Make a library of made-up tracks (the same for the same seed) to measure with.
    Synth {
        /// The database file to create. It must not exist yet.
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value_t = 100_000)]
        tracks: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Measure opening, searching and paging a library, as a list or grid would use it.
    Bench {
        #[arg(long)]
        db: PathBuf,
        /// Print the numbers as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Synth { db, tracks, seed } => synth_library(&db, tracks, seed),
        Command::Bench { db, json } => bench(&db, json),
    }
}

fn synth_library(db: &Path, tracks: usize, seed: u64) -> Result<()> {
    if db.exists() {
        bail!(
            "{} already exists: choose a new file, a library is never overwritten",
            db.display()
        );
    }
    let started = Instant::now();
    let mut library = Library::open(db).with_context(|| format!("creating {}", db.display()))?;
    let added = library.add_tracks(synth::tracks(tracks, seed))?;
    let took = started.elapsed();
    let counts = library.counts()?;
    drop(library);
    println!(
        "{added} tracks, {} albums, {} artists in {:.1} s ({:.0} tracks/s); {} on disk",
        counts.albums,
        counts.artists,
        took.as_secs_f64(),
        added as f64 / took.as_secs_f64(),
        megabytes(file_bytes(db))
    );
    Ok(())
}

/// The database file and its write-ahead log.
fn file_bytes(db: &Path) -> u64 {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    size(db) + size(Path::new(&wal))
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

struct Timing {
    name: String,
    detail: String,
    p50: Duration,
    p95: Duration,
    max: Duration,
}

/// Runs `work` `runs` times and keeps the spread. The first run is included: a list's first page is a cold one too.
fn measure<T>(
    name: &str,
    runs: usize,
    mut work: impl FnMut() -> Result<T>,
    detail: impl Fn(&T) -> String,
) -> Result<Timing> {
    let mut times = Vec::with_capacity(runs);
    let mut last = None;
    for _ in 0..runs {
        let started = Instant::now();
        let out = work()?;
        times.push(started.elapsed());
        last = Some(out);
    }
    times.sort();
    let at = |q: f64| times[((times.len() as f64 - 1.0) * q).round() as usize];
    Ok(Timing {
        name: name.to_string(),
        detail: last.as_ref().map(&detail).unwrap_or_default(),
        p50: at(0.50),
        p95: at(0.95),
        max: *times.last().unwrap(),
    })
}

fn bench(db: &Path, as_json: bool) -> Result<()> {
    if !db.exists() {
        bail!(
            "{} does not exist: make one with `stashd synth --db <file>`",
            db.display()
        );
    }
    let started = Instant::now();
    let library = Library::open(db)?;
    let counts = library.counts()?;
    let open = started.elapsed();
    let n = counts.tracks as usize;
    let albums = counts.albums as usize;
    if n == 0 {
        bail!("the library is empty");
    }

    let mut timings: Vec<Timing> = Vec::new();
    let hits = |rows: &Vec<stash_core::TrackRow>| format!("{} rows", rows.len());

    // Search as typed: one letter pair, a growing prefix, common and rare words, several words, an artist's number.
    for query in [
        "lo",
        "lov",
        "love",
        "kaleidoscope",
        "quasar xyl",
        "golden river",
        "night of the",
        "love night blue heart",
        "phoenix 4242",
        "zzzz",
    ] {
        timings.push(measure(
            &format!("search \"{query}\""),
            40,
            || Ok(library.search(query, 50)?),
            hits,
        )?);
    }

    // A track list: the first page, a page after dragging the scrollbar to the middle and to the end, and the next
    // page while scrolling on from the middle.
    let page = 200usize;
    for (label, offset) in [
        ("top", 0),
        ("middle", n / 2),
        ("end", n.saturating_sub(page)),
    ] {
        timings.push(measure(
            &format!("tracks at {label} (offset {offset}, {page} rows)"),
            40,
            || Ok(library.tracks_at(offset, page)?),
            hits,
        )?);
    }
    let middle = library.tracks_at(n / 2, 1)?;
    if let Some(row) = middle.first() {
        let cursor = (row.sort_key.clone(), row.id);
        timings.push(measure(
            &format!("tracks after the middle row ({page} rows)"),
            40,
            || Ok(library.tracks_after(Some((&cursor.0, cursor.1)), page)?),
            hits,
        )?);
    }

    // An album grid: a screenful of tiles at the top and in the middle.
    let tiles = 120usize;
    for (label, offset) in [("top", 0), ("middle", albums / 2)] {
        timings.push(measure(
            &format!("albums at {label} (offset {offset}, {tiles} tiles)"),
            40,
            || Ok(library.albums_at(offset, tiles)?),
            |rows| format!("{} tiles", rows.len()),
        )?);
    }

    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    if as_json {
        let out = json!({
            "library": { "tracks": counts.tracks, "albums": counts.albums, "artists": counts.artists, "bytes": file_bytes(db) },
            "open_ms": ms(open),
            "timings": timings.iter().map(|t| json!({ "name": t.name, "result": t.detail, "p50_ms": ms(t.p50), "p95_ms": ms(t.p95), "max_ms": ms(t.max) })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "library: {} tracks, {} albums, {} artists, {}",
            counts.tracks,
            counts.albums,
            counts.artists,
            megabytes(file_bytes(db))
        );
        println!("open + counts: {:.1} ms", ms(open));
        println!(
            "{:<58} {:>9} {:>9} {:>9}  result",
            " ", "p50 ms", "p95 ms", "max ms"
        );
        for t in &timings {
            println!(
                "{:<58} {:>9.2} {:>9.2} {:>9.2}  {}",
                t.name,
                ms(t.p50),
                ms(t.p95),
                ms(t.max),
                t.detail
            );
        }
    }
    Ok(())
}
