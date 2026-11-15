//! `stashd`: the Stash desktop core without a window: it scans a folder into a library, serves the local API every
//! shell talks to, and has the tools the measurements are made with.

mod output_check;

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
    /// Run the core's local API on this computer (and serve the built UI, if a folder is given).
    Serve {
        #[arg(long)]
        db: PathBuf,
        /// The folder of the built UI (index.html and its files).
        #[arg(long)]
        ui: Option<PathBuf>,
        #[arg(long, default_value_t = 47821)]
        port: u16,
        /// Write the measurement the UI posts to this file, then stop.
        #[arg(long)]
        bench_out: Option<PathBuf>,
        /// The libmpv file for the player (else STASH_MPV_LIBRARY, else one beside this program).
        #[arg(long)]
        mpv: Option<PathBuf>,
        /// Take the sound card for Stash alone: each track at its own format, other programs silent.
        #[arg(long)]
        exclusive: bool,
        /// Run without a player.
        #[arg(long)]
        no_audio: bool,
        /// Play to nowhere, at the speed of a sound card: everything works, nothing is heard.
        #[arg(long)]
        silent: bool,
    },
    /// Play a moment of silence at several formats and report what the sound card opened at for each.
    OutputCheck {
        /// The libmpv file to use (else STASH_MPV_LIBRARY, else one beside this program).
        #[arg(long)]
        mpv: Option<PathBuf>,
        /// Take the device for Stash alone. Other programs are silent while each file plays.
        #[arg(long)]
        exclusive: bool,
        /// Also print the audio output's own log lines.
        #[arg(long)]
        verbose: bool,
    },
    /// Read a music folder into the library: new files are added, changed ones read again, missing ones marked.
    Scan {
        #[arg(long)]
        db: PathBuf,
        /// The folder to scan, with everything below it.
        folder: PathBuf,
    },
    /// Write made-up tracks as real tagged files (for measuring a scan).
    SynthFiles {
        /// The folder to write into.
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value_t = 5_000)]
        tracks: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// How long each file is, in seconds.
        #[arg(long, default_value_t = 0.1)]
        seconds: f32,
        /// Put a made-up cover inside each file (the same for the same album), to see pictures in the window.
        #[arg(long)]
        covers: bool,
    },
    /// Bring in a library from Stash on a phone or the web player: the file "Export for Stash on the web" or
    /// "Save a backup" wrote, or the Android app's backup (.zip) or its database (stash.db). Only adds; importing
    /// the same file again changes nothing.
    Import {
        #[arg(long)]
        db: PathBuf,
        /// The file: stash-library-YYYY-MM-DD.json, a backup .zip, or stash.db.
        file: PathBuf,
    },
    /// Write the library as the file Stash on a phone or the web reads (likes, playlists, plays), to import there.
    Export {
        #[arg(long)]
        db: PathBuf,
        /// The file to write (for example stash-library.json).
        file: PathBuf,
    },
    /// Serve a folder of your own music as an add-on on this computer, to try add-ons in the window with.
    DemoAddon {
        /// The folder to serve, with everything below it.
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value_t = 47822)]
        port: u16,
        /// Hold every search and stream answer back this long, to see the window wait.
        #[arg(long, default_value_t = 0)]
        delay_ms: u64,
        /// Answer the way an add-on made for Eclipse does, to try that kind of add-on with.
        #[arg(long)]
        eclipse: bool,
        /// Offer search and streams only: no answer to "do you have this song".
        #[arg(long)]
        no_resolve: bool,
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

