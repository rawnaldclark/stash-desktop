# Stash desktop

A desktop music client for Windows, macOS and Linux, from the people who make [Stash](https://github.com/rawnaldclark/Stash) for Android.

**Status: early prototype.** There is no app to download yet. What is here: a core that scans a music folder, searches it, plays it gapless, plays from add-ons and takes in a library from the Android app or the web player, a first window for all of that, and the measurements that decide whether the design holds.

## What it is going to be

- **Your library is the product.** Local files and tracks from add-ons live in one database on your computer, with real offline playback, and everything in it can be exported in an open format.
- **Every source is an add-on.** Stash desktop ships with no music sources. You add the ones you want, in the order you want them asked, and you can see which one served each song. Add-ons speak the same HTTP add-on protocol the Stash Android app uses.
- **A native audio path.** Audio never passes through a web view: gapless playback, an exclusive bit-perfect mode, and quality shown as decoded, not as labelled.
- **Fast and light.** A hundred-thousand-track library should open, scroll and search without a wait.
- **Your phone and your desktop work together.** Import your library from the Android app and the web player; later, control the desktop from the phone and hand playback over.

## How it is built

```
crates/stash-core   the core: library database (SQLite + full-text search), folder scanning, the audio
                    engine (libmpv, loaded at run time) and the add-on client
crates/stashd       the core without a window: the local API (`serve`) and commands to scan, measure
                    and check a sound card
apps/desktop/ui     the window's UI (Svelte): a track list over 100,000 rows and search, through the local API
apps/desktop/src-tauri  the desktop shell (Tauri): one program holding the core and the UI, in a window
scripts/            fetch-mpv.ps1 downloads the pinned libmpv build the engine is tested against
```

The window (a Tauri shell around the Svelte UI) talks to the core only through the core's local API (`stashd serve`), the same API a phone remote will use. That keeps the shell replaceable and the core the part that has to be right.

## Measurements

Milestone 1 is a measured prototype: prove the risky parts with numbers before building the product. Results so far are in [docs/measurements.md](docs/measurements.md).

## Build and try

You need [Rust](https://rustup.rs) (stable). On Windows you also need the Visual Studio C++ build tools.

```
cargo test
cargo build --release
target/release/stashd synth --db test.db --tracks 100000
target/release/stashd bench --db test.db
```

`synth` makes a library of made-up tracks (the same every time for the same seed); `bench` measures opening, searching and paging it.

To use your own music instead: `target/release/stashd scan --db my.db <your music folder>` reads the tags of every audio file under the folder into a library. Run it again any time: only new and changed files are read, and files that are gone are marked missing, not deleted.

To see the UI: `cd apps/desktop/ui && npm install && npm run build`, then `target/release/stashd serve --db test.db --ui apps/desktop/ui/dist` and open http://127.0.0.1:47821.

To run the desktop app on that library: build the UI as above, then `cargo build --release -p stash-desktop` and start `target/release/stash-desktop` with `STASH_DB` set to the path of `test.db`. Without `STASH_DB` it opens (and creates) an empty library in your user data folder; scanning is a command for now (above), not a button.

To hear it, the programs need libmpv: copy `third_party/mpv/libmpv-2.dll` beside them in `target/release/`, or set `STASH_MPV_LIBRARY` to its path. Then double-click a track. The bar at the bottom shows what is playing and the signal path: what the engine decoded and what the sound card was opened at. `STASH_EXCLUSIVE=1` takes the sound card for Stash alone. `stashd serve --silent` plays to nowhere, for trying the window without sound.

To bring your library over from Stash on your phone or the web: on the phone, Settings › Library & Storage › Export for Stash on the web; in the web player, Settings › Your library › Save a backup. Then in the window, Import a library (or `target/release/stashd import --db my.db <the file>`). Likes, playlists and plays come in. A song you already have as a file stays that file; any other song plays through your add-ons. Importing only adds, so the same file can be imported again. The way back is the same file: like songs and make playlists here, then Save a library file (or `target/release/stashd export --db my.db <file>`) and import it on the phone or the web.

To try add-ons without having one: `target/release/stashd demo-addon --dir <a music folder>` serves that folder on this computer the way an add-on serves a catalogue, and prints its link. In the window, open Add-ons, paste the link, and search: the add-on's results appear under your library's. Stash desktop comes with no add-ons; each is added by its link, and the link stays in your library file (it can carry your key, so the window never shows it).

The audio tests need libmpv. On Windows, `powershell -ExecutionPolicy Bypass -File scripts/fetch-mpv.ps1` puts the pinned build in `third_party/mpv/` (it needs 7-Zip). Elsewhere, point `STASH_MPV_LIBRARY` at your system's libmpv. Without it those tests say they were skipped and the rest still run.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
