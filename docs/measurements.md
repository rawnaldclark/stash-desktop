# Measurements

Milestone 1 asks four questions, each answered with numbers on real hardware:

1. Does a 100,000-track library open, search and page fast enough that a list never waits? **Measured below.**
2. Does the audio engine play gapless, and in an exclusive bit-perfect mode? **Gapless and bit-exact output: measured below. Exclusive mode on a real device: not yet.**
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

### Not covered yet

- **Exclusive mode on a real device.** These measurements stop at the engine's output stream. Whether WASAPI exclusive mode opens at each track's own rate, and what it does to other programs' sound, has to be checked on a sound card.
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
