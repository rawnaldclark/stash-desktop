//! A made-up library of any size, the same every time for the same seed: what the measurements run on.
//!
//! The shape follows a real collection loosely: about twelve tracks an album, a few albums an artist, titles of one to
//! five words drawn from a small vocabulary so that some words are common ("love" finds thousands) and some are rare.

use crate::library::{NewTrack, Origin};

const WORDS: &[&str] = &[
    "love",
    "night",
    "blue",
    "heart",
    "time",
    "light",
    "dream",
    "fire",
    "rain",
    "home",
    "road",
    "river",
    "summer",
    "winter",
    "moon",
    "sun",
    "star",
    "shadow",
    "golden",
    "silver",
    "broken",
    "wild",
    "quiet",
    "electric",
    "paper",
    "glass",
    "velvet",
    "neon",
    "midnight",
    "morning",
    "ocean",
    "city",
    "ghost",
    "garden",
    "mirror",
    "thunder",
    "echo",
    "ember",
    "hollow",
    "distant",
    "little",
    "last",
    "first",
    "lonely",
    "dancing",
    "falling",
    "burning",
    "sleeping",
    "running",
    "waiting",
    "stranger",
    "angel",
    "devil",
    "king",
    "queen",
    "soldier",
    "sailor",
    "dancer",
    "machine",
    "radio",
    "satellite",
    "highway",
    "harbor",
    "mountain",
    "desert",
    "island",
    "avenue",
    "station",
    "window",
    "letter",
    "photograph",
    "memory",
    "promise",
    "secret",
    "silence",
    "signal",
    "static",
    "gravity",
    "orbit",
    "horizon",
    "cathedral",
    "carousel",
    "lantern",
    "compass",
    "anchor",
    "feather",
    "copper",
    "indigo",
    "scarlet",
    "amber",
    "ivory",
    "cobalt",
    "saffron",
    "juniper",
    "magnolia",
    "willow",
    "cedar",
    "sparrow",
    "raven",
    "falcon",
    "wolf",
    "tiger",
    "serpent",
    "phoenix",
    "labyrinth",
    "kaleidoscope",
    "zeppelin",
    "quasar",
    "xylophone",
];
const SMALL: &[&str] = &[
    "of", "the", "in", "and", "for", "my", "your", "no", "a", "to",
];
const CODECS: &[(&str, u32, u32)] = &[
    ("flac", 44_100, 16),
    ("flac", 96_000, 24),
    ("opus", 48_000, 0),
    ("aac", 44_100, 0),
];

/// SplitMix64: small, fast, and the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A word, the early ones in the list far more often than the late ones.
    fn word(&mut self) -> &'static str {
        let n = WORDS.len() as u64;
        let i = (self.below(n) * self.below(n)) / n;
        WORDS[i as usize]
    }

    fn phrase(&mut self, max_words: u64) -> String {
        let count = 1 + self.below(max_words);
        let mut out: Vec<String> = Vec::with_capacity(count as usize);
        for i in 0..count {
            if i > 0 && i + 1 < count && self.below(4) == 0 {
                out.push(SMALL[self.below(SMALL.len() as u64) as usize].to_string());
            } else {
                out.push(capitalize(self.word()));
            }
        }
        out.join(" ")
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `count` tracks: artists with one to four albums of eight to fourteen tracks. Deterministic for a `seed`.
pub fn tracks(count: usize, seed: u64) -> impl Iterator<Item = NewTrack> {
    let mut rng = Rng(seed);
    let mut made = 0usize;
    let mut artist_no = 0u64;
    let mut pending: Vec<NewTrack> = Vec::new();
    std::iter::from_fn(move || {
        if made >= count {
            return None;
        }
        if pending.is_empty() {
            artist_no += 1;
            // A number keeps two artists with the same words apart, as "Echo 2" would in a real collection.
            let name = rng.phrase(3);
            let artist = if rng.below(5) == 0 {
                format!("The {name}")
            } else {
                name
            };
            let artist = format!("{artist} {artist_no}");
            for _ in 0..1 + rng.below(4) {
                let album = rng.phrase(4);
                let year = 1960 + rng.below(67) as i32;
                let (codec, rate, bits) = CODECS[rng.below(CODECS.len() as u64) as usize];
                let n_tracks = 8 + rng.below(7) as u32;
                for no in 1..=n_tracks {
                    let title = rng.phrase(5);
                    pending.push(NewTrack {
                        locator: format!(
                            "D:/Music/{artist}/{album} ({year})/{no:02} {title}.{codec}"
                        ),
                        title,
                        artist: artist.clone(),
                        album_artist: None,
                        album: Some(album.clone()),
                        year: Some(year),
                        disc_no: 1,
                        track_no: no,
                        duration_ms: 120_000 + rng.below(300_000),
                        isrc: None,
                        origin: Origin::File,
                        addon_id: None,
                        codec: Some(codec.to_string()),
                        sample_rate: Some(rate),
                        bit_depth: if bits == 0 { None } else { Some(bits) },
                        added_at: 1_500_000_000 + rng.below(290_000_000) as i64,
                        file_size: None,
                        file_mtime: None,
                    });
                }
            }
            pending.reverse();
        }
        made += 1;
        pending.pop()
    })
}

/// Writes `count` made-up tracks as real files under `root` (`Artist/Album/NN Title.wav`): a tenth of a second of
/// silence each, with ID3 tags, so a folder scan has something true to read. Returns how many were written.
pub fn write_files(root: &std::path::Path, count: usize, seed: u64) -> std::io::Result<usize> {
    use lofty::config::WriteOptions;
    use lofty::prelude::*;
    use lofty::tag::{Tag, TagType};

    // 16-bit stereo, 44.1 kHz, 4,410 frames of silence.
    let data_bytes: u32 = 4_410 * 4;
    let mut wav = Vec::with_capacity(44 + data_bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&44_100u32.to_le_bytes());
    wav.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    wav.resize(44 + data_bytes as usize, 0);

    let mut written = 0usize;
    for track in tracks(count, seed) {
        let album = track.album.clone().unwrap_or_default();
        let folder = root.join(&track.artist).join(&album);
        std::fs::create_dir_all(&folder)?;
        let path = folder.join(format!("{:02} {}.wav", track.track_no, track.title));
        std::fs::write(&path, &wav)?;
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title(track.title);
        tag.set_artist(track.artist);
        tag.set_album(album);
        tag.set_track(track.track_no);
        tag.save_to_path(&path, WriteOptions::default())
            .map_err(std::io::Error::other)?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_makes_the_same_library_of_the_asked_size() {
        let a: Vec<String> = tracks(500, 7).map(|t| t.locator).collect();
        let b: Vec<String> = tracks(500, 7).map(|t| t.locator).collect();
        let c: Vec<String> = tracks(500, 8).map(|t| t.locator).collect();
        assert_eq!(a.len(), 500);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn albums_run_in_track_order_under_one_artist() {
        let all: Vec<NewTrack> = tracks(40, 1).collect();
        let first_album = all[0].album.clone();
        let run: Vec<u32> = all
            .iter()
            .take_while(|t| t.album == first_album && t.artist == all[0].artist)
            .map(|t| t.track_no)
            .collect();
        assert!(run.len() >= 8);
        assert_eq!(run, (1..=run.len() as u32).collect::<Vec<_>>());
    }
}
