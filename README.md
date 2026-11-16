# Stash desktop

A desktop music client for Windows, macOS and Linux, from the people who make [Stash](https://github.com/rawnaldclark/Stash) for Android.

**Status: early prototype.** There is no app to download yet. What is here: a core that scans a music folder, searches it, plays it gapless, plays from add-ons and takes in a library from the Android app or the web player, a window for all of that with the look of the Stash web player, and the measurements that decide whether the design holds.

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
apps/desktop/ui     the window's UI (SvelteKit, built to static files): the Stash web player's screens, on top of
                    the local API
apps/desktop/src-tauri  the desktop shell (Tauri): one program holding the core and the UI, in a window
scripts/            fetch-mpv.ps1 downloads the pinned libmpv build the engine is tested against
```

The window (a Tauri shell around the UI) talks to the core only through the core's local API (`stashd serve`), the same API a phone remote will use. That keeps the shell replaceable and the core the part that has to be right.

The UI is the Stash web player's, ported: its screens, styles and fonts. Only the UI came across. Everything behind the screens is one file written for the desktop (`src/lib/app.ts`) on top of the core's API; nothing of the web player's accounts, sync or sharing is here.

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

`synth` makes a library of made-up tracks (the same every time for the same seed); `bench` measures opening, searching and paging it. `stashd synth-files --dir <folder> --covers` writes made-up tracks as real tagged files with covers, for trying the window on something that looks like music.

To use your own music instead: `target/release/stashd scan --db my.db <your music folder>` reads the tags of every audio file under the folder into a library. Run it again any time: only new and changed files are read, and files that are gone are marked missing, not deleted.

To see the UI: `cd apps/desktop/ui && npm install && npm run build`, then `target/release/stashd serve --db test.db --ui apps/desktop/ui/dist` and open http://127.0.0.1:47821.

To run the desktop app on that library: build the UI as above, then `cargo build --release -p stash-desktop` and start `target/release/stash-desktop` with `STASH_DB` set to the path of `test.db`. Without `STASH_DB` it opens (and creates) an empty library in your user data folder; Settings › Your library › Add a music folder scans one in.

To hear it, the programs need libmpv: copy `third_party/mpv/libmpv-2.dll` beside them in `target/release/`, or set `STASH_MPV_LIBRARY` to its path. Then click a song. The bar at the bottom shows what is playing, its format as the engine decoded it, and one word for what happened on the way to the sound card (Bit-perfect, Lossless, Resampled or Lossy). Settings › Sound chooses the output it plays through (kept, and taken back when it is plugged in again), evens out loudness from the songs' ReplayGain tags (by song or by album), has the switch that takes the sound card for Stash alone, and Autoplay, which goes on with songs like the last ones from your library when the queue ends. The bar says Bit-perfect only when nothing touches the samples: the sound card for Stash alone, the volume at 100 and no ReplayGain on that song. The system's media controls (the keyboard's media keys, the volume overlay) work the window, the bar's mini player button (or the tray's) swaps it for a small window that stays on top, and the Lyrics button shows a song's words in time: a `.lrc` file beside the song or the lyrics in its tags first, then your add-ons, then LRCLIB (Settings › Lyrics switches that off). `stashd serve --silent` plays to nowhere, for trying the window without sound.

To bring your library over from Stash on your phone or the web: on the phone, Settings › Library & Storage › Export for Stash on the web; in the web player, Settings › Your library › Save a backup. Then in the window, Settings › Your library › Import (or `target/release/stashd import --db my.db <the file>`). The phone's Backup (.zip, from Settings › Library & Storage › Backup) is taken too, and brings more: every play, your downloads and your saved albums. Likes, playlists and plays come in. A song you already have as a file stays that file; any other song plays through your add-ons. Importing only adds, so the same file can be imported again. The way back is the same file: like songs and make playlists here, then Save a library file (or `target/release/stashd export --db my.db <file>`) and import it on the phone or the web.

To try add-ons without having one: `target/release/stashd demo-addon --dir <a music folder>` serves that folder on this computer the way an add-on serves a catalogue, and prints its link. In the window, open Settings › Add-ons, paste the link, and search: the add-on's results appear under your library's. A song's menu has Download, and Liked and each playlist have a download button: the song is saved as a file (in Stash in your Music folder; Library › Downloads can change that), tagged and with its cover, and from then on plays with no connection. Library › Downloads lists what is saved and what is on the way. A song none of your add-ons has, when you play or save it, goes on Library › Not found: open it, pick the add-on's song that is it, and it plays from there from then on. Ctrl+K opens a box that finds anything in your library and does anything the window can. Shuffle spreads each artist's songs out, so two by one artist rarely come together. A smart playlist fills itself by rules (liked and not played in three months, most played this month, lossless only) and changes as your library does. What you play can go to ListenBrainz, or a server that speaks its language, and waits on this computer while there is no connection. Offline, Stash plays only what is on this computer (your files, songs saved, and songs kept as they played) and asks no add-on. Shortcuts that work in any app (play or pause, skip, the volume, like the song playing) can be switched on in Settings. A playlist, or your liked songs, can be kept on this computer: their songs are saved, and so are songs added later. Library › Songs can list only the songs on this computer, and does so by itself while Stash is offline. Any song or artist can be left out of what Stash plays on its own ("Don't play"), and the phone's blocklist comes over on import. Songs can be picked in any list (Ctrl or Shift+click, Ctrl+A) and played, queued, added to a playlist, liked, downloaded or taken out of a playlist together. Playlists can be grouped in folders: carry one onto a folder in the sidebar, or use its menu; a folder folds shut in the sidebar and opens in Library › Playlists. A song by several artists ("A & B", "A feat. B") links each to their own page, and an artist's page lists their songs with others. A playlist file from another player (.m3u, .m3u8: foobar2000, MusicBee, VLC) comes in as a playlist (the + in the sidebar, or Settings), and a playlist or your likes go out as one. A song played from an add-on is kept on this computer as it plays (Settings › Your library › Keep songs you play), so next time it plays with no connection and without asking the add-on. Stash desktop comes with no add-ons; each is added by its link, and the link stays in your library file (it can carry your key, so the window never shows it).

The audio tests need libmpv. On Windows, `powershell -ExecutionPolicy Bypass -File scripts/fetch-mpv.ps1` puts the pinned build in `third_party/mpv/` (it needs 7-Zip). Elsewhere, point `STASH_MPV_LIBRARY` at your system's libmpv. Without it those tests say they were skipped and the rest still run.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
