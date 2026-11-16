# Measurements

Milestone 1 asks four questions, each answered with numbers on real hardware:

1. Does a 100,000-track library open, search and page fast enough that a list never waits? **Measured below.**
2. Does the audio engine play gapless, and in an exclusive bit-perfect mode? **Measured below, exclusive mode on one real device included.**
3. Does a list of 100,000 rows and an album grid scroll smoothly in the window? **The track list: measured below, in a browser window and in the desktop shell. The album grid: not yet.**
4. What do start-up time and memory look like for the whole app? **Measured below.**

## 1. The library database

**Machine:** Intel Core i7-2600 (2011, 4 cores / 8 threads, 3.4 GHz), 16 GB RAM, SATA SSD, Windows 10. A fifteen-year-old desktop, on purpose: numbers that are fine here are fine anywhere.

**Library:** `stashd synth --tracks 100000 --seed 1`: 100,000 tracks, 9,092 albums, 3,642 artists, 44.6 MB on disk. Titles are one to five words from a 110-word vocabulary, skewed so a few words are very common ("love" matches tens of thousands of rows) and some are rare.

**Method:** `stashd bench`, release build, 40 runs per line, the first (cold) run included. Times are for the query as the core runs it, rows decoded, before any UI.

| | p50 ms | p95 ms | max ms | result |
|---|---:|---:|---:|---|
| Open the library and count it | 5.8 | | | |
| Search "lo" | 1.2 | 1.5 | 2.6 | 50 rows |
| Search "lov" | 1.2 | 1.2 | 1.5 | 50 rows |
| Search "love" | 1.2 | 1.4 | 1.8 | 50 rows |
| Search "kaleidoscope" | 2.1 | 2.2 | 3.5 | 50 rows |
| Search "quasar xyl" | 0.8 | 1.0 | 1.1 | 0 rows |
| Search "golden river" | 21.5 | 22.5 | 23.6 | 50 rows |
| Search "night of the" | 24.6 | 25.4 | 26.2 | 50 rows |
| Search "love night blue heart" | 42.2 | 45.2 | 75.6 | 50 rows |
| Search "phoenix 4242" | 0.8 | 0.8 | 1.0 | 0 rows |
| Search "zzzz" | 0.5 | 0.5 | 0.6 | 0 rows |
| Track list, top (200 rows) | 1.4 | 1.7 | 2.5 | 200 rows |
| Track list, middle (offset 50,000) | 3.6 | 4.1 | 5.7 | 200 rows |
| Track list, end (offset 99,800) | 5.9 | 6.1 | 7.8 | 200 rows |
| Track list, next page while scrolling | 1.1 | 1.2 | 1.4 | 200 rows |
| Album grid, top (120 tiles) | 0.9 | 1.1 | 1.7 | 120 tiles |
| Album grid, middle | 1.1 | 1.2 | 1.6 | 120 tiles |

Building the library took 8.0 s (12,500 tracks a second), in one transaction.

### What the numbers changed

- **Broad searches needed their own path.** The first version ranked every match. "lo", "lov" and "love" each matched tens of thousands of rows and took 170 to 190 ms, too slow to search as you type. Now the matches are counted first, stopping at 2,000. Up to that, they are ranked (title above artist above album). Above it, the query is broad and no ranking would be useful, so titles that start with what was typed come first, from the sorted title index, and other matches follow. Those three searches now take 1.2 ms.
- **Every word is a prefix**, because people stop typing a word once it is enough ("radioh cre"). That costs the multi-word searches over very common words some time: 21 to 45 ms here. It is fast enough to type against; it is the first place to look if search ever feels slow.
- **Jumping anywhere in a list is cheap.** A page at any scroll position walks only the sort index to the position and then reads the rows of that page: 6 ms at the very end of 100,000 rows. Scrolling on from a row costs about 1 ms wherever the row is.

### Not covered yet

Measured on one machine and one made-up library. Real libraries have longer titles, more words, non-Latin scripts and messy tags. A second run on a real collection's tags, and on a slower disk, would say more.

## 2. The audio engine

The engine is libmpv (client API 2.6, the build `scripts/fetch-mpv.ps1` pins), loaded at run time. The tests in `crates/stash-core/tests/gapless.rs` send the engine's output to a WAV file holding exactly the samples a sound card would receive, and compare them with what went in.

| Question | How it was asked | Result |
|---|---|---|
| Is a join between two tracks gapless? | One continuous stereo signal (441 Hz left, 663 Hz right, 16-bit, 44.1 kHz) is cut into two WAV files at frame 30,011, not on a zero crossing or a block boundary, and the two are played one after the other. | The output is the uncut signal **sample for sample**: 70,018 frames in, 70,018 out, no sample added, dropped or changed. The audio output was opened once for both tracks. |
| Is the path bit-exact? | The same comparison: any resampling, dither or volume scaling would change sample values. | Every sample is identical, and the output's format is the files' own (16-bit, 44.1 kHz, stereo). |
| What happens when the next track has another sample rate? | A 44.1 kHz file followed by a 96 kHz file. | The output is closed and opened again at 96 kHz; the second track is not resampled. The join is then not gapless, which is the trade-off of never resampling. |
| What does a file that cannot be opened do? | A missing file followed by a real one. | The missing one ends with an error and the next one plays. |
| Is a join between two add-on streams gapless? | A small add-on server on the same computer is added by its link, searched, and asked for two streams (the same two halves of the signal, served over HTTP). The two stream URLs are played one after the other. `tests/addon_stream.rs` | The same result as for files: the uncut signal sample for sample, the output opened once. |

### On a real sound card

`stashd output-check` plays half a second of digital silence at four formats and reports what the audio output opened at. On the test machine's sound card, a basic USB audio device, through WASAPI:

