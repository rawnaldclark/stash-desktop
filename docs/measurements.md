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
- **Autoplay** (Settings › Sound, on unless switched off, as on the phone): when the last song of the queue starts and nothing repeats (or when the songs after the one playing could not be played and were passed over), ten songs like the last five are added after it, so the music goes on with no gap: about half by the same artists, then liked songs, then the most played, then any; none heard in the last three hours or already queued, only files when no add-on can play; the same artist never twice in a row when another can come between, nor right after the song heard last (the artist with the most songs goes between the others). Checked by `crates/stashd/tests/api.rs` (a one-song queue grows by the other songs of the library; switched off, it ends; a queue whose last song no add-on has goes on) and `crates/stash-core/tests/lists.rs` (five songs by one artist and five by others alternate) and in the real app (one song became a queue of eleven, five by the same artist, interleaved). Up next lists them under the queue's own "Playing from"; they are not marked as autoplay's. (Since §36: with similar music from Last.fm, autoplay starts a station from the newest of the last songs Last.fm can be asked about, and Up next shows its line.)

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
- **An add-on's "no" is believed only while it finds other songs it is asked for** (one in the last half hour). Playing a name it gave before does not count: an add-on whose allowance is spent, or whose lookup is broken, may answer "nothing" to everything and still play the names it gave. A "no" is remembered for six hours (the add-on is not asked again meanwhile), together with whether it was believed when it was said: one said before the add-on was finding songs never counts later. Adding an add-on again, or switching it back on, forgets what it was found not to have. Trouble (not reachable, an error) never puts a song there. When an add-on found a song but could not give it, the song isn't listed either.
- **Fixing one:** opening a song on the list (or "Find on your add-ons…" in a dimmed song's menu, or "Find it" on the message that a song could not be played, when that song is now listed; the player says which song it passed over, `error_id`) makes one search, now that you asked, with the words the library knows it by; the words can be changed. Picking a result keeps it as that add-on's name for the song: it plays from there from then on, the song keeps its own title and artist, and a save of it that failed goes again (when that add-on lets its songs be saved). "Find it" puts a song that was set aside back on the list before opening it. "None of these" takes it off the list; it stays in the library, dimmed. A song an add-on finds again leaves the list by itself.

### Checked

- **By the tests:** `crates/stash-core/tests/lists.rs` (listed, found again, fixed with its failed save put back, set aside, autoplay leaves such songs out) and `crates/stashd/tests/api.rs` against the demo add-on: an add-on that has found nothing yet isn't believed, and its "no" from then is not counted later; once it has found a song, one it lacks is listed, and after it is switched off and on again, the other one too (one there under other words, one nowhere); the candidates are searched as the library knows the song or in other words; the pick saves from the kept name; "None of these" leaves it marked.
- **In the real app on a scratch library** with the demo add-on: one of four liked songs saved, the other three failed and were listed and dimmed in Liked; the dimmed song's menu had "Find on your add-ons…", which opened the list on it; searched as the library knows it ("Devil Three Blue") nothing came up, searched as "Blue" the add-on's song was first, and picking it saved it (the message: Fixed. “Blue” plays from Demo folder now, and is being saved.); "None of these" left one song on the list. Playing a song the add-on lacks (volume at 0; no sound is made for it) brought the message with "Find it", which opened the list on that song. No errors in the window.

### Not covered yet

- An add-on that can only resolve (no search) can't show results to pick from.
- No real add-on has been asked: the owner's has a small daily allowance.

## 16. Songs kept as they were played

- **A song played from an add-on is kept as it plays.** The engine reads an add-on's plain stream (not DASH or HLS, not encrypted) of a library song through the core (`/relay/{token}` on the core's own address, a link that is hard to guess and answered for twelve hours), and the core writes what passes to a file in `Played` beside the library. Nothing is fetched twice: a copy is written only from the engine's own reading, from the stream's start to its end; a stream the engine seeks in before it has all of it is relayed and not kept, and a stream that is not audio by its first bytes is not kept.
- **The next play comes from the copy**: no connection is needed and no add-on is asked (its allowance is the listener's). A copy is checked against the size it was kept at before it is played; one the engine cannot play is let go before the player asks for the song again, so it comes from the add-on. Saving the song as a file takes the copy too, and asks no add-on, when the add-on it came from lets its songs be saved, and its download would be no better (an Eclipse add-on's download is its stream; another's, unless the copy is lossless already); once saved, the copy goes.
- **A stream that breaks off is picked up again** where it stopped (up to three times, by asking for the rest), so the song does not end early and its copy is still whole. A stream whose length is known is sent to the engine with that length, so an end that comes too soon is never taken for the song's end. Streams are asked for uncompressed; a server that compresses anyway is relayed and not kept. A playlist of pieces (HLS, DASH) the add-on did not say it was is handed to the engine at its own address. Only the engine is answered: a request carrying a web page's headers (Origin, Referer, Sec-Fetch) is refused.
- **Room**: Settings › Your library › Keep songs you play: off, or 1, 2 (unless changed), 5 or 10 GB. When the room is full the songs played longest ago make way; a smaller room keeps the most recently played; Clear deletes them all. A copy cut short (the program ended, the engine stopped reading) is deleted, and on start what a run before left half written is too: only files named the way copies are named (`<song>.<kind>`, `<song>-<n>.part`) are ever deleted there, and nothing is when the library cannot say which copies it keeps. A copy is written through to the disk before it is named, so a power cut never leaves one kept that is not all there. A copy that was coming in when Clear was pressed is not kept; a file the engine had open when it was to be deleted is deleted later.

### Checked

- **By the tests:** `crates/stash-core/tests/lists.rs` (kept within the room, the least recently played go first, a new copy replaces the old file, one larger than the room is not kept, a smaller room and none at all), `crates/stash-core/src/played.rs` (tidying deletes only copies' names, never a path out of the folder), `crates/stashd/tests/api.rs` with the engine on the silent output: a song played from the demo add-on (which answers ranges, as a real server does) is kept byte for byte, plays again with the add-on removed (no add-on named, no error), and Clear deletes the file. `crates/stashd/src/relay.rs`: a stream that breaks off at 40,000 of 100,000 bytes is picked up and comes whole; one that cannot be picked up, or ends quietly before its length, is an error and never an end; ranges are read whole or not at all (no overflow).
- **In the real app on a scratch library** with the demo add-on (volume at 0): one play kept a 4.4 MB copy; with the add-on removed it played from the copy, "Lossless" still shown; Settings showed "1 song kept, 4 MB"; Clear emptied the folder. No errors in the window.

### Not covered yet

- No real add-on's stream has been relayed: a server that does not answer ranges, or one that sends the file slower than it plays, was not tried; neither was a long hi-res file larger than the engine's own buffer (the stream then stays open as long as the song plays, on a thread of its own).
- A song kept is not marked in the lists while online (offline, the songs that are not here are dimmed: section 22).
- A stream whose length is not told cannot be picked up again, and its early end looks like the song's end to the engine. A stream that stops sending without closing keeps its thread until the server gives up.
- Songs the engine has to seek in before it plays them (an MP4 with its index at the end) are never kept, and are fetched again each time.

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

- **By the tests** (`shuffle.rs`, 300 shuffles each): a library-like mix (10 artists, 1 to 8 songs each, 32 songs) had two songs by one artist side by side **30 times** in 300 shuffles; a fair shuffle of the same songs had them **1,237 times**. Four artists with six songs each: 4 times in 300. One artist's three albums of four songs: two songs from one album side by side 10 times in 300. Every song comes exactly once and what comes before the shuffled part stays. With nine songs by the playing song's artist and one by another, the other comes first every time.
- **The player's own test** (`tests/player.rs`, the real player): the queue shuffled and unshuffled while it plays, as before.

### Not covered yet

- An artist with more than half the songs cannot be kept apart everywhere; their songs are spread as evenly as the rest allows.

## 19. Smart playlists

- **A playlist can fill itself by rules** (`crates/stash-core/src/smart.rs`; Library › Playlists › New smart playlist, or "New smart playlist" in Ctrl+K). A rule compares one thing the library knows of a song: its title, artist or album (contains, does not contain, is, is not, starts with; without case or accents, so "beyonce" finds "Beyoncé"), whether it is liked, its plays (ever, or at least N times in the last D days), when it was last played or added (in the last N days, or not), its year, its length, its quality from the file (lossy, lossless, hi-res; an add-on's song has none until it is saved), and whether it is on this computer (saved songs too). Every rule, or any one; an order (newest added, most played, played most recently, not played for the longest, title, artist, album, newest year, or shuffled with a new pick each day); and at most N songs. A new one starts from one of seven common lists (Recently added, Most played, On repeat, Forgotten favourites, Never played, Lossless, Daily mix). The editor counts the songs the rules pick as they are written, and says in words what does not make sense yet.
- **It is read anew whenever it is opened, and after each play that counts**, so it follows the library. Its songs are its rules': they are not added, removed or moved by hand, and it is left out of "Add to playlist". Its page says its rules in a line, with Edit rules.
- **It travels as its songs, with its rules**: in the library file, a smart playlist goes out as the songs it holds then and its rules. The phone and the web player keep the songs (they read only the fields they know); another desktop Stash makes it smart again, and brings in its songs for the rules to find. One that comes back without rules (a phone's copy) stays smart here. Every value typed into a rule reaches SQLite bound, never as SQL.

### Checked

- **By the tests:** `crates/stash-core/tests/smart.rs` runs every rule against a library of five songs (text without case or accents, an album that is missing, years and lengths that are missing, plays ever and lately, never played, likes, quality from the file, an add-on's song), every rule or any one, orders and limits, pages that end at the limit, a shuffle that holds all day and changes on other days, hand edits refused, new rules, and the library file out and back in (into an empty library, into the same one, and back from a phone without rules). `smart.rs` checks rules that make no sense (said in words). `crates/stashd/tests/api.rs`: made, listed with its rules and count, its songs read, hand edits refused (409), the preview's count and first songs, rules that make no sense refused (400), new rules, and none taken by a plain playlist.
- **On the built page served by the core** (a scratch library, light and dark): a new one opened on Recently added (645 songs); Daily mix gave 30; a second rule with no words said "Not yet: a rule is missing its words." and kept Create off; "lanterns" gave 10 with every rule and 35 with any; Create opened its page (Smart playlist, its rules in a line, no handles to reorder); the sidebar said "Smart playlist · 35 songs"; Edit rules down to one rule gave 15, kept on Save; a song's Add to playlist did not offer it; Ctrl+K found "New smart playlist". No errors in the page.

### Not covered yet

- Playlist folders, and rules over genre or rating (the library keeps neither yet).

## 20. The core answers only the window

- **The core's API is for the window.** It listens on 127.0.0.1, answers only when it is asked for as 127.0.0.1 or localhost on its port (a name that leads there through DNS is refused), and lets only the window's own origins read its answers (the shell's `tauri.localhost`, or the page the core serves itself).
- **A page elsewhere is not answered at all.** A browser says where a request comes from (Sec-Fetch-Site). A request from another site must name the window: by its Origin, or, for a picture, by the window's page as its Referer. Otherwise the core answers 403. Before, a page anywhere could put the core's addresses in a picture or a no-cors request. It could not read the answer, but asking was enough: a search, or a song's candidates in Not found, makes the core ask an add-on, and that spends the add-on's daily allowance. The window now asks for the core's own covers by CORS, so they carry its Origin. Programs that are not browsers (the engine, scripts) send none of these headers and are answered as before; the relay answers only the engine (section 16).
- **A page at tauri.localhost needs the window's key.** Browsers send every `*.localhost` name to this computer. A page that another program serves on port 80 or 443, opened as `tauri.localhost`, would therefore carry the window's own origin.
  - The shell now gives the window a new key on every run: 128 random bits, set in the page's starting script next to the core's address.
  - From the window's origins, the core answers only requests that carry this key, whether the request names the origin or a page there as its Referer.
  - Programs that are not browsers, and the page the core serves itself, need no key.
- **What the browser does first:** current Edge already stops a public site's requests to this computer (Local Network Access). Pages served from this computer, and browsers without that protection, reach the core, and it now refuses them.

### Checked

- **By the tests:** `crates/stashd/tests/api.rs` (`a_web_page_cannot_use_the_core_and_the_window_can`): from another site, a request with no Origin, with another page as its Referer, or with another Origin is refused; with the window's Origin, or with the window's page as its Referer (`http://tauri.localhost/…`, `tauri://localhost/`), it is answered. So are same-origin requests, typed addresses and programs.
- **In the real shell**, on a new scratch library of two files with their own covers: the window (on `http://tauri.localhost`) showed both covers, asked for with its Origin (200). The same picture asked for the way a page's own picture is (no CORS, no Referer) got 403. No errors in the page.
- **In Edge**, a page served by another server on this computer (`127.0.0.1:47999`, then `localhost:47999`) asked for a cover, a search and the song counts. Each reached the core (Sec-Fetch-Site same-site or cross-site, no Origin, that page as its Referer) and got 403. A page on a public name never reached the core (Edge's Local Network Access).

- **After the review, by the tests:** `a_page_at_the_windows_address_needs_the_windows_key`.
  - With the window's origin, a request with no key or a guessed key was refused, and so was a picture whose Referer was the window's page.
  - With the key, each was answered, for both of the window's origins.
  - The core's own page and a program needed no key.
- **After the review, in the real shell:**
  - Both covers carried the key and loaded (200).
  - From the window, a request with no key was refused (403), so was one with a guessed key (403), and one with its own key was answered (200).
  - The mini player read the core and its cover with the key, with no errors.

### Not covered yet

- A browser that sends no Sec-Fetch-Site (Chrome before 76, Firefox before 90, Safari before 16.4) is not told apart from a program, so a page in one could still make the core ask an add-on.

## 21. Scrobbling

- **What you play goes to ListenBrainz, or to a server that speaks its language** (Settings › Scrobbling; `crates/stash-core/src/scrobble.rs`, `listenbrainz.rs`, `crates/stashd/src/scrobbling.rs`). The listener pastes their token; the core checks it with the service before keeping it, and keeps the name the service knows them by. Another server is given by its address (Maloja, Koito and others people run themselves); an address pasted with one of the API's own paths on it is cut back to its root. More than one service can be on at once.
- **A play is sent when it counts**, by the rule the library counts plays by (half the song, never before 30 s, always by 4 minutes), with the time it began. Its title, artist and album go, and its length, ISRC and track number when the library knows them. The song that begins playing is told as playing now. A file whose artist is the "Unknown artist" placeholder is not sent.
- **Plays wait in the library until the service takes them** (schema 14), so what is played with no connection goes when there is one. At most 50,000 wait for each service; past that the oldest make way. A service that is down or busy is left 30 s, then twice as long each time up to 30 minutes, or as long as it asks; Try again asks it at once. One that refuses the token is asked nothing more until the token is given again, and plays still wait for it. A play it says is wrong is let go on its own: the others sent with it go one by one, and the listener is told how many were let go. While a service is off, plays are not kept for it.
- **The token** is kept in the library, as an add-on's link is, and the window is never given it. A redirect is not followed, so the token goes only where the listener said.
- **Plain http is only for a server nearby.** Every request carries the token, and over plain http anyone on the way can read it.
  - ListenBrainz itself is always asked over https, even when its address is given as http.
  - Another server needs its https address, unless it is on this computer, on the home network (a private address, or a name such as `nas`, `nas.local` or `maloja.lan`), or reached through Tailscale.
- **A play counts as taken only when the service says so.** The answer must be a JSON answer whose status, if it gives one, is "ok". A hotel's sign-in page also answers 200, and the plays it got are kept for later.
- **A play is let go as wrong only when the service took others sent with it.** When a batch is called wrong, its plays are sent one at a time, and those called wrong are let go. If the service takes none of them (five in a row are called wrong), it is taken to be in trouble: the plays wait, the trouble is shown, and the service rests. Before, a server that called everything wrong could empty the whole list.
- **A token refused is the one that was sent.** If the listener gives a new token while the old one is being refused, the new one is not marked refused.
- **Offline, no token is checked** (adding a service asks it), and a backlog being sent stops between batches.
- **If the library cannot be written** (a full disk), the service rests, so the same plays are not sent again and again.

### Checked

- **By the tests:** `listenbrainz.rs` (addresses cut to their root; plain http only to nearby servers, and ListenBrainz moved to https; songs with nothing empty in them; answers read as what to do next, a 200 that is not the service's being no yes), `scrobble.rs` (a play waits for each service that is on, once, oldest first; taken, let go, troubled, refused and given again; removed with what waited; past 50,000 the oldest make way), and `crates/stashd/tests/api.rs` (`plays_go_to_listenbrainz_and_wait_while_it_is_away` (also: a service that calls every play wrong keeps them waiting, shows its trouble, and takes them once it is asked again)), against a stand-in for ListenBrainz that checks what is sent as the service does. A wrong token, no token, an address that is not one and an address with nothing there were refused in words; the token was never given back. An untagged file's play counted and was not sent. An add-on's song was told as playing now, then sent as a single play with the time it began. With the service away, the next play waited with "it answered 503" and went when the service was asked again. A refused token held the next play back until it was given again. Off, a play was not kept for it; then it was removed.
- **On the built page served by the core**, light and dark, against a stand-in on this computer: a wrong token said "That server did not take that token."; connecting showed "As listener · Nothing sent yet"; three plays waiting while the service was away showed with "Trouble just now: it answered 503." and went as one import on Try again; a refused token was said, and the play that waited went once the token was given again. Ctrl+K finds Settings › Scrobbling. No errors in the page.

### Not covered yet

- Scrobbling to Last.fm. It needs an application key and secret, and those do not belong in a public repository. (Radio and autoplay need no Last.fm key in the program: they ask Stash's proxy; see §36.)
- The real ListenBrainz: the stand-in follows its documented checks, but no real token was used.

## 22. Offline

- **Offline, Stash plays only what is on this computer.** It is turned on in Settings › Your library › Offline or with Ctrl+K, and turned off there or with the header's Offline chip; the core keeps it from one run to the next. "On this computer" means a file that is here (one of the listener's own, or a song saved), or a song kept as it was played (section 16). Each song row says which it is.
- **Nothing streams and no add-on is asked.** A queue holds only songs that are here: playing a list from a song that is not here starts at the next one that is, and a list with none is refused in words. A queue made online and played offline passes over what is not here. In the window, songs that are not here are dimmed; clicking one says why and plays nothing, while liking it or adding it to a playlist still works. Search looks in the library only. Lyrics show what was found before, however old, and nothing new is looked up. Saving songs waits, autoplay does not ask, and scrobbles wait; all of it goes on once Stash is back online.
- **The song playing when Stash goes offline plays to its end.** A seek in it may still reach the add-on. Anything the engine was already handed to follow it is taken back and looked up again, and offline that means passed over. Before, a song from an add-on that was about to start would still stream.
- **A song being saved stops and waits**, at the front of the list as it was.
- **Up next dims what will be passed over.** Each row of the queue says whether it plays with no connection. Offline, the others are dimmed in Up next, and clicking one says why instead of jumping to it.
- **Dimmed, not hidden.** The phone's Download mode hides what is not downloaded. Here the lists keep every song, so a playlist reads the same online and off, and the songs that will play are the bright ones.

### Checked

- **By the tests:** `crates/stashd/tests/api.rs` (`offline_plays_only_what_is_here_and_asks_no_add_on`), on a playlist of an add-on's song, a file, an add-on's song with a copy kept, and another file. Its rows said here: no, yes, yes, yes. Offline (and kept in the library), songs none of which is here were refused in words. Played from the top, the queue held the file, the kept song and the other file, and the kept song played from its copy (lossless, no add-on named, and nothing asked of the add-on that day). An add-on search, adding an add-on and a song's candidates were refused. A song that is not here was not added to the queue; one that is, was. Back online, the add-on's song played from the add-on.
- **On the built page served by the core**, on a scratch library of 600 files and 53 songs known only by their description: Settings › Your library › Offline turned it on, and the header showed "Offline". Library › Songs dimmed 5 of the first 28 rows. Clicking one said "Not on this computer, and Stash is offline" and nothing played; a row that is here played. Ctrl+K offered "Go back online"; Search said "Stash is offline: songs, albums and artists from your library."; the header's chip went back online. No errors in the page.

- **By the tests, after the review:** `offline_takes_back_the_song_handed_ahead`, on a queue of a 60-second file, a song from an add-on, and another file:
  - Near the first file's end, the add-on's song was asked for and handed to the engine.
  - Stash then went offline, and the first file played on.
  - When it ended, the add-on's song was passed over and the other file played.
  - The add-on was not asked again.

  `offline_plays_only_what_is_here_and_asks_no_add_on` now also shows that a scrobbling token is not checked while offline. Offline, the queue's rows said here: yes, no, yes.
- **Up next on the built page served by the core.** A queue was made online of three files and two songs known only by their description.
  - Offline, Up next dimmed exactly the two described songs.
  - Clicking one said "Not on this computer, and Stash is offline", and the song playing stayed.
  - No errors in the page.

### Not covered yet

- Stopping a song while it is being saved was reasoned through, not tried: it takes a slow download from an add-on.
- The mini player shows only the song playing, so it has nothing to dim.
- Covers from the web are still asked for (with no connection they just do not load).

## 23. Shortcuts that work in any app

- **Settings › Shortcuts** has one switch, off until the listener turns it on, and keys for seven actions:
  - play or pause;
  - next song and previous song;
  - volume up and volume down, 5 at a time;
  - like the song playing;
  - show Stash.

  The first five come with keys: Ctrl+Alt with Home, Page Down, Page Up, Up and Down. None of them is Ctrl+Alt with a letter, which many keyboards turn into a character (AltGr). Like and show have no keys until the listener picks some.
- **Picking keys:** click a shortcut's keys, then press the new ones. Esc leaves them as they were; Backspace leaves that action with no keys. While keys are being picked, Stash lets go of all of them, since a key held across the system never reaches the window. Some keys are refused in words:
  - a key alone, or with only Shift, would stop working in every other app (F13 to F24 are allowed, as nothing types with them);
  - keys another shortcut has.

  When another program holds a shortcut's keys, its row says so.
- **The like shortcut only likes.** Nothing shows which way a press went, so a second press does not take the like back. A song from an add-on is made a song of the library first, as a like in the window does it.
- **Media keys work without these** (section 13).
- **Likes made elsewhere now reach every window.** The core counts changes to the likes and the playlists, and the player's answers carry that count. A window that sees the count move past its own reads its lists again. Before, a like in the mini player did not show in the full window until the window was opened again.
- **A test run leaves the listener's window alone.** A run on a library named by `STASH_DB` no longer saves the window's place and size. That place is kept in the system's own folder, which the test could not move elsewhere.

### Checked

- **By the tests** in `crates/stashd/tests/api.rs`:
  - `shortcuts_are_held_by_the_shell_and_do_what_they_say`, with a stand-in for the shell:
    - Off, the shell was asked to hold nothing.
    - On, it was asked to hold the five keys, and the one another app had was reported back.
    - Keys picked as "alt+ctrl+End" were kept as Ctrl+Alt+End. Keys another shortcut had (409, naming it), an S alone (400) and an action there is not (404) were refused.
    - While keys were picked, nothing was held; then all six were held again.
    - The choice was kept in the library.
    - The actions:
      - play/pause paused and played;
      - next and previous moved between songs;
      - volume down twice made 90, and up three times stopped at 100;
      - like liked a file, and then a song from an add-on, made a song of the library first;
      - show asked for the full window;
      - an unknown action was refused.
    - Each like raised the count of changes, and a like made in a window said the new count in its answer.
  - `without_a_shell_there_are_no_shortcuts_to_hold`.
  - A unit test of how keys are named and which are refused.
- **In the real app on a scratch library.** Windows was asked whether each key was held: another program asking for keys Stash holds is refused. Another program held Ctrl+Alt+Page Up before Stash started.
  - Off, Ctrl+Alt+Home was free.
  - Switched on, Ctrl+Alt+Home, Page Down and Down were held by Stash, and Previous song said "Another app uses those keys. Pick other keys."
  - While new keys were picked for the like, Ctrl+Alt+Home was free. Ctrl+Alt+End, typed into the page, was then held by Stash, and Ctrl+Alt+Home was held again.
  - For Show Stash, an S alone and Ctrl+Alt+Home were each refused in words, and Esc left it with no keys.
  - Switched off, every key was free and the choice was kept. After Stash ended, every key was free.
  - No errors in the page.
- **On the built page served by the core, as two pages** (the full window and the mini player):
  - A like in the mini player filled the full window's heart 0.9 s later, and Liked songs went from 30 to 31.
  - The like shortcut's action filled both hearts 0.85 s later.
  - An unlike in the full window emptied the mini player's heart 2.1 s later. That page was in the background, where timers run slower.

### Not covered yet

- No key was pressed on this computer, since a press would reach other programs. A key pressed reaches the core through the same call the tests make. The one step not seen is the plugin handing a press to the shell.
- macOS and Linux were not tried. On Linux the keys need X11; under Wayland they are not held.

## 24. Lists kept on this computer

- **A playlist, or the liked songs, can be kept on this computer.** The download button on a playlist's page, and on Library › Liked, is now a switch. While it is on:
  - the list's songs are saved as files;
  - so are songs that come onto the list later: added to it, liked, or picked by a smart playlist's rules.

  The kept lists are looked at whenever the likes or the playlists change, and every 30 seconds while a smart playlist is kept. Next to the switch, the list says how far it is: "Saving: 3 of 40 on this computer", "On this computer", and how many could not be saved.
- **A song that could not be saved, or that the listener took off this computer, is not tried again on its own.** Each try may spend an add-on's allowance. The listener can ask for it again (Library › Downloads, or the song's menu).
- **Turning the switch off saves no more of the list.** Its songs still waiting are taken off the list, unless another kept list holds them; the one coming in finishes, since what it cost is spent. Songs already saved stay; Library › Downloads takes them off. Offline, nothing is fetched and the list waits.
- **A kept list is marked** with a small arrow in the sidebar and on its card (Library and Home).
- **A song that already plays from a copy kept from playing it** is saved from that copy, and no add-on is asked (as before for any save).
- **Schema 15** adds `playlists.download`. The liked songs' choice is the setting `download.liked`. Schema 16 adds `download_declined` and `downloads.by_list` (see the review fixes below).

### Checked

- **By the tests:** `a_list_kept_on_this_computer_saves_its_songs_and_those_that_come_later`, with an add-on on this computer:
  - A playlist that was not kept had nothing saved.
  - Kept, its song was saved, and the playlist's row said so. A song added to it later was saved too.
  - Liked songs, kept: the liked song was saved, and so was a song liked later.
  - A song the add-on had lost could not be saved. It was said so, and was not put back on the list when the playlist was kept again.
  - Turned off, a song added later was not saved, and the songs saved stayed.
  - A switch with no true or false, and a playlist that does not exist, were refused.
- **By the tests:** `letting_a_list_go_takes_its_waiting_songs_off_the_list`, with every add-on answer held back 0.7 s. A playlist of four songs was kept, and its last song was liked with the liked songs kept too. Turned off while the first song came in, the answer showed two waiting (the one coming in and the liked one). The first and the liked song were saved; the other two were taken off the list and did not come back when the kept lists were looked at again.
- **On the built page served by the core, light and dark.** This used a new scratch library and an add-on on this computer serving four made-up songs, with each answer held back 0.7 s.
  - The playlist's switch said "Saving: 0 of 3 on this computer", then "On this computer".
  - A song added to the playlist showed "Saving: 3 of 4 on this computer", then "On this computer".
  - Liked songs, kept, showed "On this computer".
  - The sidebar showed the mark on Liked songs and Trip, and the Library's cards on Trip.
  - Turned off, the playlist said "Trip is no longer kept on this computer. Songs saved stay (Library › Downloads)", its mark left the sidebar, and the four files saved were still there.
  - No errors in the page.

### Review fixes

A review of the work above found these, all fixed:

- **A song the listener took off came back.** "Remove from this computer" or "Don't save" on a kept list's song deleted its row, and the next look at the lists queued it again, spending the allowance each time. A song taken off is now remembered (`download_declined`, schema 16). Kept lists leave it be, and the list's line says "2 taken off by you". Asking for it by hand clears that.
- **Trouble with an add-on failed every waiting song.** An allowance used up, a rate limit, an add-on that could not be reached, or a file server's 429 or 5xx each marked the song "not saved", and saving went straight on to ask for the next one. Now none of these is the song's fault. The song waits its turn again, first in line, and saving waits 1 minute, then twice as long each time it is held again, up to an hour. Library › Downloads says "Saving waits: your allowance with the add-on is used up. It tries again at 14:05, or now with Try again". A song saved, or Try again, ends the wait.
- **Songs that left a kept list were still saved.** This happened when a kept playlist was deleted, a song was taken off it, a song was unliked, or a smart playlist's rules changed. A song a list put on the downloads now says so (`downloads.by_list`, schema 16). After any change to the likes or the playlists, the lists are looked at before the next song is fetched, and waiting songs that no kept list holds are taken off. Songs asked for by hand stay whatever the lists hold.
- **The look at the lists cost one statement per song.** With 50,000 liked songs that was 50,000 statements under the library's lock, every 30 seconds. It is now a few set-wise statements, and the 30-second look runs only while a smart playlist is kept: other lists change only through an edit, which looks at once. How far a list is comes from one query, however long the list. Whether the liked songs are kept rides with the library's counts.
- **Songs saved since the window read its lists showed as not on this computer when offline.** Going offline now has every window read its lists again. Up next takes the queue's word on each song over the window's older copy.
- **Each flip of the switch had the window read every like again.** The answer now carries the core's count of changes, so the window that flipped it updates its own marks.

Checked:

- **By the tests:**
  - In the first test above, a song saved for the kept playlist was taken off this computer. It did not come back after another change, and the list said `declined: 1`. Asked for by hand, it was saved again.
  - In the second, the playlist was kept again and deleted while its songs waited. The one coming in finished and the other was taken off: three songs saved in all, not four.
  - `an_add_on_whose_allowance_is_spent_holds_saving_and_fails_nothing`: the demo add-on refused every download as `quota_exceeded`. Saving said it waited because the allowance is used up. Half a second later all three songs were still waiting, none "not saved". With the allowance back and Try again pressed, all three were saved and the wait was gone.
- **On the built page:** the check above was run again with a fifth song, liked, which was saved only after the page had read the likes. Turned offline from Settings, without a reload, the Liked list dimmed 0 of its 2 songs.

### Not covered yet

- A smart playlist whose songs change by themselves (random order, or rules by date) saves the new songs each time it changes. Keeping the daily mix saves up to its length in new songs a day, until every song it can pick is saved. Songs that leave it stay saved.

## 25. On this computer, in Library › Songs

- **A chip on Library › Songs, "On this computer", lists only the songs that play with no connection:** files that are here (the listener's own or saved) and songs with a copy kept from playing them. The count beside it says "3 of 6 songs". Searching the list searches those songs only.
- **It follows offline.** It turns on when Stash goes offline and off when it comes back, as the phone's Download mode hides what is not downloaded. The chip changes it in between.
- **Playing from the filtered list** queues the songs on this computer from the one clicked, as the whole list does for the library.
- While the filtered list is shown, how many songs are here is read again every 5 seconds, so songs being saved arrive in it.
- The core: `GET /api/tracks?here=1` pages these songs in title order, `GET /api/tracks/here` counts them, and `POST /api/player/play {offset, here: true}` plays them.

### Checked

- **By the tests:** in `a_list_kept_on_this_computer_saves_its_songs_and_those_that_come_later`, the four songs saved were counted, and listed in title order. A page from the middle was the third, and without the filter all six songs were listed.
- **On the built page served by the core:** a new scratch library with six made-up songs from an add-on on this computer. Every other one was saved by keeping a playlist, into a scratch downloads folder.
  - The list showed 6 songs. With the chip on, it showed 3 rows and "3 of 6 songs"; off again, 6.
  - Offline, after a reload, the chip was on by itself, with 3 rows.
  - With the volume at 0, playing the second row played that song, and the queue held the two songs here from it.
  - Back online, after a reload, the chip was off and all 6 songs were listed.
  - No errors in the page.

### Not covered yet

- Albums, Artists and the playlists' pages have no such filter: offline, they dim what is not here.

## 26. Songs and artists not played

- **"Don't play this song" and "Don't play {artist}"** are in every song's menu. Stash then never plays them on its own:
  - autoplay never picks them;
  - a list played through leaves them out, and so does a group of songs added to Up next.
- **A song chosen by itself still plays.** That covers one song played or added to Up next. A click on a dimmed row says "You chose not to play …" and offers "Play it", which plays that one song.
- **Choosing takes effect at once.**
  - The song or the artist's songs leave Up next, and the song playing is skipped if it is one of them.
  - Their rows dim everywhere.
  - The toast offers Undo, and the menu then offers "Play this song again" or "Play {artist} again".
- **A song is known by its words**, so every copy of it counts: a file, a saved song, an add-on's track. An artist is known by its name, whatever case or accents. A name with no letters or digits, like "!!!", is refused, since it would stand for every such name.
- **Settings › Not played** lists them, the latest first, each with "Play again". The command palette goes there.
- **The phone's blocklist comes over on import.** Songs the listener chose not to play on the phone are not played here either, and the import says how many came over.
- **Schema 17** adds `blocked(kind, key, title, artist, at_ms)`. A track row says `blocked: "song" | "artist" | null`. `GET /api/blocks` lists the blocks. `POST /api/blocks {kind, title, artist, blocked}` chooses or plays again; it answers with the library's rows it covers, so the window marks them at once.

### Checked

- **By the tests:**
  - `a_song_is_known_by_its_words_and_an_artist_by_its_name` (core):
    - A song block covered the same words with a "feat." credit and in other case, but not another artist's song of that name.
    - An artist block covered every song by that name, accents or not.
    - Autoplay picked none of them.
    - Playing the artist again left the song's own block in place.
    - "!!!" was refused.
  - The Android import test: the phone's blocked song came over as one block, and importing again added none.
  - `songs_and_artists_chosen_not_to_be_played_are_left_out`, with the real engine on the silent output and two made-up artists of three songs each:
    - A song block put one row's `blocked` to "song". The library played through queued the other five.
    - That song played when asked for by itself. Added to Up next with two others it was left out; added by itself it was queued.
    - An artist was blocked while its first song played. The song was skipped to the next by someone else, the artist's other songs left Up next, and their rows said "artist".
    - A list of only that artist's songs was refused with 409.
    - Playing the artist again left one block.
    - A wrong kind, a missing artist and "!!!" were refused.
- **On the built page served by the core:** a new scratch library of the same made-up files, scanned.
  - The menu showed both entries. "Don't play this song" said "Stash won't play “…” on its own" with Undo, and one row dimmed.
  - A click on that row said "You chose not to play “…”" with Play it, and nothing played.
  - "Don't play Shadow 1" dimmed four rows.
  - Playing from a free song of the other artist queued it and the next free one, leaving out the two dimmed songs between.
  - The menu of a Shadow 1 song then offered "Play Shadow 1 again".
  - Settings › Not played listed Shadow 1 ("Every song by this artist") and the song. Play again on the artist left one entry, and Songs then dimmed one row.
  - No errors in the page.

### Not covered yet

- An add-on's track found in a search is not marked until it is played or kept (its words are checked when it would be queued).
- Smart playlists still list blocked songs (dimmed); playing them leaves those out.
- The way back out (the library file the phone and web player read) does not carry the blocks yet.

## 27. Picking songs

- **Ctrl+click** (Cmd+click on a Mac) picks a song in a list, or lets it go.
- **Shift+click** picks every song from the last one Ctrl picked; Ctrl+Shift+click adds them to the songs picked already.
- **Ctrl+A** picks the whole list: the one with the focus, the one songs are picked in, or the page's first.
- **Ctrl+Space and Shift+Space** on a row's button do the same from the keyboard.
- **Escape lets them go**, and so does going to another page. A list whose songs change (a playlist edited, the liked songs, History, the filter's results) lets its picks go, since their places changed too.
- **A plain click still plays the song**, as before, and lets the picks go.
- **Every song list can pick:** Library › Songs, Liked, History and Downloads, playlists, albums, an artist's songs, and search's songs.
  - Library › Songs reads picked songs from the core page by page, so Shift+click and Ctrl+A run past the rows drawn on screen.
  - A pick is at most 5,000 songs.
- **While songs are picked**, a bar above the player says how many, with Play, Add to playlist, the menu for all of them, and Clear. Toasts rise above it.
- **The menu of a picked song acts on all of them:**
  - Play next, Add to Up next, Add to playlist;
  - Like all, Download all (or Remove from this computer on Library › Downloads);
  - "Don't play these songs", or "Play these songs again" when every one is chosen out already.
  - In the listener's own playlist, "Remove N from this playlist" takes them out together. It does so only while the playlist still has those songs at those places; otherwise it says the playlist changed.
- Menus opened from the bar open upward. A menu that changes size stays inside the window, and menus sit above toasts.

### Checked

On the built page served by the core: a new scratch library of 12 made-up files, scanned. Volume 0 before anything played.

- **Picking in Library › Songs:**
  - Ctrl+click on row 0 gave "1 selected".
  - Shift+click on row 3 picked rows 0–3.
  - Ctrl+click on row 2 let it go, leaving 0, 1 and 3.
  - Ctrl+Shift+click on row 6 added 2–6, giving 7.
  - Nothing played.
- **The menu of a picked row** was "Options for 7 songs": Play next, Add to Up next, Add to playlist, Like all 7, Download all 7, Don't play these 7 songs. There was no Go to artist or album.
  - Add to playlist › New playlist "Picked" made the playlist with those 7 songs in the list's order.
- **Escape** took the bar away.
- **Ctrl+A** picked all 12 (read from the core). The bar's Play queued 12.
- **A plain click on row 5** with 11 picked took the bar away and played row 5.
- **In the playlist:** two Ctrl+clicked rows offered "Remove 2 from this playlist". It left the other 5 in order, and said "Removed 2 songs from this playlist".
- **The bar's Add to playlist** opened on the playlists, above the bar.
- **Going to Settings** let the picks go.
- No errors in the page, light and dark.

## 28. Second review: saving, "Don't play", the filter

A reviewer read sections 24–26 and found these. All are fixed. (The rules for trouble that may be the song's own changed after a third review: see §30.)

### Saving

- **Trouble belongs to one song or it does not.**
  - An add-on's quota, rate limit, refused key, being down or out of reach, and no add-on to ask at all make saving wait.
  - The add-on's answer about the song itself does not: a file it cannot give as asked (encrypted, DASH, another codec), a request it refused, "not found". Those fail the song, and no allowance is spent asking again and again.
  - The file's own fetch waits for the line or the server: no connection, a timeout, a stall, a missing host, 429, 502–504. It fails for the link itself: a bad address, a redirect to http, too many redirects, a certificate.
- **The downloads folder not being writable makes saving wait**, not fail every song. That covers a full disk or an unplugged drive.
- **A song held goes to the back of the line**, so the songs after it are tried first. A song held again after other songs were saved in between is given up on at its second strike: the trouble was its own.
- **Saving waits as long as the add-on asked** (`retry_after`, at least 5 seconds, at most an hour). Otherwise it waits a minute, doubling up to an hour.
- **These end the wait at once:** Try again, an add-on added or switched on, or a new downloads folder.
- **A "no" from add-ons that are not finding other songs either** (a broken lookup, or a spent allowance that answers "nothing") is no longer final:
  - The song goes to the back of the line, and the next one is tried at once.
  - Two songs in a row with such a "no", and nothing else between, make saving wait.
  - A song is given up on at its third. That costs no extra allowance, since a recent "no" is not asked again.
  - It is not listed under Not found, since it was not believed.
- **Try again on every failed song gives them back to the listener.** They are no list's any more, so letting a list go does not take them off.

### "Don't play"

- **The queue is cleaned in one go**, under the player's lock, wherever the songs are. Songs before the one playing go too, since the queue may start over.
- **It also works while the player rests or is still asking an add-on**, as with a queue kept from last time when Stash opens. The song it rests on gives way to the next one kept.
- **Only what the new choice covers leaves.** A song chosen not to be played and then played by itself keeps playing when another artist is chosen out.
- **The phone's blocklist, on import, takes its songs out of the queue too.**
- **"Play again" in Settings goes by the key the choice is kept under**, so a name over 300 characters can be played again.
- **A refusal from the menu is said** (a title with no letters).
- **The window's marks of what was chosen are dropped whenever the lists are read again**, so a newer word from the core wins.

### Library › Songs

- **Searching with "On this computer" asks the core for songs here only**, and the chip turned (or going offline) asks again. A common word no longer says "nothing matches" while songs here do.
- **A click plays the song clicked** even when the list moved since the window read it: the core looks for that song near its old place.
- **The count of songs here** is two plain counts, not a look for a copy per song. The window does not ask while it is hidden.
- **A kept list's progress is read again when Stash comes back online.**
- **Smart playlists pick the page's songs first** and read their columns for the page only.

### Checked

- **Tests in stashd** (`saving::tests`):
  - Each add-on code waits or fails as above, with the add-on's `retry_after` passed along.
  - Trouble that holds every song never gives one up. A song held again after others were saved fails at its second strike, and Try again forgives.
  - Unbelieved "no"s: one song alone is tried again and given up at its third; two songs in a row wait; anything in between ends the run.
  - The wait follows `retry_after`, with a floor of 5 seconds.
- **The allowance API test, extended:**
  - With the only add-on switched off, saving waited ("there is no add-on to save it from") with 3 queued and 0 failed.
  - Switched on with its allowance spent, saving asked at once and waited again ("allowance").
  - Allowance back and Try again: all 3 saved.
- **The saving API test:** with no add-on left, the song waited instead of failing; an add-on added again saved it.
- **The Not found API test passed unchanged:**
  - An unbelieved "no" did not list India.
  - After Golf was found, Hotel's "no" listed it.
  - After the add-on was switched off and on, India's "no" listed it.
- **Player test** `tracks_are_taken_out_wherever_they_are_in_one_go`:
  - Songs before and after the playing one went, and it played on a place up.
  - The playing one went, and the next kept one played.
  - With none kept after it, playback ended and the queue stayed.
  - A played-out queue's last song went, and nothing was shown.
  - A kept queue resting on a song that went rested on the next one kept, from its start, and Play started it.
- **The "Don't play" API test, extended:**
  - A song played by itself kept playing when an artist was chosen out.
  - Play again by key left one block.
  - The artist's two songs before the playing one left the queue, leaving only it, at place 0.
  - Playing from offset 0 with the third row's id queued that song first.
- **The kept-list API test:** a search for every song's artist with `here=1` found the 4 here, and 6 without it.
- **On the built page:** the "Don't play", picking, kept-list and "On this computer" checks all passed again, with no errors in the page.

## 29. Playlist folders

Playlists can be grouped into folders, in the sidebar and in Library › Playlists. A folder holds playlists, not other folders. It has a name and a place after the folders made before it, and it is folded shut or open.

Folders are this computer's own. The phone and the web player have none, and the library file Stash writes for them says nothing of folders. Schema 18 adds the `playlist_folders` table and `playlists.folder_id`.

### Making and filling folders

- **Make a folder** from the "+" beside Your library in the sidebar (it also makes a playlist or a smart playlist), or from the New folder card in Library › Playlists.
- **Put a playlist in a folder** in any of these ways:
  - Carry it with the mouse onto the folder in the sidebar. Carrying it onto "Your library", or among the playlists in no folder, takes it out.
  - Use its menu: right-click it in the sidebar or on its card, or press Move to folder on its page. The menu marks the folder it is in, offers "Not in a folder", and can make a new folder with the playlist in it.
  - Make a new playlist while a folder is open in Library › Playlists: the playlist goes in that folder.
- **Carrying uses the pointer's own events**, as moving songs in Up next does. A drag started by the system (HTML5 drag and drop) does not reach the page in the window on Windows.
  - The place under the pointer lights up, and a chip with the playlist's name follows it.
  - Escape puts it back.
  - The click at the end of a carry is swallowed, so the playlist does not open.

### What a folder shows

- **In the sidebar**, a folder shows its playlists a step in under it. A click folds it shut or open, and the core keeps that for next time.
- **In Library › Playlists**, the top level shows the folders, then the playlists in no folder. A folder's card opens it, with its playlists, Rename and Delete.
- **Searching Library › Playlists** finds playlists wherever they are.
- **A playlist's page** names its folder above its title, as a link back to it.

### Deleting a folder

Deleting a folder keeps its playlists: they come out of it. The deletion is said with Undo, which makes the folder again with the same name and those playlists in it.

### Checked

- **Core test** (`folders::tests`):
  - Folders are made, and an empty name is refused.
  - Playlists go in and out, and a folder or playlist that does not exist is said.
  - Folders are renamed and folded shut.
  - A deleted folder's playlists stay, out of any folder.
  - Deleting a playlist leaves the folder.
- **API test** (in `what_is_done_here_goes_out_as_a_library_file`):
  - A folder made with a playlist in it, renamed, then folded shut.
  - An empty name is refused in words, and renaming a folder that does not exist answers 404.
  - The playlist goes out of the folder and back in by `/api/playlists/{id}/folder`. Each answer carries the folders.
  - The written library file does not name the folder.
  - Deleting the folder keeps the playlist, out of any folder.
- **On the built page** (light and dark, with a scratch library of 6 made-up songs and 3 playlists):
  - **Making a folder:** "+" offered New playlist, New smart playlist and New folder. "Summer" was made in place.
  - **Carrying:**
    - Road was carried onto Summer: Summer lit up, the chip said "Road", Road went in, and the page did not move.
    - Night was carried with Escape pressed: it stayed out.
    - Trip was carried onto Your library: it came out.
  - **Folding:** Summer was folded shut and was still shut after a reload.
  - **Menus:**
    - Trip's right-click menu ("Move to folder: Summer, Not in a folder, New folder") moved it in, and said so.
    - The playlist page's menu took a playlist out.
    - Its keys worked: Escape closed it and focus went back to its button.
  - **Library › Playlists:**
    - The top level showed Summer's card and Night.
    - Opening Summer showed Road and Trip.
    - Renaming it changed the sidebar too.
    - A playlist made inside it went in it, and its page named the folder.
  - **Deleting:** Summer was deleted from its menu and its playlists stayed. Undo brought it back with Road in it.
  - **In the narrow rail:** the folder is a tile, and its playlists' tiles are smaller (32 px against 40).
  - No errors in the page.

## 30. Third review: saving's tries, group actions, Downloads picks

A reviewer read sections 27 and 28 (picking songs, the second review's fixes) and found these. All are fixed. The saving rules below replace §28's strikes and unbelieved "no"s. (How tries are counted changed again after a fourth review: see §32.)

### Saving: whose trouble it is

Every way a song can fail to be saved now falls into one of three kinds.

| Kind | What it covers | What happens |
|---|---|---|
| **Everyone's** | An allowance spent, too many asks (from the add-on or the file's server), a key refused, no add-on to ask, the downloads folder not writable | Saving waits (as long as asked, else a minute doubling to an hour). The song goes to the back of the line. It is never given up for this, so a spent allowance on a long kept list fails nothing. |
| **The song's own** | No add-on has it (believed), a file that cannot be kept, a link that is bad in itself | It fails at once. |
| **Maybe either** | An add-on or the file's server out of reach, busy (502–504) or stopping mid-file; "no"s from add-ons that are not finding other songs either; a request the add-on refused, or answered with what the contract does not allow | See below. |

Trouble that may be either:

- **Each try that asked something counts against the song.** The song is given up on at its third.
- **A refusal is given up on at once**, unless the song before it had the same refusal.
- **Two songs in a row with the same trouble look like everyone's,** so saving waits instead of asking for the next song at once. A plan that lapsed and refuses every request no longer fails a whole list in a minute.
- **The same song twice in a row** (it is the only one left) also waits, so nothing is asked over and over.
- **Every try still counts.** So two songs that each have trouble of their own are not tried for ever, and neither is the last song in line whose file's server keeps failing.
- **An unbelieved "no" is asked again once the add-on is finding other songs:** its answer counts then.
  - A try that only met "no"s remembered from before asked nothing. It does not count, and it waits.
  - Try again forgets the unbelieved "no"s of the songs it retries.

### Group actions

- **"Remove N from this playlist" is one change in the core:** each place with the song there. Nothing is taken out when any place holds another song now (the playlist changed since they were picked), and the window says so. A refusal is said, not hidden behind "Removed".
- **"Don't play these N songs" is one change too.** The core keeps them in one go, cleans the queue once, and answers once; Undo is one change.
- **The queue is cleaned by looking each add-on song's words up once**, not by comparing them with every choice.

### Library › Downloads picks

- **The menu used the first picked song's state for all of them.** A pick with one song still on the way offered "Don't save", which deleted the saved files too.
  - **Now each song goes by its own state.** The menu offers "Don't save the N on the way" (taking only those off the list) and "Remove N saved songs from this computer" apart.
  - **Removing asks first:** "Delete their N files from this computer?" Keep them deletes nothing.
- **The picks go when the songs at their places change.** The list is read every second, in an order that follows each song's state.

### The player

- **`Player::retain` does a new start's bookkeeping under the same hold of the player's state.** It used to hand it to `start_at` after letting go, so a second call in between (an import and a "Don't play" at once) could index past the queue's end and poison the player's lock. A place past the end is never used.
- **A played-out queue whose last song goes now rests on the last song left**, from its start, at once. It used to rest on nothing for a moment, until `settle` put it there.
  - The test checked that passing moment.
  - It now checks the state that stays.

### Checked

- **Tests in stashd** (`saving::tests`):
  - **Which kind each add-on code and miss falls into**, with `retry_after` passed along and whether anything was asked.
  - **A song alone:** tried at once, then after a wait, then given up.
  - **Two songs in a row:** they wait, still count, and are given up at their third.
  - **Remembered "no"s:** tries that asked nothing do not count.
  - **Refusals:**
    - The first refusal fails at once.
    - In a run, refused songs wait and fail at their third.
    - Another refusal code is not the same run.
  - **Try again** starts every song over.
- **The Not found API test**, rewritten for the new rule:
  - India, asked while the add-on found nothing, waited ("your add-ons are not finding songs just now"), with 1 queued and 0 failed, and was not listed.
  - Once Golf was found, India was asked again, believed, failed, and listed.
  - Hotel was listed too.
  - After the add-on was switched off and on, both stayed listed.
- **Group API checks:**
  - **"Don't play" for 2 songs plus one with no letters:** the two were marked "song", the third was passed over, and the queue around them kept only the song playing, at place 0. Played again together, the choices went back to 1.
  - **Removal by places:** a stale place removed nothing (`done` false), and two right places left the third song.
- **Player test:** the resting track's removal left the player resting on the last song left at 0 ms, and still so 600 ms later.
- **On the built page:**
  - **Library › Downloads** with 2 saved songs and 1 waiting, all picked:
    - The pick survived a read of the list.
    - The menu offered "Don't save the one on the way" and "Remove 2 saved songs from this computer".
    - Remove asked first, and Keep them deleted nothing.
    - Not saving the one on the way left both files.
    - Deleting after the question removed both files.
  - **Picking checks:** "Remove 2 from this playlist" (one change) left the right songs. "Don't play these 3 songs" dimmed 3 rows and made 3 choices; Undo took all 3 back.
  - **The "Don't play" checks** passed again.

## 31. Songs by several artists

A credit like "Drake & Future" or "Newcomer (feat. Drake)" names several artists. Stash shows each one as its own link, and each artist's page lists their songs with others.

### How a credit is split

The core splits a credit (`crates/stash-core/src/credits.rs`). The library has no table of credits for this: the split is worked out when it is asked for, from the artists the library has.

- **Some words always mean several artists:** "feat.", "ft.", "featuring", ";" and " / ". The credit is always split there.
- **Others only may:** ",", "&", "and", "x", "with" and "vs.". These split a credit only as far as the pieces are artists the library knows by themselves (some song or album is credited to that name alone).
  - "Drake & Future" splits when Drake or Future has a song of their own here.
  - "Earth, Wind & Fire" and "Simon & Garfunkel" stay whole, because no piece of them is known.
  - "Tyler, The Creator & Drake" splits into two artists, not three, when "Tyler, The Creator" is known: of every way to cut a stretch between sure words, the one naming the most known artists wins, and then the one with the fewest parts.
- **Case and a leading "The" do not matter:** names are compared as they sort.
- **The credit itself never counts as known**, since every song's credit is an artist row of its own.
- **The parts give the credit back as written:** the words between them and a closing ")" are kept.
- **Limits:** a credit of more than 12 pieces, or over 400 characters, is left whole.

### What the window does with it

- **Song rows and Now playing:** each name with a page here is its own link.
  - In a row, a name the library does not know stays text.
  - In Now playing, an unknown name links to a search for it.
  - A credit with no known name links to its own page, as before.
- **The window asks the core about credits in batches** (`POST /api/credits`, at most 400 a time). The credits on screen at one moment go in one ask. A credit with none of the joining words is never asked about.
  - The window keeps the answers.
  - It lets them go when the number of artists in the library changes (someone new may now be known by themselves).
- **The song menu offers "Go to" for each artist with a page**, up to four, in place of one "Go to artist".
- **An artist's page lists their songs with others** under "With others", the most played first.
  - The core reads the credits that hold the artist's name (at most 500) and keeps those whose parts name this artist.
  - "Drakeo the Ruler" holds "Drake" but names no Drake.

### Checked

- **Core tests (`credits::tests`):**
  - **Splitting:** known artists split as above. The band names stay whole. "feat." splits even when no name is known.
  - **Writing back:** eight credits, with "(feat. …)", "(Ft. Future (UK) )", "A; B;" and mixed words, came back exactly as written from their parts.
  - **An artist's songs with others:** Drake had three, not their own song and not "Drakeo the Ruler". Future had one, The Weeknd one, and Simon & Garfunkel none.
- **API test:**
  - **`/api/credits`:** it split "Drake & Future" into both artists with their ids, and "Newcomer (feat. Drake)" into Newcomer (no id) and Drake (id, closing ")"). It gave no parts for "Simon & Garfunkel" or "Drake", and passed over a number.
  - **The artist page:** Drake's own songs were ["One"]; with others, ["Four", "Three"].
- **On the built page, with a scratch library imported from a made-up library file:**
  - Library › Songs asked about credits once, for the 4 credits with joining words.
  - "Ada Vale, Ben Okoro and Cy Marsh" linked Ada Vale and Ben Okoro, and left Cy Marsh as text. "Newcomer (feat. Ada Vale)" linked Ada Vale only. "Simon & Garfunkel" kept its one link.
  - The song menu offered "Go to Ada Vale" and "Go to Ben Okoro". The second opened Ben Okoro's page: Songs ["Low Tide"], With others ["Fireline", "Paper Planes"].
  - A name in a row there went on to Ada Vale's page: with others ["Fireline", "Glasshouse", "Paper Planes"].
  - The duo's page had no "With others".
  - There were no errors in the page.

## 32. Fourth review: saving's tries kept, no bursts; a start never stops a newer one; name fields

A reviewer read sections 29 and 30 (playlist folders, the third review's fixes). It found two major defects in saving and three minor ones; all are fixed. The saving rules below replace §30's counting. The folders work, `retain`, the batch routes and the Downloads picks were found sound.

### Saving

- **An add-on out of reach is everyone's trouble.**
  - No answer at all from an add-on (no connection, or its server down) now holds saving. The song waits its turn, and nothing is spent.
  - The wait is a minute, doubling to five minutes (not an hour), so saving goes on soon after the connection is back.
  - Under §30 a dropped connection failed a song waiting alone in about a minute.
- **Back online, saving goes on at once:** switching Offline off ends a hold.
- **A try counts against a song (a strike) only when another song was saved since its try before.** Then the add-on is giving other songs, so the trouble is likely this one's. A song is given up on:
  - at its third strike;
  - at once for a refusal the song before did not have (unchanged);
  - when nothing tells whose trouble it is for a long while: it has had such trouble for a day and been asked three times in it, or been asked twelve times.

  So a lone song whose server keeps failing is still given up on, and an outage with nothing saved fails nothing for a day. (§34 drops the twelve asks, which broke this for a lone song.)
- **What a song has had is kept in the library** (schema 19: `strikes`, `asks`, `unsure_since` and `tried_at` on its downloads row). A new start of Stash no longer gives every song three new tries. A song saved, asked for again, or retried with Try again starts over.
- **No bursts.** Saving waits before the next song, even after a song is given up on, when:
  - the song before had trouble that may be its own (of any kind now, not only the same);
  - the same song comes again;
  - the add-on asked to be left alone.

  Under §30 the last round of a run asked every song back to back and failed them all within a second. The add-on's "ask again in" was also dropped when the next song was asked at once.

### The player

- **`Player::go` stopped what played before it handed a start to its thread**, without checking that the start was still the newest. Two quick starts (an import and a "Don't play" at once) could stop the newer one's song, and then nothing played.
- **The stop is now in `Ctx::start`**, after its generation check and under the same hold of the player's state. `retry_failed` stops the same way.

### The window

- **Library › Playlists' folder view kept a rename open when another folder was opened** (the page stays), and Save then renamed the other folder. Opening another folder now drops the form.
- **Name fields now get the keyboard when they show** (`lib/ui/focus.ts`), with their words picked. This covers renaming a folder or playlist, a new folder or playlist, and the menus' name fields. `autofocus` did nothing there, because the button that showed them had the keyboard.

### Checked

- **Core test (`lists::tests`):**
  - A song's first try, and a try that asked nothing, are no strike.
  - Another song saved since its try before is one strike, counted once.
  - Try again, and taking the song off the list, start it over.
- **stashd tests (`saving::tests`):**
  - A song is given up on at its third strike, after a day with three asks, or at twelve asks. A try that asked nothing gives nothing up.
  - A run waits, and a give-up in a run still waits. A save, a settled song, or Try again ends a run.
  - The add-on's "ask again in" is waited for even after the first song.
  - Refusals behave as before.
- **API test (the allowance one, extended):**
  - Try again with the add-on out of reach (the demo add-on now answers with no HTTP at all): saving waited, held for at most 61 s, with 3 queued, 0 failed and 0 saved.
  - Once the add-on could be reached, Offline on and then off saved all three at once.
- **Player tests:** all 13 passed, with libmpv.
- **Page check (folders):**
  - Rename put the keyboard in the name, with "Summer" picked.
  - Opening Work from the sidebar dropped the form.
  - The sidebar's Rename put the keyboard in its field.

## 33. Playlist files

Another player's playlist (.m3u or .m3u8, as foobar2000, MusicBee, VLC and most players save them) comes in as a playlist, and a playlist, or the liked songs, goes out as one (`crates/stash-core/src/playlist_file.rs`, routes in `crates/stashd/src/playlist_files.rs`).

### Coming in

The sidebar's "+" menu, or Settings › Your library, opens the system's own dialog for the file.

- **The file is read as written:**
  - UTF-8 (with or without a byte-order mark), else the Windows code page older players write .m3u files in.
  - `#PLAYLIST` names the playlist, else the file's name does.
  - Each `#EXTINF` gives its entry a length and "Artist - Title".
  - At most 10,000 entries, and a file of at most 5 MB.
- **Each entry becomes, in this order:**
  - **A file the library has**, by its full path. A path written relative to the playlist file is found beside it, and `file://` addresses are read too. Paths are made whole the way a scan writes them, and on Windows any case matches.
  - **A file on this computer the library does not have yet.** Its tags are read and it is added, as a scan would add it.
  - **The song the entry names** (its `#EXTINF` words, else a file name like "Artist - Title"), found among the library's songs or kept as one the add-ons play.
  - **Left out:** web addresses (radio streams), and entries that name no song.
- **The window says what came of it** ("Made Road trip: 4 songs; 1 left out"), and opens the new playlist.

### Going out

A playlist's page has "Save as a playlist file", and Library › Liked has the same for the likes.

- **The file** is an .m3u8: its name, then each song that is a file on this computer (its own, or saved from an add-on), by its full path, with its length, artist and title.
- **Songs only the add-ons play are left out**, since no other player could play them, and the window says how many. A list with no such file saves nothing, and says why.
- **The file's name is the playlist's**, without the characters Windows refuses. The counts travel as `X-Songs` and `X-Left-Out`, which the core now lets the window read.

### Also fixed

- **`Player::retain` stopped the engine after letting go of the player's state** when the playing track went with nothing kept after it. A start begun in between would have been stopped (the race §32 fixed for starts). It now stops under the same hold.
- **The test of that step waited for "idle"**, which lasts only until `settle` rests the player on the last track left. It now waits for either, then for the rest on that track. It failed about one run in five with all player tests at once; it passed six runs of six after the change.

### Checked

- **Core tests (`playlist_file::tests`):**
  - A file with a byte-order mark, CRLF lines, comments, a quoted comma in an `#EXTINF` attribute, a lower-case `#extinf`, a web address, a relative path and a `file://` address was read as written.
  - An old .m3u in the Windows code page read "Beyoncé", "Déjà Vu – Live".
  - A leading track number is no artist.
  - Places: relative paths were made whole beside the file, `file:///C:/Music/x%20y.mp3` became `C:\Music\x y.mp3`, and `C:/Music/../Music/a.mp3` became `C:\Music\a.mp3`.
- **stashd tests:** file names lost the characters Windows refuses, and names were encoded for `filename*`.
- **API test (`a_playlist_file_comes_in_and_goes_out`):**
  - Library: two files scanned, one outside the library.
  - Import of a file with an absolute path, a relative one with forward slashes, a relative one with backslashes to the outside file, a missing file named by its `#EXTINF`, a radio address and a nameless entry: "Road trip", 4 songs (2 files the library had, 1 added, 1 by words), 2 left out, and the songs in the file's order.
  - Export: 3 songs and 1 left out, the file name "Road trip.m3u8", and the first entry's path exact. Brought in again, every entry was a file the library had.
  - Refusals: not a playlist file, not found, no songs in it, and no such playlist (404).
- **On the built page** (the system's dialog stood in for by answering its route with a path):
  - Saving "Night Drive" downloaded "Night Drive.m3u8", with 3 files in it and "One song only your add-ons play is not in it".
  - The "+" menu offered "Import a playlist file". Importing "From MusicBee.m3u" opened the new playlist, with 4 songs in the file's order (the file from outside the library added, the missing one found by its words among the library's songs) and 1 left out.
  - Settings had the button.
  - Saving the likes (one song only add-ons play) saved nothing, and said why.
  - There were no errors in the page, apart from the covers the made-up files do not have.

## 34. Fifth review: out of reach only when no add-on answered; copies say nothing; credits split better

A reviewer read sections 31 and 32 (songs by several artists, the fourth review's fixes). It found two major defects, two medium ones and three minor ones; all are fixed. The saving rules below amend §32's. The schema 19 migration and the player change were found sound.

### Saving

- **"Out of reach" now means nothing reached the add-on.**
  - A request counts as out of reach only when it never got there. That is: the server's name was not found, nothing listened on its port, there was no route to it, or connecting took longer than 8 s (finding the server by name, longer than 5 s).
  - Before, every error that came without an answer counted, including a line that broke after the request went out and an answer that was not HTTP. Those are now the add-on not working, which is trouble that may be the song's own: the add-on may have been asked, and spent something on it.
- **One add-on out of reach while another answered is not the connection being down.**
  - Here the other add-on said it does not have the song, now or a little while ago. The connection was up, so this is trouble that may be the song's own, and its rules apply (strikes, a day).
  - Before, one add-on that was switched off held such a song for ever, and asked the others for it again every six hours.
  - When two add-ons had trouble, the one that answered now says more than the one out of reach, whichever came last.
- **The window says why saving waits in words of its own:** "no add-on can be reached (is this computer online?)".
- **A song saved from a copy kept from playing it says nothing of the add-ons.**
  - It no longer ends a run of trouble, starts the wait's doubling over, or counts as a strike against the songs tried before it.
  - On the downloads row, `tried_at` of a saved song now says when an add-on gave it, and is empty for a copy. There is no schema change.
- **No twelve-ask limit; trouble that comes again waits longer each time.**
  - §32 gave a song up at its twelfth ask. A lone song reached that about five hours into an add-on's outage, although §32 said nothing fails for a day.
  - Now a song with no strikes is given up on only after a day with three asks in it.
  - Each time such trouble comes again (the song before had it too, or this one is the only one left), saving waits twice as long as the time before. That is at least as long as the add-on asked, and at most an hour, so an add-on that asks to wait five seconds is not asked every five seconds. The first such wait is as long as the add-on asked (true only since §35: before, it was at least a minute, or twice a wait that had ended long before).
- **Going back online ends only a wait for add-ons out of reach, and only when Stash was offline.** Before, every request saying `offline: false` ended any wait, an allowance spent included, and started its doubling over.

### Songs by several artists

- **Names that end in a joining word now split.**
  - "Lil Nas X & Jack Harlow" was read as "Lil Nas" and "& Jack Harlow", because " X " took the space " & " needed. It was not split.
  - The places a credit may be cut may now share their spaces, and the split chooses among cuts that do not overlap.
  - The same held for names ending in "and", "with" or "vs".
- **The ")" that closes a "(feat." now closes it where a name ends: before the end of the credit, or before another place it may be cut.** ("Drake (feat. Future) Remix" keeps "Future) Remix" as one name, with no link.)
  - In "Drake (feat. Future) & Newcomer", Future's part was "Future) & Newcomer" (Newcomer not being known), which never linked.
  - It now reads as Drake, Future, Newcomer.
- **An artist page's "With others" reads only credits that hold the artist's name as words of their own** ("yo" is part of many names), up to 500 of them. Before, the first 500 credits holding those letters were read, so for a short name the real ones could be left out.
- **The window's cache of credits no longer empties itself past 4,000 credits.**
  - With more than that on screen (a long Liked list), it asked again for ever.
  - It is let go when the number of the library's songs, albums or artists changes (and, since §35, when a scan ends). Before, only a change in the number of its artists did.
  - An answer asked for before such a change is thrown away, and the credits are asked again.
  - An ask that failed is asked again 5 s later (since §35 also when a credit was wanted while it was out; before, that ask went again after 30 ms). Before, those credits stayed unsplit until the library changed.

### The window

- **Any change dropped a folder rename in Library › Playlists' folder view** (a like, or a folder made elsewhere), because the folders are read again after it.
  - The form now follows the folder's id.
  - Only opening another folder drops it.

### The demo add-on

- **Out of reach, nothing answers on its port.**
  - It now listens through a door in front of its server. Shut, nothing listens there, so a connection is refused, and the connections open are cut, as when a server goes down.
  - Before, it answered with text that is not HTTP, which now counts as the add-on not working.
  - `out_of_reach()` gives a switch that returns once the door is so.

### Checked

- **Core tests:**
  - Credits, newly split:
    - "Lil Nas X & Jack Harlow" and "Ben With x Drake" split. ("Jack Harlow and Lil Nas X", listed here before, split already; §35 adds "Lil Nas X and Jack Harlow", which did not.)
    - "Lil Nas X" alone did not.
    - "Drake (feat. Future) & Newcomer" read as Drake, Future, Newcomer (not known).
    - In "Drake (feat. Future), Newcomer", Future's part closed with ")".
  - Credits, given back as written: five more credits, among them "Drake (feat. Future)) & Newcomer", "Drake (feat. ) & Future" and "Drake; ; Future".
  - `lists::tests`: a save from a copy was no strike; a save an add-on gave was one, counted once.
- **stashd tests:**
  - Out of reach only when no add-on answered; with an answer, it was trouble that may be the song's own.
  - Trouble that came again waited 5, 10, then 20 s after an add-on asked for 5 s. It waited 600 s when asked for 600 s, and an hour at most.
  - Going back online ended a wait for add-ons out of reach, and not one for an allowance spent.
  - With no strikes, 40 asks within a day gave nothing up.
- **API tests:**
  - The allowance test, with the door shut (the connection refused):
    - Saving was held, saying "no add-on can be reached", for at most 61 s, with 3 queued and 0 failed.
    - Once online again, all three were saved.
  - New: one add-on out of reach while the other said it does not have the song. Saving was held, but not as out of reach, with 1 queued and 0 failed.
    - It first failed on the second try. The other add-on's "no" was remembered but not believed, so the other add-on was not asked, and only the one out of reach was left.
    - A remembered "no" now counts as an answer.
- **Page checks:**
  - Credits: one ask for 4 credits; links and "With others" as before.
  - Folders: a rename kept "Summer 20" while a folder made elsewhere appeared, and opening another folder still dropped the form.
- **All core and stashd tests, clippy, and the window's check, unit tests (53) and build** passed.

## 35. Sixth review: an answer cut off is the add-on not working; old "no"s and old waits say nothing now

A review read §34's commit. It confirmed 28 findings: 6 medium and 22 minor. A further 2 were contested, and those are fixed too. Several findings were the same defect seen from two sides, so they are listed together below. All of them are fixed.

### Saving

- **An answer that stops coming in is the add-on not working, not a broken answer.**
  - A line that broke, or went quiet, after the answer's first lines counted as an answer that breaks the contract. A song was then given up on at once, or counted as a "no" and put on Not found.
  - Now it is trouble that may be the song's own: tried again, with strikes and a day.
  - An answer that went on too long, or was not text, still breaks the contract. A busy or failing answer whose words were cut off still counts as what its status says (a 429 is still busy, with its wait).
- **Out of reach is now a short list, not "anything else".**
  - The list: the name was not found, nothing listened, there was no way there, or connecting took too long.
  - Before, everything but six kinds of trouble counted, among them a certificate that does not hold. An add-on whose certificate ran out was "no add-on can be reached (is this computer online?)" for ever.
  - A name the system could not find (on Windows this comes with no kind of its own) now counts as not found.
- **An add-on that answered counts only when it answered now, or a little while ago and is still answering.**
  - A "no" remembered from hours before counted as an answer. So did one from the very add-on now out of reach. Then a real outage was taken for busy, songs were given up after a day, and the wait outlived going back online.
  - Now an add-on counts as having answered when it answered this time, whatever it said. A "no" remembered from before counts only when its add-on answered in the last five minutes and was not out of reach this time.
  - An add-on that is never asked for a song (an Eclipse add-on that finds songs only by ISRC, for a song with none) no longer counts as asked.
- **A wait doubles only from a wait just before it, of the same kind.**
  - The doubling counted from the last wait ever, however long ago. So one 503 after an idle spell could hold saving for an hour, even when the add-on asked for 5 s. A wait out of reach right after a short wait the add-on asked for was 10 s, not a minute.
  - Now it doubles only from a wait that ended no longer ago than it lasted, and only from one of the same kind (out of reach, or not).
  - A run of trouble ends once saving has been quiet for two minutes past any wait.
  - The first such wait is now as long as the add-on asked, or a minute when it asked nothing, as §34 said.
- **Which trouble is kept when several add-ons fail, by what it says about the song.**
  - Busy, slow, out of its allowance or not working says most. Out of reach comes next. A refusal says least, since the add-on out of reach may have the song once it is back.
  - Before, an answering add-on's refusal outranked one out of reach. The song was given up on at once, although the add-on that had it was only briefly down.
- **A restart no longer fails the first song refused.**
  - A refusal is the song's own only on the song's first ask. A song asked before, and kept in line because the songs around it were refused the same way, is not given up on at once after Stash starts again.
- **A song fixed in Not found starts its trouble over**, as any song asked for again. Before, it kept its old strikes, so its first trouble gave it up.
- **Adding an add-on, or switching one on, wakes saving after the list of add-ons is read again.** Before, the try it woke could use the old list and start a new wait.

### Songs by several artists

- **A "(feat." group with ";" or " / " inside it now closes on its ")".** In "Drake (feat. Future; Newcomer)", Newcomer's part was "Newcomer)", which never linked.
- **Two marks between the same two names part them as one.** That covers the Oxford comma (", and", ", &") and a mark written twice (", ,", "& &", "; ;"). Before, the second mark stayed at the start of the next name, so it never linked.
- **Each name is looked up as written.** One shared lookup was kept by sort key, so whether "The Weeknd" linked to "Weeknd" or "The Weeknd" depended on which was read first.
- **The window lets go of what it was told when a scan ends.** A scan can rename artists without changing any count.

### The demo add-on

- **The door waits for connections instead of checking every 2 ms.**
- **Switching it out of reach and back says when it could not.** It gives the reason when the port could not be listened on again, or did not open or shut in time. Before, it gave up silently after 5 s.

### Docs and tests

- **§34 corrected:**
  - the first wait;
  - where a ")" closes;
  - Future's old part;
  - when the window's cache is let go;
  - the example that was said to split newly but split before.
- **Doc comments corrected:**
  - a strike, which is no longer "another song was saved";
  - the limit on a credit's pieces;
  - `sort_key`'s doc, back on `sort_key`.
- **New tests:**
  - What counts as out of reach, and what an answer cut off is. That includes a real server on this computer that hangs up partway through an answer, with a 200 and with a 429.
  - Which trouble is kept when two add-ons fail.
  - Saving's waits:
    - a wait does not double from one that ended long ago, or from one of the other kind;
    - a run ends after a quiet spell;
    - a song refused before is not failed at once after a restart;
    - trouble that comes again with a 5 s ask waits 5, 10 and 20 s through the real path;
    - a song saved from a copy leaves the wait and the run as they were.
  - The lower bound of "at least a few seconds".
  - Credits:
    - "Drake (feat. Future; Newcomer)" and "Drake (feat. Future / Newcomer)";
    - "Ada Vale, Ben Okoro, and Cy Marsh";
    - doubled marks;
    - names ending in "and" or "vs";
    - "Lil Nas X and Jack Harlow";
    - the lookup by exact name;
    - "With others" past 510 longer names that only hold the artist's.
  - A song fixed in Not found starts with no strikes.
  - In the API, the out-of-reach test now proves the add-on out of reach was asked: the wait is in its words. Saying "online" while Stash already was leaves a wait out of reach in place.
  - In the window, the credits cache:
    - one ask for the credits shown together;
    - asked again 5 s after a failure, with what was wanted meanwhile;
    - an answer from before a library change let go;
    - 4,500 credits all kept.

### Checked

- **All core tests passed (73 unit tests, plus every integration test). All stashd tests passed (18 unit tests, 32 API tests).**
- **Clippy passed.**
- **The window passed its check, its 57 unit tests and its build.**

## 36. Similar music: radio, and autoplay that knows what is like what

A radio is a queue that grows by itself with songs like the one it starts from: a song (its menu, or Ctrl+K for the song playing), or an artist (the artist page). Autoplay at the end of a queue is the same engine, started from the last songs. Last.fm says which artists and songs are alike. The songs come from the library first, and an add-on is searched only for artists the library has too few of.

### What is sent, and to whom

- **Questions go to Stash's own Last.fm proxy, which holds the key.** The program has no Last.fm key. A build carries only the proxy's address, given when it is built (`STASH_LASTFM_PROXY`; `stashd serve --lastfm-proxy`). A build without it has no radio, and autoplay picks from the library as before.
- **Two questions, both GET, as the phones ask them:**
  - `track.getsimilar` with the song's artist and title, `autocorrect=1`, `limit=30`;
  - `artist.getsimilar` with the artist's name, `autocorrect=1`, `limit=100`.
  - Headers: `User-Agent: Stash-desktop/<version>` and `X-Stash-Version: desktop/<version>`. Redirects are not followed, and an answer over 1 MB is not read.
- **Nothing else.** Nothing about the library, what is played or any account. The proxy sees this computer's internet address, as any server does.
- **What is asked about is cleaned first:** the first artist a credit names, and the title without the words video sites add ("(Official Audio)", "- Topic"). A song whose words say too little ("Unknown artist", a phone's recording, a file name) is never asked about.
- **The address never shows.** No answer, message or log line carries it; a test reads every answer the window sees for it, and `crates/stashd/tests/no_private_addresses.rs` fails if an address of that kind or a key is put in the repository.

### How often

- At most 2 questions in a station's life: the song's, then the artist's when the song named fewer than 4 other artists, or when its artists are used up.
- Each answer is kept in the library (schema 20, `similar_cache`) and is fresh for 14 days, so each question is asked once in that time. Answers older than 60 days, and the oldest past 5,000, go when the core starts. "Not found" is kept too.
- At most 100 questions a day; past that, stations pick from the library until the next day.
- After trouble, Last.fm is left alone: as long as a 429 asks (else a minute), or a minute doubling to an hour while it cannot be reached. Meanwhile a kept answer of any age is used. A question that failed twice is not asked again in that run.
- Never offline. Offline, kept answers are used, and only songs on this computer are picked.
- Autoplay acts once a song. One thread asks, so no question goes out twice at once, and the library is never held while it waits.

### What a station picks

- **Per batch of 10:** at most 3 by the seed's artist (2 for autoplay, whose artist was just heard), at most 2 by any other. Artists come in Last.fm's order.
- **Never the same artist twice in a row,** nor right after the song before (`shuffle::apart`, which autoplay's own picks now use too).
- **Last.fm's titles first** for each artist, then liked songs, then the most played. "Song - 2011 Remaster" is Last.fm's "Song"; "Song (Live)" is not.
- **Left out:**
  - the seed itself;
  - another version Last.fm did not name;
  - songs and artists chosen not to be played;
  - songs no add-on finds;
  - missing files;
  - songs heard in the last 3 hours;
  - anything already in the queue or added by a station earlier in the run.
- **It grows** when 3 or fewer songs are left after the one playing, while repeat is off.
- **It ends** when another queue starts, when Stop is pressed (its songs still to come are taken out, songs added by hand stay, and autoplay does not act on the song playing), or when it has nothing more. A batch found for a queue that was replaced meanwhile is dropped.
- **When Last.fm cannot help** with a station's first songs (not reached, busy, offline, nothing like it, or asked enough today), the library picks them as autoplay always did, from the seed and its artist. Up next says so: "Songs like Teardrop from your library", with why on hover.

### The window

- **Start radio in a song's menu**: for the song playing, it plays on and what follows is replaced; any other song plays now. Also **a radio button on the artist page**, and **Ctrl+K** ("Start radio from the song playing", "Stop radio").
- **Up next keeps "Playing from Teardrop radio"** and draws one line above the station's songs still to come: "Similar to Teardrop · from Last.fm · Stop" (autoplay: "Autoplay · like Teardrop · from Last.fm · Stop"; from the library: "Autoplay · from your library · Stop").
  - "Last.fm" opens the seed's page on Last.fm. The core builds the link from the seed, never from the request, and the shell opens only `https://www.last.fm/` links. The window is granted nothing of the opener.
  - While its first songs are found: "Finding songs like Teardrop…".
- **Settings › Sound › Similar music from Last.fm** (on by default; shown only in a build that has it) says what is sent. Off, nothing is sent, there is no radio, autoplay keeps to the library, and what was kept is deleted. The Autoplay and Offline lines say what changes.

### Checked

- **Core tests:**
  - `similar` (Last.fm's answer shapes; errors 6 and 7 as not found, 29 and a 429 as busy, the proxy's own errors as unreachable; the questions as the phones send them; the lookup rules of the phones; an address that is not https, or carries a name, query or fragment, refused; one GET with Stash's headers to a stand-in; redirects not followed);
  - `station` (12 tests: shares and spreading over 5 batches, no song twice by any of its ids, when it asks the second question, blocked artists, spent);
  - the library's picks by artist and the kept answers ageing out (`tests/lists.rs`);
  - `apart`;
  - a radio from the song playing keeps it playing as the first of a new queue, and songs are added only to the queue they were found for (`tests/player.rs`).
- **API tests** (`crates/stashd/tests/radio.rs`, 10, against a stand-in for the proxy and the demo add-on):
  - one question for a song radio, as the phones ask it;
  - library songs as library rows, the add-on's for artists the library lacks, one search per artist;
  - growing to at most 2 questions;
  - kept answers asked again by no one, in the same run or the next;
  - Stop leaves the song added by hand;
  - blocked songs and artists never come, and are never searched;
  - offline asks no one;
  - autoplay from Last.fm, and from the library for an unaskable last song;
  - busy, unreachable, not found and the day's limit all go on from the library;
  - a new queue drops a batch that came late;
  - switched off, the kept answers are deleted;
  - without the proxy there is no radio;
  - an artist radio.
  - Ran 4 times in a row with no failure.
- **On the built page served by the core** (debug stashd, silent, volume 0), light and dark, against the stand-in:
  - Start radio on Teardrop played it as "Playing from Teardrop radio" with 10 songs after it: Glory Box first, Massive Attack between the others, Zero 7 and Morcheeba from the add-on.
  - The line read "Similar to Teardrop · from Last.fm · Stop". Last.fm opened `https://www.last.fm/music/Massive+Attack/_/Teardrop`, refused in the browser for the check. Stop said "Radio stopped" and left nothing after the song.
  - The artist page's radio started "Massive Attack radio".
  - Ctrl+K listed both entries.
  - The Settings row and its switch worked; off, the song menu had no Start radio.
  - Last.fm was asked twice in all.
  - No page errors. The 404s were covers of the test files, which have none.

### Not covered yet

- The real proxy: no test or check asked it.
- How Last.fm's terms fit the way Stash is offered (the owner's call), and the Last.fm credit the terms ask for: the "powered by AudioScrobbler" page they point to is gone, so the window names Last.fm in words, with a link.
