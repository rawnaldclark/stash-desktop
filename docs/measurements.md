# Measurements

Milestone 1 asks four questions, each answered with numbers on real hardware:

1. Does a 100,000-track library open, search and page fast enough that a list never waits? **Measured below.**
2. Does the audio engine play gapless, and in an exclusive bit-perfect mode? **Gapless and bit-exact output: measured below. Exclusive mode on a real device: not yet.**
3. Does a list of 100,000 rows and an album grid scroll smoothly in the window? *Not measured yet.*
4. What do start-up time and memory look like for the whole app? *Not measured yet.*

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