fn import(db: &Path, file: &Path) -> Result<()> {
    let started = Instant::now();
    let incoming = stash_core::import::read_file(file)?;
    let read = started.elapsed();
    let opening = Instant::now();
    let mut library = Library::open(db).with_context(|| format!("opening {}", db.display()))?;
    let opened = opening.elapsed();
    let merging = Instant::now();
    let summary = library.import(&incoming)?;
    let merged = merging.elapsed();
    println!(
        "{} likes, {} playlists, {} plays and {} other songs in the file{}",
        incoming.likes.len(),
        incoming.playlists.len(),
        incoming.plays.len(),
        incoming.songs.len(),
        incoming
            .generator
            .as_deref()
            .map(|g| format!(" (from {g})"))
            .unwrap_or_default()
    );
    println!(
        "new songs {}, here already {}; likes {} (+{} here); playlists {} new, {} merged (+{} songs); plays {} (+{} here)",
        summary.songs_added,
        summary.songs_already,
        summary.likes_added,
        summary.likes_already,
        summary.playlists_added,
        summary.playlists_merged,
        summary.playlist_songs_added,
        summary.plays_added,
        summary.plays_already
    );
    let counts = library.counts()?;
    println!(
        "library: {} tracks, {} liked, {} playlists (file read {:.2} s, library opened {:.2} s, merged {:.2} s)",
        counts.tracks,
        counts.liked,
        counts.playlists,
        read.as_secs_f64(),
        opened.as_secs_f64(),
        merged.as_secs_f64()
    );
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Synth { db, tracks, seed } => synth_library(&db, tracks, seed),
        Command::Bench { db, json } => bench(&db, json),
        Command::Import { db, file } => import(&db, &file),
        Command::Export { db, file } => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let started = Instant::now();
            let library =
                Library::open(&db).with_context(|| format!("opening {}", db.display()))?;
            let out =
                library.export(now_ms, concat!("Stash desktop ", env!("CARGO_PKG_VERSION")))?;
            let text = out.to_string();
            std::fs::write(&file, &text).with_context(|| format!("writing {}", file.display()))?;
            let count = |name: &str| out[name].as_array().map(Vec::len).unwrap_or(0);
            println!(
                "{} likes, {} playlists, {} plays written to {} ({:.1} MB, {:.2} s)",
                count("likes"),
                count("playlists"),
                count("history"),
                file.display(),
                text.len() as f64 / 1e6,
                started.elapsed().as_secs_f64()
            );
            Ok(())
        }
        Command::DemoAddon {
            dir,
            port,
            delay_ms,
            eclipse,
            no_resolve,
        } => {
            let addon = stashd::demo_addon::DemoAddon::bind_with(
                &dir,
                port,
                Duration::from_millis(delay_ms),
                eclipse,
                !no_resolve,
            )?;
            println!(
                "{} tracks. Add this link in Stash: {}",
                addon.tracks(),
                addon.link()
            );
            addon.run();
            Ok(())
        }
        Command::Scan { db, folder } => scan(&db, &folder),
        Command::SynthFiles {
            dir,
            tracks,
            seed,
            seconds,
            covers,
        } => {
            let written = synth::write_files(&dir, tracks, seed, seconds, covers)
                .with_context(|| format!("writing into {}", dir.display()))?;
            println!("{written} files written under {}", dir.display());
            Ok(())
        }
        Command::OutputCheck {
            mpv,
            exclusive,
            verbose,
        } => output_check::run(mpv, exclusive, verbose),
        Command::Serve {
            db,
            ui,
            port,
            bench_out,
            mpv,
            exclusive,
            no_audio,
            silent,
        } => stashd::serve::serve(stashd::serve::Options {
            db,
            ui,
            port,
            bench_out,
            audio: match (no_audio, silent) {
                (true, _) => None,
                (false, true) => Some(stash_core::audio::Output::Silent),
                (false, false) => Some(stash_core::audio::Output::Device { exclusive }),
            },
            mpv,
            play_counts_between_seconds: None,
        }),
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

fn scan(db: &Path, folder: &Path) -> Result<()> {
    let mut library = Library::open(db).with_context(|| format!("opening {}", db.display()))?;
    let report = stash_core::scan::scan_folder(&mut library, folder, |so_far| {
        eprintln!(
            "  {} files: {} added, {} changed, {} unchanged ({:.1} s)",
            so_far.files, so_far.added, so_far.changed, so_far.unchanged, so_far.seconds
        );
    })?;
    println!(
        "{} audio files in {:.1} s ({:.0} files/s): {} added, {} changed, {} unchanged, {} missing, {} unreadable",
        report.files,
        report.seconds,
        report.files as f64 / report.seconds.max(0.001),
        report.added,
        report.changed,
        report.unchanged,
        report.missing,
        report.unreadable
    );
    for example in &report.unreadable_examples {
        println!("  unreadable: {example}");
    }
    let counts = library.counts()?;
    println!(
        "library: {} tracks, {} albums, {} artists",
        counts.tracks, counts.albums, counts.artists
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