| File | Shared mode | Exclusive mode |
|---|---|---|
| 16-bit 44.1 kHz | 48 kHz float (resampled by the system mixer's format) | **16-bit 44.1 kHz**, the file's own format |
| 16-bit 48 kHz | 48 kHz float | **16-bit 48 kHz**, the file's own format |
| 24-bit 96 kHz | 48 kHz float (resampled) | 16-bit 48 kHz: the device refuses every 24-bit and every 96 kHz format |
| 24-bit 192 kHz | 48 kHz float (resampled) | 16-bit 48 kHz, for the same reason |

So exclusive mode does what it is for: with nothing else in the path, a CD-quality file reaches the device in its own format, where shared mode converts everything to the mixer's 48 kHz. And when the hardware cannot take a format, the engine says which one it fell back to. That is what a signal-path display has to show: not "lossless" as labelled, but what was opened.

### Not covered yet

- **Exclusive mode on more devices.** It is checked on one basic USB device (below). A device that accepts hi-res formats, and what a format change mid-playlist sounds like on real hardware, are not.
- **Compressed formats.** WAV has no encoder delay. Gapless FLAC should behave the same; AAC, MP3 and Opus carry padding the decoder must trim, and need their own test files.
- **Streams over a real network.** The add-on test runs on one computer, where the next stream is ready at once. Over the internet the join depends on the next stream being open in time (`prefetch-playlist`); that needs a slow server to measure against. DASH and HLS streams are not measured either.
- **Crossfade.** mpv has none; it needs a mixer of our own.

## 3. The window

The UI (`apps/desktop/ui`, Svelte) reaches the library only through the core's local API (`stashd serve`). Its track list holds just the rows on screen and a few around them, and asks for pages of 200 as it scrolls; a row whose page has not arrived is drawn as a placeholder.

**How it was measured.** `stashd serve --db <100,000 tracks> --ui apps/desktop/ui/dist --bench-out result.json`, and the page opened with `?bench` in Microsoft Edge 157 (Chromium, the engine the desktop shell's web view uses on Windows), as an app window of 1384 × 861 on a 75 Hz display, with a fresh profile each time. The page drives itself and posts its numbers to the core. Three runs; the ranges below cover all three. Same machine as above.

| | Result |
|---|---|
| First rows on screen | The page's script starts 440 to 510 ms after the window begins loading (the browser's own start), the first screenful has its data 110 to 120 ms later, and is painted by 635 to 695 ms. |
| Jump anywhere in the library (30 jumps) to real rows on screen | 40 to 48 ms typical, 54 to 58 ms at the 95th percentile, 80 ms at worst |
| Steady scroll, 225 rows a second, 240 frames | Every frame on time: 13.3 ms typical, 14.3 ms at worst (a 75 Hz frame is 13.3 ms). No row was ever blank. |
| Hard fling, 2,250 rows a second, 240 frames | 13.3 ms typical, 13.4 ms at the 95th percentile; one frame of 26.6 ms in one run, none late in the others. On average 3 to 4 % of the rows on screen were placeholders, and for a frame or two the whole screen was. |
| Search while typing "golden river", each letter until its results are on screen | 28 to 32 ms typical, 40 to 46 ms at worst |
| JavaScript heap at the end | 4.5 to 7.9 MB |

### What the numbers say

- **The list is not the bottleneck.** With 100,000 rows the frame time is the display's, at a steady scroll and in a fling.
- **A fling outruns the pages.** At 2,250 rows a second the list crosses a page of 200 rows every 90 ms, and placeholders show for a moment. Asking for the next page in the direction of travel before it is needed would close that.
- **A jump costs about three frames**: one request to the core and the draw. It is the round trip, not the query (the page at the end of the library takes 6 ms in the core).

### Not covered yet

- The album grid, with cover art.
- A window that is covered by another one is not drawn by Chromium, so the measurement was run with occlusion tracking switched off.

## 4. The whole app

The desktop shell (`apps/desktop/src-tauri`, Tauri 2.12 on the system's WebView2 157) is one program of 10.9 MB holding the shell, the core and the built UI. It starts the core's local API on a free port inside its own process and opens a 1400 × 900 window on the UI. The library is the same 100,000 tracks; the machine is the same 2011 i7.

**Start-up**, from starting the program to the first screenful of real rows painted, timed from outside the program:

| Launch | Page begins loading | First rows have data | Painted |
|---|---:|---:|---:|
| First ever (the web view sets up its data folder) | 2,506 ms | 2,624 ms | 2,654 ms |
| Second | 533 ms | 647 ms | 667 ms |
| Third | 658 ms | 798 ms | 819 ms |

Nearly all of it is the web view starting. From the page beginning to load, the first rows take about 115 to 140 ms.

**In the window**, the same self-measurement as in section 3, three runs: a jump anywhere shows real rows in 40 to 48 ms (53 to 70 ms at the 95th percentile); a steady scroll never misses a frame; a 2,250 rows-a-second fling is on time at the 95th percentile (13.4 to 13.5 ms) with at most 1.3 % of frames late; search answers each typed letter in 33 to 38 ms. The shell changes nothing measurable against the plain browser window.

**Memory**, summed over the program and the six web view processes below it:

| | Private | Working set |
|---|---:|---:|
| The program itself (shell and core, library open) | 5.5 to 6.5 MB | 25 to 30 MB |
| Everything, window open and untouched for 12 s | 155 MB | 295 MB |
| Everything, right after the scroll and search runs | 244 to 258 MB | 369 to 399 MB |

### What the numbers say

- **The core is small; the web view is the cost.** The program that holds the library, the API and (later) the audio engine uses about 6 MB. The system web view's browser, GPU, renderer and utility processes are the other 150 MB at rest. That is the price of a web UI on any shell that uses one; what a Tauri shell avoids is shipping the browser (the program is 10.9 MB) and paying for a second copy of it.
- **A warm start is under a second** on a fifteen-year-old CPU. The first-ever start pays about two seconds once.

### Not covered yet

- Memory with the audio engine playing, with cover art in an album grid, and after hours of use.
- macOS (WKWebView) and Linux (WebKitGTK), where the web view is a different engine. Linux is the known risk for this shell.
- A packaged install (installer size, code signing, the first-run experience on a PC without WebView2).

## 5. Scanning a folder

`stashd scan --db <library> <folder>` walks a folder, reads the tags of every audio file it has not seen (or whose size or time changed) and writes them in batches of 500. A file that is gone is marked missing, never deleted.

**How it was measured.** `stashd synth-files --tracks 5000` writes 5,000 small WAV files with ID3 tags (179 artists, 454 albums, 39 MB), then two scans into a new library. Same machine.

| | Time | Rate |
|---|---:|---:|
| First scan: every file's tags read, 5,000 rows written | 0.9 s | 5,800 files a second |
| Second scan, nothing changed: no file opened | 0.1 s | 34,000 files a second |

This is a best case: small files that the system had just written and still held in memory, on an SSD. A first scan of a real collection is bound by reading the files from disk (or a network share), not by this code. The re-scan number is the one that carries over: checking 100,000 unchanged files should take about three seconds.

### Not covered yet

A real collection (large FLAC files with embedded pictures, a spinning disk, a network share), watching a folder for changes instead of re-scanning, and files moved or renamed (today: one missing, one added).

## 6. Playing from the window

Not a measurement, a check of the whole path: library, local API, player, engine, sound card, and back to the window.

- **In a browser, on a silent output** (`stashd serve --silent`, 30 scanned tracks of 12 s): double-clicking a row plays it and queues the rows after it; the playing row is marked; the queue moves on by itself at the end of a track; pause holds the position; next and previous move through the queue; a click three quarters along the bar lands at 0:09 of 0:12; a double-click in search results plays from that result with the results after it; Space pauses when the search box is not in use; stopping returns the bar to rest.
- **In the desktop app, on the real sound card** (the same library; the files are digital silence): the engine beside the program is found, the chosen row plays with the rest queued, and the signal path reads `PCM 16-bit 44.1 kHz → shared 32-bit float 48 kHz, resampled, format changed`. That is the truth for this device in shared mode (section 2): the bar says so in the colour for "changed on the way".
- **By the tests** (`tests/player.rs`, `crates/stashd/tests/api.rs`): the same controls against the engine directly and over HTTP, on a silent output.

### Not covered yet

Exclusive mode from the window (it is an environment switch, `STASH_EXCLUSIVE=1`, not a setting yet), a queue view, volume, shuffle and repeat.

## 7. Add-ons in the window

Again a check of the whole path, not a measurement: an add-on added by its link, searched, and played from. The add-on is the demo add-on (`stashd demo-addon`): a folder of files served on this computer through the add-on contract (manifest, search, stream, media over HTTP with ranges), with every search and stream answer held back 0.3 to 0.7 s to stand in for a real one. No real add-on was used: that takes the listener's own link.

How the player treats an add-on's track: where it plays from is a link the add-on makes on request, which costs the listener's allowance there and runs out. So the stream is asked for when the listener starts the track, or when the track before it has 30 s left, and not when the queue is made. The engine still has the next track before the current one ends.

- **In a browser, on a silent output:** a link that is not an add-on is refused with the reason and nothing is kept; the demo's link adds it (name, version, server, what it offers); the search box then searches the library at every letter and the add-on 0.35 s after the last one, with its own heading and "Asking…" until it answers; a double-click on one of its rows shows the track at once with "Asking Demo folder for the stream…" and then plays it; the bar names the add-on and what it said the stream is (`Demo folder, lossless`) beside what the engine reports; a click half way along the bar seeks in the stream; next goes straight on when the engine already has the next track; a track the add-on does not have is passed over and named in a strip above the bar; switched off, the add-on is no longer asked.
- **In the desktop app, on the real sound card:** the same through the app's own core: added, searched (4 tracks for "night"), the first played after its 0.3 s of asking (`PCM 16-bit 44.1 kHz → shared 32-bit float 48 kHz`, the add-on having said lossless), and the queue moved on to the add-on's next track by itself.
- **By the tests:** `tests/player.rs` (an add-on's track is asked for late, not at queue time; a missing one is passed over; next and previous before the engine has the track; trouble with the add-on is said once and the queue is not marched through) and `crates/stashd/tests/api.rs` (add, refuse, list without the link, search, switch off, play over HTTP with a seek, kept across a restart, remove).

### Not covered yet

- **The join between two add-on tracks handed over late is not measured.** Section 3 proves the join bit-exact when the engine has both streams; here the second arrives up to 30 s before the first ends, by the same engine mechanism, but that path has no sample-for-sample proof of its own (the output that writes samples to a file does not run in real time).
- A real add-on: DASH and HLS streams, an add-on that asks for a key, answers that arrive after seconds rather than tenths. Add-ons made for Eclipse and its family (a manifest without `stash`) are read through the contract's mapping: checked against the contract's own examples (every manifest, search, resolve, stream and error example, when their folder is given to the test) and end to end against the demo add-on speaking that way (`stashd demo-addon --eclipse`); their settings, catalogs and albums are not used yet, and request headers a stream asks for are not sent.
- A stream link that runs out while the track is paused for hours, and reading an add-on's manifest again after it changes.
- The bar says when an add-on's "lossless" is in fact a lossy codec, or its "hi-res" is CD format. That check is written and has not been seen to fire: the demo add-on tells the truth.

## 8. Importing a library from the phone or the web player

The Android app (Settings › Library & Storage › Export for Stash on the web) and the web player (Settings › Your library › Save a backup) write the same file, `stash-web-library` version 1: likes, playlists in order and plays, each song as its description (title, artist, album, length, ISRC, the ids other sources know it by). The desktop reads that file with the rules the other two use and merges it by their rule for "the same song" (the same words, or the same ISRC; two different ISRCs are two recordings). The reader's test fixture is the golden file the Android app's tests write and the web player's tests read.

A song the library already has as a file keeps being the file and gains the like, the playlist place and what it lacked (a length, an ISRC). A song it does not have becomes a row known by its description; when it is about to play, the listener's add-ons are asked for it in their order (the add-on contract's `resolve`: the add-on says which of its tracks is the same song, the host refuses an answer whose length is off by more than 8 s or 3 %), and the name the add-on gave is kept with the song for next time.

### Speed (i7-2600, a made-up phone library into a library of 100,000 files)

The file: 7.1 MB, 14,000 likes, 150 playlists of 120 songs, 5,000 plays; 18,128 different songs.

| | |
|---|---|
| Reading and checking the file | 0.21 s |
| Merging it (18,128 new songs, 14,000 likes, 150 playlists, 5,000 plays) | 3.80 s |
| The same file again (nothing new, everything recognised) | 0.90 s |
| Opening a 100,000-track library made by an earlier build for the first time (the tracks table is made again for the new kind of song, once) | 7.16 s |
| Afterwards, with 118,128 tracks: open 5 ms, list pages 0.7 to 4.6 ms, search 0.3 to 24 ms at p95 | as before |

### Checked by hand

- **In a browser, on a silent output:** the window's Import a library took a phone-style file (7 likes, 2 playlists, 2 plays; two of its songs were files already in the library) and said `Imported from Stash for Android 0.9.125: 11 new songs, 7 likes, 2 playlists, 2 plays. 2 songs were here already.`; the sidebar then listed Liked (7) and the two playlists with their counts; the liked file of this computer carried its heart and played as the file; an imported song showed `Asking your add-ons for this song…` and then played from the demo add-on, the bar naming the add-on and `lossless`; a liked song no add-on has was passed over and named.
- **In the desktop app, on the real sound card:** the same file through the app's own core; importing it again changed nothing; an imported song played from the demo add-on (`PCM 16-bit 44.1 kHz → shared 32-bit float 48 kHz`); the liked file played as the file.
- **By the tests:** `tests/import.rs` (the golden file; a second import; a like landing on a file; the ISRC rule; a playlist that is here gaining only what it lacks; what is not a library file, and what does not fit, refused or left out) and `crates/stashd/tests/api.rs` (import over HTTP, the lists, imported songs through an add-on, a stale name replaced, a recording of another length refused).

### Not covered yet

- **The phone's own library is read too (added after this section was first written):** the Android app's Backup archive (.zip) or its database (`stash.db`), by the app's own export rules, plus what only a database holds: every play, the downloaded songs and the songs of saved albums. Checked by tests on a database made up in the app's shape, and once on a real phone's library copied off a debug build (read as a copy; the second import changed nothing). Not checked: a backup archive written by a real phone (the test archives are made here), and databases of older app versions (missing columns are tolerated by design, not by trial). The optional whole-history section of the shared file (`listens`) is still ignored.
- **Songs with the same title and artist are one song** when neither has an ISRC, also when they come from different albums or differ in length. That is the shared rule of all three Stashes; on a library without ISRCs it folds a single and its album version together, and can fold two different versions.
- **No real add-on has answered `resolve` here**, only the demo add-on, which matches by the same words and a close length. How well a real add-on picks the same song is that add-on's doing; the desktop does not second-guess it beyond the length check.
- **An add-on that can only search** (many made for Eclipse offer search and streams and no way to be asked for a song) is searched for the song, and a result is taken only when it is the same song by the contract's rule (`matching.rs`: the same title once release labels are set aside, the same artist, a length within 8 s or 3 %; a live take, a remix or an edit is another song). It is strict on purpose and will leave out songs a looser match would find. It is checked by its own tests, end to end against the demo add-on, and against the labelled sets the Android app is measured with (`crates/stash-core/tests/matcher_corpus.rs`; set `STASH_ANDROID_REPO` to a checkout of the Android repository to run it): real library songs, a public catalogue's top results for each, and whether each result is the song. On 777 songs it picks wrongly 2 times and misses 21; on 3,682 songs, 17 and 142 (the Android app's matcher: 8 / 18 and 33 / 105). A version written after a featuring credit ("Song feat. X (Live)"), beside a title in another script ("紅蓮華 - Instrumental") or as a live album the song is "From" is another song, and a result credited the same way is preferred to one whose credit only names the artist somewhere. A song brought in from YouTube is first read as the song its title names, by a port of the Android app's reader (`youtube_title.rs`: 91.7 % of 6,839 labelled titles read right, as on the phone), and its length counts only for an artist's own audio (a Topic channel or a row with an album), since an upload's length is the video's; when that reading finds nothing, the row as kept and the other way round ("Song - Artist") are tried on the same results (and asked of an add-on that resolves songs, and of LRCLIB for lyrics). A saved file is tagged with the row as kept, not with a reading. Most wrong picks left are such songs with no length to go by, where the catalogue has a live take or a re-recording under the same plain title: text cannot tell them apart. The catalogue in those sets is not an add-on, so how an add-on's own search ranks songs is not measured. What an add-on did not have is remembered for six hours, in memory only. Settings in a web backup are not imported. Covers are kept as addresses and not shown yet.

## 9. Likes, playlists and plays made here, and the way back out

Section 8 brought a library in. This is the other half: what the listener does on the desktop, and the same file written back out for the phone and the web player.

- **Liking:** a heart beside every song (shown when the row is pointed at, always when liked). On an add-on's search result, liking first makes the track a song of the library (the row the library has for that song, or a new one known by its description, with the add-on's id for it).
- **Playlists:** made from the sidebar or from the + beside a song; renamed and deleted from the playlist's own header; a song is added from its +, and in a playlist moved up or down or taken out. A shared mix someone else owns can be deleted here but not filled.
- **Plays:** the core counts a play by the other Stashes' rule: once half the song was really played (not paused, not waiting for data), never before 30 s and always by 4 minutes, stamped with when it started. Only songs of the library are kept; a track played straight from an add-on's results is not.
- **Save a library file** (or `stashd export --db my.db <file>`): the library as `stash-web-library` version 1. A file is described, never located; a file without tags is left out.

### Checked

- **By the web player's own reader:** a file written by the desktop (the golden library imported, plus a like, a playlist and a play made here) was read by the web player's `readBackup` in its own test runner. It read 4 likes under the same identities the desktop uses (`halo|beyonce`, `isrc:USUM71607009`), the 3 playlists (the one made here, the imported one, the shared mix still a follow with its version and sharer) and the 4 plays.
- **In a browser, on a silent output:** with the mouse: a heart clicked (Liked went to 1), the + opened the menu with the name field ready, a new playlist was made with the song in it. Then every control once: adding to a playlist that is there (and being told when it has the song already), moving up, taking out, renaming, a name of spaces refused in words, liking and adding an add-on's search result (the library grew by that one song), taking a like back in the Liked list (the row left), making a playlist from the sidebar, deleting one after being asked, and saving the file (the download itself was held back; its name and contents were checked).
- **In the desktop app, on the real sound card:** a 70 s song played from a playlist made there: after 20 s no play was kept, after 38 s one was, under the real rule; the file then held the like, the playlist in its order and that play.
- **By the tests:** `tests/lists.rs` (likes, playlists, adoption, plays, the golden file out and back in, nothing that only works here leaving), `tests/player.rs` (a play counts once half was really played; paused time and a skipped song do not), `crates/stashd/tests/api.rs` (all of it over HTTP, and the core importing its own file changes nothing).

### Not covered yet

- **The Android app's reader has not read a desktop file.** It is the same format and the web player's reader accepts it, but the phone's import was not run. The phone merges a playlist with an `app-…` id back into its own playlist of that number only when the names agree.
- **Saving the file from inside the desktop app's own window** was not clicked (the save goes through the web view's download); `stashd export` and the core's answer were checked instead.
- A like taken back here does not travel: importing only ever adds, on every Stash. Reordering is one step at a time (no dragging). No undo. Export speed on a large library was not measured.

## 10. The basics of daily use

Volume, the queue, the sound card setting and scanning from the window. Plain controls in the window as it is; where they sit is for the design pass to decide.

- **Volume:** a slider in the bar; it is the engine's own volume and is kept between runs. In exclusive mode anything but 100 changes the samples, and the Sound panel says so.
- **Queue:** a panel beside the list with what is playing and what follows; a double-click plays from that place, and the queue stays whole.
- **Sound:** "Take the sound card for Stash alone" is a setting now, kept between runs, and can be switched while a track plays.
- **Music folders:** "Add a music folder" asks for a folder with the system's own dialog and scans it in the background through a database connection of its own, with a running count; the folder is remembered, and "Scan again" reads the remembered folders.

### Checked

- **Switching exclusive mode while a track plays, on the real sound card** (a file of digital silence): shared `16-bit 44.1 kHz → 32-bit float 48 kHz, resampled`; switched on, `16-bit 44.1 kHz → 16-bit 44.1 kHz`, mode exclusive, not resampled, the track still playing; switched off, shared again. The position stood still for about a second at each switch while the output was opened again.
- **In a browser, on a silent output:** the slider moved the core's volume and showed it again after a reload; the queue panel listed the queue with the playing track marked and the played ones dimmed; a double-click on the sixth played it and the list followed; the Sound panel's switch changed the core's setting and back; a folder of 300 files was scanned (0.05 s) and remembered, "Scan More again" appeared and said nothing had changed.
- **By the tests:** `tests/player.rs` (volume, queue, jump), `tests/lists.rs` (settings and folders kept), `crates/stashd/tests/api.rs` (a scan from the API to its end, a second scan finding one new file, volume and setting kept by a core opened later, the queue and a jump over HTTP).

### Not covered yet

- **The folder dialog itself was not clicked** (it is the system's own window): the route that opens it is untested; scanning a named folder is.
- A scan cannot be stopped once started, a remembered folder cannot be forgotten from the window, and folders are not scanned by themselves when the app starts or when files change.
- The queue can be looked at and jumped into, not edited (no removing, reordering, "play next"). No shuffle or repeat.

## 11. The window, with the web player's look

The window is now the Stash web player's UI, ported: the sidebar, Home, Search, Library, the playlist, album and artist pages, the floating player bar, the Lyrics / Up next panel and the full Now playing screen, with its styles and fonts and the colour taken from the cover that is playing. Only the UI came across. The screens talk to one boundary written for the desktop on top of the core's local API.

What the desktop adds to it:

- **Library** has Songs (the whole library on the virtual list, searched in place), Liked, Playlists, Albums, Artists and History.
- **The bar and Now playing say what the sound is:** the format as decoded, and one word for the way to the sound card: Bit-perfect, Lossless, Resampled or Lossy. For a stream, which add-on it came from and what the add-on said it was.
- **Settings:** Sound, Add-ons, Your library (music folders, import, save a library file), Appearance, About.
- **A file's own cover.** A file in a music folder had no picture; only songs another Stash described did. A file row's picture is now read out of the file (the front cover in its tags), else `cover`, `folder`, `front` or `album` `.jpg/.png/.webp` beside it. Whether a file has one is found out the first time it is asked for and kept, so a file without is asked for once.
- **The queue is edited from the window:** play next, add to queue, remove, drag to reorder, shuffle, repeat, save as a playlist.

### Measured

Same machine, same 100,000-track library, same self-measurement as sections 3 and 4 (`?bench`, which now measures Library › Songs). A row is 56 px with a cover, a heart and a menu; the old one was a 34 px line of text.

In Edge (1384 × 861, 75 Hz, fresh profile):

| | Old plain list | New window, as ported | New window, rows reused |
|---|---|---|---|
| Jump anywhere to real rows | 40 to 48 ms typical | 51 ms | 45 to 53 ms typical, 66 to 77 ms at the 95th percentile |
| Steady scroll | every frame on time | every frame on time | every frame on time |
| Hard fling: frames | 13.3 ms typical, at most one late | 26.5 ms typical, 50 % late | 13.3 ms typical, 13.4 to 13.5 ms at the 95th percentile, 1.3 to 2.1 % late |
| Hard fling: rows on screen that were placeholders | 3 to 4 % | 26 % | none |
| Search while typing, each letter | 28 to 32 ms typical | 42 ms | 39 to 40 ms typical, 70 to 92 ms at worst |
| JavaScript heap at the end | 4.5 to 7.9 MB | 24 MB | 11 to 13 MB |

In the app (1400 × 900, three launches): first rows painted 870 to 1,270 ms after starting the program (the first launch ever is the slow one; it was 670 to 820 ms warm with the old list); a jump shows real rows in 63 to 67 ms; a steady scroll never misses a frame; in a fling 6 to 10 % of frames are late, each by one frame, and no row is blank; search answers a letter in 46 to 49 ms typical, about 100 ms at worst. Memory right after the scroll and search runs, summed over the program and its web view processes: 370 to 420 MB private, 430 to 440 MB working set (it was 244 to 258 and 369 to 399). The program itself is still about 8 MB of that.

### What the numbers say

- **A richer row needs a list that does not build rows while it scrolls.** The list used to make a new row and throw one away for every row that scrolled past. With text rows that was free; with the web player's rows a fling missed every other frame. The rows on screen are now a fixed set of boxes that are given the row that arrived and moved there. That alone brought the fling back on time.
- **Reading one page ahead removed the placeholders.** The page beyond the rows drawn, in the direction of travel, is asked for early. Section 3 said this would close the gap; it did.
- **The look costs memory, and some frames in the app.** About 120 MB more across the web view's processes than the plain list, most of it in the GPU process, and a fling in the app drops a frame now and then where the plain list did not. The translucent bar that blurs the list moving under it is the first place to look.

### Checked

- **Every screen, in a browser on a silent output** (a made-up library: 600 tagged files with covers, 53 songs described by another Stash, 4 playlists, and the demo add-on): Home, the six Library tabs, an album of files and one of described songs, an artist, a playlist, search with the library's and the add-on's results, a track played straight from the add-on, a described song found and played through the add-on (Now playing named the add-on and "lossless"), Settings, and a 960 × 640 window. No errors from the page. A reload on any screen gets the page again (the core answers a screen's address with the page).
- **In the real app, on the real sound card:** the page is on one origin and the core on another, and everything crossed: the lists, covers read from files, the colour taken from a cover, playing a file (the bar said `PCM · 16-bit/44.1 kHz`, Resampled, which is what the shared 48 kHz output does to it), and Save a library file.
- **By the tests:** `crates/stashd/tests/api.rs` (a screen's address is the page and a missing file is missing; a file's picture, the folder's picture, a file without one stops offering it, an unplugged file is not judged, the picture's answer varies by origin), `crates/stash-core/src/covers.rs`, and the UI's own 53 tests (`npm test`).

### Not covered yet

- **Full-size covers.** The made-up covers are tiny. A folder of albums with 3,000 px pictures inside every file sends each one whole to a 40 px box; a small-picture cache in the core is the fix when it shows.
- **Bit-perfect and exclusive wording on screen** were read from the engine's report, not looked at in the new bar with exclusive mode on.
- **Lyrics:** the panel is there and says there are none; the core has no lyrics yet (since section 13 it has).
- The folder dialog and Import were not clicked in the new Settings; their routes are the ones already tested.

## 12. Opening where it was left

- **The queue is kept between runs.** What was queued, the place in it, the seconds into the track, shuffle and repeat are written into the library's settings as they change, and brought back when the program opens: paused where it was, nothing playing until asked. A song of the library is kept by its row, an add-on's track whole. A song that is gone by then is left out. Nothing relies on a clean exit: the place is written every five seconds while a track plays, and to the second when paused.
- **The player rests on a track while nothing plays.** At the end of the queue that is the last track, and a track that could not be played is the one it rests on. The state says paused with the track, so the bar stays, and play starts it. Songs added to a resting queue are only added.
- **Search finds albums and artists in the library**, a dozen of each beside the songs: an album by its title or its songs' artist, an artist by name, names that start with what was typed first. The card at the top is the artist or the album when its name is what was typed.
- **The window opens at the size and place it had.**
- **A window under 1,100 px keeps the volume, Up next and the format line in the bar.** The web player's medium layout drops them; a desktop window that size has nothing else.

### Checked

- **In the real app, killed in the middle of a queue** (the process ended, not closed): 646 songs queued from the library, paused 7 s into the first, repeat on. Opened again, the bar showed the same song, paused at 7 s, the same queue and repeat; pressing play in the bar went on from there on the sound card.
- **In a browser on a silent output:** a one-song queue played to its end left the bar on that song, paused; "lanterns" put the artist The Lanterns on the top card with an Artists row and an Albums row under it; at 960 x 640 the bar had the volume, Up next and the format line.
- **By the tests:** `crates/stash-core/tests/player.rs` (resting at the end of the queue, a seek and previous while resting, adding to a resting queue, a kept queue brought back and played on from its second), `crates/stashd/tests/api.rs` (the queue written down, a second core opening on it with a song gone in between, nothing kept after stop; albums and artists in a search).

### Not covered yet

- A kept queue of an add-on's tracks was not restarted in the real app (the path is the same as for the library's; the stream is asked for again when play is pressed).
- The window's remembered size was not checked by resizing it by hand; the run above opened at the size it was closed at.
- Search does not ask add-ons for albums or artists, only for songs.

## 13. Working with the rest of the computer

- **One Stash at a time.** Starting it again while it runs brings the open window to the front; the second start ends by itself.
- **The system's media controls** (media keys, the volume overlay, the lock screen) show the song, its artist, album, cover and place, and their play, pause, next, previous and seek work the player. Stop pauses.
- **The sound output is chosen in Settings › Sound**: the system's own choice, or any output of the system's sound driver. The change is heard at once, also in the middle of a song, and the choice is kept. When the chosen output is not connected, the system's choice plays, Settings says which one is missing, and the chosen one is taken back within five seconds of being plugged in again.
- **Even out loudness** (Settings › Sound): off, by song or by album, from the ReplayGain (or R128) gain in a song's tags, heard at once. The bar's Bit-perfect now also needs the volume at 100 and no ReplayGain on the song: before, a volume below 100 in exclusive mode still said Bit-perfect though the samples were scaled.
- **Lyrics.** A song's own come first: a `.lrc` file beside it (read each time, so one dropped in shows at once), or the lyrics in its tags. Then the add-ons that offer lyrics, the one serving the song asked by its own id. Then LRCLIB, an open database of timed lyrics, unless Settings › Lyrics switches it off; it is sent the song's title, artist, album and length. What came from the network is kept in the library for 30 days, and "none there" for a day; a miss is not kept when a source could not be asked. What any source gives is cleaned and capped as the add-on contract says.

### Checked

- **In the real app on a scratch library with scratch user folders** (so it opened "its own" library as a listener's would): through the system's media session, aimed at Stash's session only (no key presses, which would reach whatever else plays), next, play/pause, a position and previous all moved the player, and the session's title followed the song. A second start ended by itself, the first kept playing.
- **ReplayGain, sample for sample:** a song with a track gain of -6.02 dB in its tags, played to a file by song, came out at half the peak of the same song played with it off (`crates/stash-core/tests/player.rs`); in real time the signal said the samples were scaled with it on, not after switching it off mid-song, and again when the volume went to 80. In the app the chips switched the core mid-song.
- **The output switched in the middle of a song** to another of five outputs and back: the same song played on, from where it was.
- **Lyrics in the panel**: a `.lrc` beside a song showed in the panel while it played, with the line of the moment lit. One LRCLIB lookup of a well-known song came back in 280 ms with 50 timed lines; asked again, 11 ms from the library.
- **By the tests:** `crates/stash-core/tests/player.rs` (the outputs listed, the system's choice first, one chosen and back), `crates/stash-core/src/lyrics.rs` (LRC with several stamps a line and an offset, plain text, a source's lyrics checked, the LRCLIB search result picked by length and name), `crates/stashd/tests/api.rs` (an output chosen, kept for the next run, refused when not there, missing when unplugged; lyrics from a `.lrc`, from an add-on by its id and by title and artist, kept when the add-on is gone, the LRCLIB switch). No test reaches LRCLIB.

### Not covered yet

- Unplugging an output while it plays was not tried with a real device; the path is the engine's own fallback and the five-second check.
- Word-by-word lyrics are shown when an add-on gives them; LRCLIB and `.lrc` files give lines only.
- The full-size covers and memory notes of section 11 still stand.

- **The tray icon** (by the clock): its tip says what plays, a click brings the window back, and its menu plays or pauses, skips either way, shows the window and quits. With Settings › Appearance › Keep playing when the window is closed, closing the window hides it and the music goes on; off (the default), closing ends Stash. The choice is read about once a second by the tray, so closing the window never waits on the core (an import holds the library for a while), and the menu asks the core off the window's thread. Checked in the real app on scratch user folders, the window closed with the message its close button sends: on, it kept playing with the window hidden, and starting Stash again brought it back; off, it ended. The tray menu itself was not clicked in a check. On Linux, a desktop with no tray host (GNOME without its AppIndicator extension) may still report a tray, and closing would then hide the window with no icon to bring it back but starting Stash again; not tried.
- **The mini player** (the bar's button beside Now playing, or the tray's menu): a small window that stays above the others, with the song, its cover, where it is in it, previous, play or pause, next and like; drawn tall, the cover fills it. It has no frame and is moved by its background; it opens where it was last left. Only one shows at a time: it hides the full window, and its corner button (or Escape, or closing it) brings the full window back. The page asks for it through the core like everything else (`POST /api/window`, which a core without a window refuses), and the mini window alone may ask the shell for one thing, to start moving itself (Tauri capability `capabilities/mini.json`). Checked by an API test and in the real app on a scratch library: Windows reported the full window hidden while the mini player showed and back after; its buttons drove the player; moving was allowed from the mini player and refused from the full window.

- **The command palette** (Ctrl+K, Cmd+K on a Mac): one box that does what the window can (play or pause, skip, like the song playing, shuffle, repeat, the mini player, Up next, lyrics, every Library tab and Settings section) and finds the library's songs, albums, artists and playlists as you type (the library is asked 120 ms after typing stops). Arrow keys move, Enter chooses, Escape closes and gives focus back to where it was. Checked in Edge on the built page served by the core on a scratch library, light and dark: "lib not" found Library › Not found and Enter went there; a song's name listed the songs, ArrowDown moved, a click played it; Escape closed and focus went back. No errors in the page.
- **Autoplay** (Settings › Sound, on unless switched off, as on the phone): when the last song of the queue starts and nothing repeats (or when the songs after the one playing could not be played and were passed over), ten songs like the last five are added after it, so the music goes on with no gap: about half by the same artists, then liked songs, then the most played, then any; none heard in the last three hours or already queued, only files when no add-on can play; the same artist never twice in a row when another can come between, nor right after the song heard last (the artist with the most songs goes between the others). Checked by `crates/stashd/tests/api.rs` (a one-song queue grows by the other songs of the library; switched off, it ends; a queue whose last song no add-on has goes on) and `crates/stash-core/tests/lists.rs` (five songs by one artist and five by others alternate) and in the real app (one song became a queue of eleven, five by the same artist, interleaved). Up next lists them under the queue's own "Playing from"; they are not marked as autoplay's.

## 14. Songs saved to this computer

- **A song of the library can be saved as a file**, from its menu, or all the liked songs or a playlist's at once. One song at a time, the core finds it through the add-ons the way it does for playing (the add-on that has it already first, then each add-on in order), and asks for a file to keep: the contract's `download` route, or the stream of an add-on made for Eclipse, unless its manifest says `allowDownloads` is off. Only one progressive file is asked for.
- **The file** comes in under a temporary name, is checked to be audio by its first bytes (a web page or an error is not kept), and goes to `Artist/Album/NN Title.ext` in the downloads folder (Stash in the system's music folder unless another is chosen), never over another file. Its tags get the title, artist, album, track number and ISRC the library knows the song by, and the add-on's cover goes inside it.
- **The song's row becomes that file**: its likes, playlists and plays stay, the window shows the file's format, it plays with no connection, and the cover shown is the file's own. Taking it off the list deletes the file and the folders it leaves empty, and the song plays through the add-ons again.
- A song of the library whose file is not there when it comes up in the queue (given back after it was saved, or on a drive that is not plugged in) is asked for through the add-ons like any song known by its description.
- A song that cannot be saved stays on the list with the reason ("there is no add-on to save it from", the add-on's own words); Try again puts those back. A song coming in when the program ends is started again next time.

### Checked

- **By the tests:** `crates/stashd/tests/api.rs` against the demo add-on in both dialects (its `/download` route, and an Eclipse-style add-on's stream): saved into the chosen folder, tags written, asked again (nothing to do), given back (file and empty folders gone, the song back to playing through add-ons), and failing with a reason when no add-on can save it; `crates/stash-core/src/downloads.rs` (safe names on every system, never over another file, audio told from a web page by its first bytes); `crates/stash-core/src/eclipse.rs` (the `allowDownloads` rule).
- **In the real app on a scratch library** with the demo add-on and a scratch downloads folder: a search result saved from its menu came in as `Artist/Album/Title.wav`, showed in Library › Downloads as "WAV · 4.2 MB", played from the file (no add-on asked), and "Remove from this computer" deleted it.

### Not covered yet

- No real add-on has been asked for a download (the owner's has a small daily allowance; a download counts like a play).
- Big lists show 300 songs at a time; there is no per-song progress for waiting songs, only for the one coming in.
- A DASH or HLS add-on is not asked for downloads (only one progressive file).

## 15. Songs no add-on could find

- **A song your add-ons don't have is listed, to be fixed by hand.** When a song known only by its description is played or saved, and every add-on that can be asked for songs says it doesn't have it, the song goes on Library › Not found (the tab shows only while songs wait there) and is drawn dimmed wherever it is listed, with "Not found on your add-ons" as its tooltip. Nothing checks the whole library: songs come there only from playing or saving them, so no add-on's allowance is spent looking.
- **An add-on's "no" is believed only while it finds other songs** (one in the last half hour). An add-on whose allowance is spent, or of the wrong kind, may answer "nothing" to everything; trouble (not reachable, an error) never puts a song there. When an add-on found a song but could not give it, the song isn't listed either.
- **Fixing one:** opening a song on the list (or "Find on your add-ons…" in a dimmed song's menu, or "Find it" on the message that a song could not be played, when that song is now listed; the player says which song it passed over, `error_id`) makes one search, now that you asked, with the words the library knows it by; the words can be changed. Picking a result keeps it as that add-on's name for the song: it plays from there from then on, the song keeps its own title and artist, and a save of it that failed goes again. "None of these" takes it off the list; it stays in the library, dimmed. A song an add-on finds again leaves the list by itself.

### Checked

- **By the tests:** `crates/stash-core/tests/lists.rs` (listed, found again, fixed with its failed save put back, set aside, autoplay leaves such songs out) and `crates/stashd/tests/api.rs` against the demo add-on: an add-on that has found nothing yet isn't believed; once it has, two songs it lacks are listed (one there under other words, one nowhere); the candidates are searched as the library knows the song or in other words; the pick saves from the kept name; "None of these" leaves it marked.
- **In the real app on a scratch library** with the demo add-on: one of four liked songs saved, the other three failed and were listed and dimmed in Liked; the dimmed song's menu had "Find on your add-ons…", which opened the list on it; searched as the library knows it ("Devil Three Blue") nothing came up, searched as "Blue" the add-on's song was first, and picking it saved it (the message: Fixed. “Blue” plays from Demo folder now, and is being saved.); "None of these" left one song on the list. Playing a song the add-on lacks (volume at 0; no sound is made for it) brought the message with "Find it", which opened the list on that song. No errors in the window.

### Not covered yet

- An add-on that can only resolve (no search) can't show results to pick from.
- No real add-on has been asked: the owner's has a small daily allowance.

## 16. Songs kept as they were played

- **A song played from an add-on is kept as it plays.** The engine reads an add-on's plain stream (not DASH or HLS, not encrypted) of a library song through the core (`/relay/{token}` on the core's own address, a link that is hard to guess and answered for twelve hours), and the core writes what passes to a file in `Played` beside the library. Nothing is fetched twice: a copy is written only from the engine's own reading, from the stream's start to its end; a stream the engine seeks in before it has all of it is relayed and not kept, and a stream that is not audio by its first bytes is not kept.
- **The next play comes from the copy**: no connection is needed and no add-on is asked (its allowance is the listener's). Saving the song as a file takes the copy too, and asks no add-on; once saved, the copy goes.
- **Room**: Settings › Your library › Keep songs you play: off, or 1, 2 (unless changed), 5 or 10 GB. When the room is full the songs played longest ago make way; a smaller room keeps the most recently played; Clear deletes them all. A copy cut short (the program ended, the engine stopped reading) is deleted, and on start what a run before left half written is too: only files named the way copies are named (`<song>.<kind>`, `<song>-<n>.part`) are ever deleted there.

### Checked

- **By the tests:** `crates/stash-core/tests/lists.rs` (kept within the room, the least recently played go first, a new copy replaces the old file, one larger than the room is not kept, a smaller room and none at all), `crates/stash-core/src/played.rs` (tidying deletes only copies' names, never a path out of the folder), `crates/stashd/tests/api.rs` with the engine on the silent output: a song played from the demo add-on (which answers ranges, as a real server does) is kept byte for byte, plays again with the add-on removed (no add-on named, no error), and Clear deletes the file.
- **In the real app on a scratch library** with the demo add-on (volume at 0): one play kept a 4.4 MB copy; with the add-on removed it played from the copy, "Lossless" still shown; Settings showed "1 song kept, 4 MB"; Clear emptied the folder. No errors in the window.

### Not covered yet

- No real add-on's stream has been relayed: a server that does not answer ranges, or one that sends the file slower than it plays, was not tried; neither was a long hi-res file larger than the engine's own buffer (the stream then stays open as long as the song plays, on a thread of its own).
- A song kept is not marked in the lists, and the copies are not counted as offline songs anywhere yet.

## 17. What each add-on was asked for

- **Settings › Add-ons says, for each add-on, what Stash asked of it today** ("Today: 12 songs played, 1 saved"), when it last gave a song, and its last trouble when that came after ("Trouble 3 min ago: …", in the add-on's own words: not reachable, its allowance is spent). An add-on's allowance is the listener's, so this is what tells them how much is left. Every stream and download an add-on gave is kept with its time for a week (schema 12 `addon_events`); "today" is counted from the window's own midnight (the core has no time zone; `GET /api/addons/use?since=`). A song played from its kept copy, or from a file, asks nothing and counts nothing. The window reads it again every 15 seconds while Settings is open.

### Checked

- **By the tests:** `crates/stashd/tests/api.rs`: a song saved from the demo add-on counts one download and no stream, and the add-on's last song is known; a song played from it counts one stream.
- **On the built page served by the core** (scratch library, demo add-on): the add-on's row said "Today: 1 song saved · last gave a song just now". No errors in the page.

### Not covered yet

- Searches are not counted (the matcher searches add-ons that cannot be asked for a song; whether a search spends an allowance is the add-on's to say). The last trouble and the last song given are kept in memory only.

## 18. Shuffle that spreads artists

- **Shuffle spreads each artist's songs across the queue** (`crates/stash-core/src/shuffle.rs`). A fair shuffle often puts two songs by one artist side by side, and that is heard as "not random". Each artist's songs are laid evenly across the queue from a start chosen by chance, each a tenth of a step off by chance, and the queue is read in that order (the dithering Spotify described in 2014). An artist's songs take turns between its albums the same way. The first song after the one playing is not by that song's artist when another artist can come first. A credit counts under the first artist it names, read the way the matcher reads credits ("Drake, Future" is Drake's). Turning shuffle off still puts the queue back as it was.

### Checked

- **By the tests** (`shuffle.rs`, 300 shuffles each): a library-like mix (10 artists, 1 to 8 songs each, 35 songs) had two songs by one artist side by side **30 times** in 300 shuffles; a fair shuffle of the same songs had them **1,237 times**. Four artists with six songs each: 4 times in 300. One artist's three albums of four songs: two songs from one album side by side 10 times in 300. Every song comes exactly once and what comes before the shuffled part stays. With nine songs by the playing song's artist and one by another, the other comes first every time.
- **The player's own test** (`tests/player.rs`, the real player): the queue shuffled and unshuffled while it plays, as before.

### Not covered yet

- An artist with more than half the songs cannot be kept apart everywhere; their songs are spread as evenly as the rest allows.
