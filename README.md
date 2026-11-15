# Stash desktop

A desktop music client for Windows, macOS and Linux, from the people who make [Stash](https://github.com/rawnaldclark/Stash) for Android.

**Status: early prototype.** There is no app to download yet. What is here is the core and the measurements that decide whether the design holds.

## What it is going to be

- **Your library is the product.** Local files and tracks from add-ons live in one database on your computer, with real offline playback, and everything in it can be exported in an open format.
- **Every source is an add-on.** Stash desktop ships with no music sources. You add the ones you want, in the order you want them asked, and you can see which one served each song. Add-ons speak the same HTTP add-on protocol the Stash Android app uses.
- **A native audio path.** Audio never passes through a web view: gapless playback, an exclusive bit-perfect mode, and quality shown as decoded, not as labelled.
- **Fast and light.** A hundred-thousand-track library should open, scroll and search without a wait.
- **Your phone and your desktop work together.** Import your library from the Android app and the web player; later, control the desktop from the phone and hand playback over.

## How it is built

```
crates/stash-core   the core: library database (SQLite + full-text search), the audio engine
                    (libmpv, loaded at run time) and the add-on client; soon the local API
crates/stashd       the core without a window: a command line for now, the local server later
apps/desktop/ui     the window's UI (Svelte): a track list over 100,000 rows and search, through the local API
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

To see the UI: `cd apps/desktop/ui && npm install && npm run build`, then `target/release/stashd serve --db test.db --ui apps/desktop/ui/dist` and open http://127.0.0.1:47821.

The audio tests need libmpv. On Windows, `powershell -ExecutionPolicy Bypass -File scripts/fetch-mpv.ps1` puts the pinned build in `third_party/mpv/` (it needs 7-Zip). Elsewhere, point `STASH_MPV_LIBRARY` at your system's libmpv. Without it those tests say they were skipped and the rest still run.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
