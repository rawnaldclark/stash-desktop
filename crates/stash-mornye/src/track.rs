//! A song as an add-on gives it (spec §9 "Track object fields read"; Lokal `MorneyeTrack.of`, pkgs.dart:319-610).

use serde::Serialize;
use serde_json::{Map, Value};
use url::Url;

use crate::manifest::text_of;

const MAX_NAME: usize = 500;

/// Amazon's image hosts, whose paths carry size tokens (`._SX1500_.jpg`).
const AMAZON_IMAGE_HOSTS: [&str; 6] = [
    "m.media-amazon.com",
    "images-na.ssl-images-amazon.com",
    "images-eu.ssl-images-amazon.com",
    "images-fe.ssl-images-amazon.com",
    "images.amazon.com",
    "ecx.images-amazon.com",
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MTrack {
    pub id: String,
    pub name: String,
    /// Joined with ", ".
    pub artists: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub isrc: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<String>,
    /// Its quality label (`audio_quality`, `audioQuality`, `quality`).
    pub quality: Option<String>,
    /// FLAC, ALAC, OPUS, VORBIS, E-AC-3, AAC or MP3.
    pub codec: Option<String>,
    pub bit_depth: Option<u32>,
    /// In Hz (values of 384 or less are read as kHz).
    pub sample_rate: Option<u32>,
    /// The largest cover it names.
    pub artwork: Option<String>,
    /// Whether it is offered in Dolby Atmos.
    pub atmos: bool,
    /// As the add-on gave it: handed back to it when the song is downloaded.
    #[serde(skip)]
    pub raw: Value,
}

impl MTrack {
    /// The song `value` describes; `None` without an id, without a name (when `need_name`), or for a row whose
    /// `item_type` is something other than `track`.
    pub fn of(value: &Value, need_name: bool) -> Option<MTrack> {
        let map = value.as_object()?;
        let id = text_of(Some(present(map, "id")?));
        if present(map, "item_type").is_some_and(|kind| kind.as_str() != Some("track")) {
            return None;
        }
        let name = present(map, "name").or_else(|| present(map, "title"));
        if need_name && name.is_none() {
            return None;
        }
        Some(MTrack {
            id,
            name: text_of(name).chars().take(MAX_NAME).collect(),
            artists: artists(map),
            album: album(map),
            duration_ms: first(map, &["duration_ms", "durationMs"])
                .and_then(number)
                .filter(|ms| *ms > 0.0)
                .map(|ms| ms as u64),
            isrc: map
                .get("isrc")
                .and_then(Value::as_str)
                .filter(|isrc| !isrc.trim().is_empty())
                .map(str::to_string),
            track_number: whole(first(map, &["track_number", "trackNumber"])),
            disc_number: whole(first(map, &["disc_number", "discNumber"])),
            year: year(map),
            quality: first(map, &["audio_quality", "audioQuality", "quality"])
                .map(|q| text_of(Some(q))),
            codec: codec(map),
            bit_depth: bit_depth(map),
            sample_rate: sample_rate(map),
            artwork: artwork(map),
            atmos: atmos_mode(value).is_some(),
            raw: value.clone(),
        })
    }
}

/// The songs a search or a link gave: a list; or `{tracks}`, `{album: {tracks}}`, `{artist: {top_tracks}}`,
/// `{track}`.
pub fn rows(result: &Value) -> Vec<MTrack> {
    let listed = match result {
        Value::Array(list) => Some(list.clone()),
        Value::Object(map) => {
            let found = present(map, "tracks")
                .or_else(|| result.pointer("/album/tracks").filter(|v| !v.is_null()))
                .or_else(|| {
                    result
                        .pointer("/artist/top_tracks")
                        .filter(|v| !v.is_null())
                });
            match found {
                Some(Value::Array(list)) => Some(list.clone()),
                Some(_) => None,
                None => present(map, "track").map(|track| vec![track.clone()]),
            }
        }
        _ => None,
    };
    listed
        .unwrap_or_default()
        .iter()
        .filter_map(|row| MTrack::of(row, true))
        .collect()
}

/// The Dolby Atmos rendition a song's own fields name (lib/services/tidal_atmos_metadata.dart): `AC4_ATMOS`,
/// `EAC3_JOC` or `DOLBY_ATMOS`.
pub(crate) fn atmos_mode(value: &Value) -> Option<&'static str> {
    fn collect(value: &Value, modes: &mut Vec<String>) {
        match value {
            Value::Array(items) => items.iter().for_each(|item| collect(item, modes)),
            Value::String(text) => {
                for part in text.split([',', ';', '|']) {
                    let mode: String = part
                        .trim()
                        .to_uppercase()
                        .chars()
                        .map(|c| if c == ' ' || c == '-' { '_' } else { c })
                        .collect();
                    modes.push(mode);
                }
            }
            _ => {}
        }
    }
    let map = value.as_object()?;
    let mut modes = Vec::new();
    for key in [
        "audio_mode",
        "audioMode",
        "audio_quality",
        "audioQuality",
        "spatial_audio",
        "spatialAudio",
        "audio_modes",
        "audioModes",
        "audioQualities",
        "audio_qualities",
    ] {
        if let Some(value) = map.get(key) {
            collect(value, &mut modes);
        }
    }
    let has = |names: &[&str]| modes.iter().any(|mode| names.contains(&mode.as_str()));
    if has(&["AC4_ATMOS", "AC_4_ATMOS"]) {
        Some("AC4_ATMOS")
    } else if has(&["EAC3_JOC", "E_AC_3_JOC"]) {
        Some("EAC3_JOC")
    } else if has(&["DOLBY_ATMOS", "ATMOS"]) {
        Some("DOLBY_ATMOS")
    } else {
        None
    }
}

/// `map[key]`, when it is there and not null.
fn present<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|value| !value.is_null())
}

/// The first of `keys` that is there (Lokal's `a ?? b ?? …`).
fn first<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| present(map, key))
}

/// A number, or a string holding one.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// A JSON number as a whole number of 0 or more (track and disc numbers).
fn whole(value: Option<&Value>) -> Option<u32> {
    let n = value?.as_f64()?;
    (n.is_finite() && n >= 0.0 && n <= u32::MAX as f64).then_some(n as u32)
}

fn artists(map: &Map<String, Value>) -> String {
    match map.get("artists") {
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|artist| match artist {
                Value::String(name) => Some(name.as_str()),
                Value::Object(artist) => artist.get("name").and_then(Value::as_str),
                _ => None,
            })
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        _ => match present(map, "artists").or_else(|| present(map, "artist")) {
            Some(Value::Object(artist)) => text_of(artist.get("name")),
            other => text_of(other),
        },
    }
}

/// Words a catalogue puts where it has no album (`Unknown Album`, `n/a`, …).
fn placeholder_album(text: &str) -> bool {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return true;
    }
    if matches!(
        text.as_str(),
        "unknown" | "untitled" | "na" | "n/a" | "none" | "null" | "undefined"
    ) {
        return true;
    }
    ["unknown", "untitled"].iter().any(|word| {
        text.strip_prefix(word).is_some_and(|rest| {
            rest.starts_with(char::is_whitespace) && rest.trim_start() == "album"
        })
    })
}

/// The first album name that is not a placeholder (pkgs.dart:319-337).
fn album(map: &Map<String, Value>) -> Option<String> {
    let nested = match map.get("album") {
        Some(Value::String(name)) => Some(Value::String(name.clone())),
        Some(Value::Object(album)) => {
            first(album, &["name", "title", "album_name", "albumName"]).cloned()
        }
        _ => None,
    };
    let candidates = [
        map.get("album_name").cloned(),
        map.get("albumName").cloned(),
        map.get("album_title").cloned(),
        map.get("albumTitle").cloned(),
        nested,
    ];
    candidates
        .into_iter()
        .flatten()
        .find_map(|value| match value {
            Value::String(name) if !placeholder_album(&name) => Some(name.trim().to_string()),
            _ => None,
        })
}

/// The first four characters of `release_date`, when they are a number.
fn year(map: &Map<String, Value>) -> Option<String> {
    let date = text_of(present(map, "release_date"));
    let head: String = date.chars().take(4).collect();
    head.trim().parse::<i64>().ok().map(|year| year.to_string())
}

fn codec(map: &Map<String, Value>) -> Option<String> {
    let raw = first(
        map,
        &[
            "codec",
            "audio_codec",
            "audioCodec",
            "audio_format",
            "audioFormat",
            "format",
        ],
    )?;
    let text = text_of(Some(raw)).to_lowercase();
    let codec = if text.contains("flac") {
        "FLAC"
    } else if text.contains("alac") {
        "ALAC"
    } else if text.contains("opus") {
        "OPUS"
    } else if text.contains("vorbis") {
        "VORBIS"
    } else if text.contains("eac3") || text.contains("ec-3") {
        "E-AC-3"
    } else if text.contains("aac") || text.contains("mp4a") {
        "AAC"
    } else if text.contains("mp3") || text.contains("mpeg") {
        "MP3"
    } else {
        return None;
    };
    Some(codec.to_string())
}

fn bit_depth(map: &Map<String, Value>) -> Option<u32> {
    let raw = first(
        map,
        &[
            "bit_depth",
            "bitDepth",
            "bits_per_sample",
            "bitsPerSample",
            "maximum_bit_depth",
            "maxBitDepth",
        ],
    )?;
    let bits = match raw {
        Value::Number(n) => n.as_f64().map(|n| n.trunc() as i64),
        other => text_of(Some(other)).trim().parse::<i64>().ok(),
    }?;
    (1..=64).contains(&bits).then_some(bits as u32)
}

fn sample_rate(map: &Map<String, Value>) -> Option<u32> {
    let raw = first(
        map,
        &[
            "sample_rate",
            "sampleRate",
            "sampling_rate",
            "samplingRate",
            "maximum_sampling_rate",
            "maxSampleRate",
        ],
    )?;
    let value = match raw {
        Value::Number(n) => n.as_f64(),
        other => without_khz(&text_of(Some(other)))
            .trim()
            .parse::<f64>()
            .ok(),
    }?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    // 44.1 and 88.2 stay exact: kHz values are scaled before rounding.
    let hz = (if value <= 384.0 {
        value * 1000.0
    } else {
        value
    })
    .round();
    (8000.0..=768000.0).contains(&hz).then_some(hz as u32)
}

/// `"44.1 kHz"` → `"44.1"`: Lokal's `\s*k(?:hz)?` removed, any case.
fn without_khz(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::new();
    let mut rest = lower.as_str();
    while let Some(c) = rest.chars().next() {
        if c == 'k' {
            let trimmed = out.trim_end().len();
            out.truncate(trimmed);
            rest = &rest[1..];
            rest = rest.strip_prefix("hz").unwrap_or(rest);
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The largest cover the song names (pkgs.dart:481-563).
fn artwork(map: &Map<String, Value>) -> Option<String> {
    let album = map.get("album").and_then(Value::as_object);
    let mut candidates: Vec<Option<&Value>> = vec![map.get("images"), map.get("image")];
    if let Some(album) = album {
        candidates.extend([
            album.get("images"),
            album.get("image"),
            album.get("cover_art"),
            album.get("cover_url"),
        ]);
    }
    for key in [
        "artwork_url",
        "artworkUrl",
        "album_art",
        "albumArt",
        "cover_art",
        "coverArt",
        "cover_url",
        "coverUrl",
        "thumbnail",
    ] {
        candidates.push(map.get(key));
    }
    let mut best: Option<(String, f64)> = None;
    for value in candidates.into_iter().flatten() {
        if let Some(found) = image(value)
            && best.as_ref().is_none_or(|(_, size)| found.1 > *size)
        {
            best = Some(found);
        }
    }
    best.map(|(url, _)| url)
}

/// An image a value names, and how large it is (0 when unknown).
fn image(value: &Value) -> Option<(String, f64)> {
    match value {
        Value::String(url) if Url::parse(url).is_ok() => Some((url.clone(), size_in_url(url))),
        Value::Object(map) => {
            for field in [
                "url",
                "src",
                "cover_url",
                "coverUrl",
                "original",
                "large",
                "medium",
                "small",
            ] {
                if let Some(found) = map.get(field).and_then(image) {
                    let explicit = stated_size(map);
                    return Some((found.0, if explicit > 0.0 { explicit } else { found.1 }));
                }
            }
            None
        }
        Value::Array(items) => {
            let mut best: Option<(String, f64)> = None;
            for found in items.iter().filter_map(image) {
                if best.as_ref().is_none_or(|(_, size)| found.1 >= *size) {
                    best = Some(found);
                }
            }
            best
        }
        _ => None,
    }
}

/// `width * height`, or the sum of their squares when one is missing.
fn stated_size(map: &Map<String, Value>) -> f64 {
    let dimension = |key: &str| map.get(key).and_then(number).unwrap_or(0.0);
    let (width, height) = (dimension("width"), dimension("height"));
    if width > 0.0 && height > 0.0 {
        width * height
    } else {
        width * width + height * height
    }
}

/// The size an Amazon image's path names (`_SX1500_`, `_UY300_`, …), squared.
fn size_in_url(url: &str) -> f64 {
    let Ok(parsed) = Url::parse(url) else {
        return 0.0;
    };
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if !AMAZON_IMAGE_HOSTS.contains(&host.as_str()) {
        return 0.0;
    }
    let path = parsed.path();
    let mut largest: f64 = 0.0;
    let mut at = 0;
    while let Some(found) = path[at..].find('_') {
        let start = at + found + 1;
        at = start;
        let rest = &path[start..];
        if ["SX", "SY", "SL", "UX", "UY"]
            .iter()
            .any(|token| rest.starts_with(token))
        {
            let digits: String = rest[2..].chars().take_while(char::is_ascii_digit).collect();
            if let Ok(size) = digits.parse::<f64>() {
                largest = largest.max(size);
            }
        }
    }
    largest * largest
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_full_song_is_read() {
        let track = MTrack::of(
            &json!({
                "id": 123,
                "name": "Song",
                "artists": [{ "name": "A" }, "B", { "id": 3 }, ""],
                "album": { "title": "Record", "images": [{ "url": "https://img.example.com/s.jpg", "width": 64, "height": 64 }] },
                "duration_ms": 201000,
                "isrc": "USABC1234567",
                "track_number": 4,
                "discNumber": 1,
                "release_date": "2021-05-01",
                "audio_quality": "HI_RES_LOSSLESS",
                "audio_codec": "flac",
                "bitDepth": "24",
                "sample_rate": 96
            }),
            true,
        )
        .unwrap();
        assert_eq!(track.id, "123");
        assert_eq!(track.name, "Song");
        assert_eq!(track.artists, "A, B");
        assert_eq!(track.album.as_deref(), Some("Record"));
        assert_eq!(track.duration_ms, Some(201000));
        assert_eq!(track.isrc.as_deref(), Some("USABC1234567"));
        assert_eq!(track.track_number, Some(4));
        assert_eq!(track.disc_number, Some(1));
        assert_eq!(track.year.as_deref(), Some("2021"));
        assert_eq!(track.quality.as_deref(), Some("HI_RES_LOSSLESS"));
        assert_eq!(track.codec.as_deref(), Some("FLAC"));
        assert_eq!(track.bit_depth, Some(24));
        assert_eq!(track.sample_rate, Some(96000));
        assert_eq!(
            track.artwork.as_deref(),
            Some("https://img.example.com/s.jpg")
        );
        assert!(!track.atmos);
        assert_eq!(track.raw["id"], 123);
    }

    #[test]
    fn fields_fall_back() {
        let track = MTrack::of(
            &json!({ "id": "x", "title": "Titled", "artist": "Solo", "albumName": "Unknown Album",
                     "album": "Real One", "durationMs": 5000, "sampleRate": "44.1 kHz", "format": "mp4a.40.2" }),
            true,
        )
        .unwrap();
        assert_eq!(track.name, "Titled");
        assert_eq!(track.artists, "Solo");
        assert_eq!(track.album.as_deref(), Some("Real One"));
        assert_eq!(track.duration_ms, Some(5000));
        assert_eq!(track.sample_rate, Some(44100));
        assert_eq!(track.codec.as_deref(), Some("AAC"));
        assert_eq!(track.year, None);
    }

    #[test]
    fn rows_without_what_they_need_are_dropped() {
        assert!(MTrack::of(&json!({ "name": "no id" }), true).is_none());
        assert!(MTrack::of(&json!({ "id": null, "name": "x" }), true).is_none());
        assert!(MTrack::of(&json!({ "id": "1" }), true).is_none());
        assert!(MTrack::of(&json!({ "id": "1" }), false).is_some());
        assert!(
            MTrack::of(
                &json!({ "id": "1", "name": "x", "item_type": "album" }),
                true
            )
            .is_none()
        );
        assert!(
            MTrack::of(
                &json!({ "id": "1", "name": "x", "item_type": "track" }),
                true
            )
            .is_some()
        );
        assert!(MTrack::of(&json!("1"), false).is_none());
    }

    #[test]
    fn names_are_cut_and_odd_values_left_out() {
        let track = MTrack::of(
            &json!({ "id": "1", "name": "n".repeat(600), "bit_depth": 128, "sample_rate": 2,
                     "duration_ms": -5, "track_number": -1, "isrc": "", "album": { "name": "n/a", "title": "Later" } }),
            true,
        )
        .unwrap();
        assert_eq!(track.name.chars().count(), 500);
        assert_eq!(track.bit_depth, None);
        assert_eq!(track.sample_rate, None);
        assert_eq!(track.duration_ms, None);
        assert_eq!(track.track_number, None);
        assert_eq!(track.isrc, None);
        // The album's first name is a placeholder: Lokal does not look further inside it.
        assert_eq!(track.album, None);
    }

    #[test]
    fn the_largest_cover_wins() {
        let track = MTrack::of(
            &json!({
                "id": "1", "name": "x",
                "thumbnail": "https://img.example.com/thumb.jpg",
                "images": [
                    { "url": "https://img.example.com/300.jpg", "width": 300, "height": 300 },
                    { "url": "https://img.example.com/640.jpg", "width": 640, "height": 640 },
                    { "url": "https://img.example.com/64.jpg", "width": 64, "height": 64 }
                ]
            }),
            true,
        )
        .unwrap();
        assert_eq!(
            track.artwork.as_deref(),
            Some("https://img.example.com/640.jpg")
        );

        let amazon = MTrack::of(
            &json!({
                "id": "1", "name": "x",
                "image": "https://m.media-amazon.com/images/I/abc._SX120_.jpg",
                "album": { "image": "https://m.media-amazon.com/images/I/abc._SX1500_.jpg" }
            }),
            true,
        )
        .unwrap();
        assert_eq!(
            amazon.artwork.as_deref(),
            Some("https://m.media-amazon.com/images/I/abc._SX1500_.jpg")
        );

        // Without sizes, the first one named is kept; a value without a scheme is not a cover.
        let plain = MTrack::of(
            &json!({ "id": "1", "name": "x", "cover_url": "//cdn.example.com/a.jpg",
                     "artwork_url": "https://cdn.example.com/b.jpg", "coverUrl": "https://cdn.example.com/c.jpg" }),
            true,
        )
        .unwrap();
        assert_eq!(
            plain.artwork.as_deref(),
            Some("https://cdn.example.com/b.jpg")
        );
    }

    #[test]
    fn atmos_is_read_from_the_song_alone() {
        assert_eq!(
            atmos_mode(&json!({ "audio_modes": ["STEREO", "DOLBY_ATMOS"] })),
            Some("DOLBY_ATMOS")
        );
        assert_eq!(
            atmos_mode(&json!({ "audioQualities": "LOSSLESS, ac-4 atmos" })),
            Some("AC4_ATMOS")
        );
        assert_eq!(
            atmos_mode(&json!({ "audio_mode": "eac3 joc|atmos" })),
            Some("EAC3_JOC")
        );
        assert_eq!(
            atmos_mode(&json!({ "audio_quality": "HI_RES_LOSSLESS" })),
            None
        );
        assert_eq!(atmos_mode(&json!({ "codec": "ac4" })), None);
        let track = MTrack::of(
            &json!({ "id": "1", "name": "x", "audioModes": ["ATMOS"] }),
            true,
        )
        .unwrap();
        assert!(track.atmos);
    }

    #[test]
    fn rows_take_every_shape() {
        let song = |id: &str| json!({ "id": id, "name": "s" });
        assert_eq!(rows(&json!([song("a"), song("b")])).len(), 2);
        assert_eq!(rows(&json!({ "tracks": [song("a")] }))[0].id, "a");
        assert_eq!(
            rows(&json!({ "album": { "name": "r", "tracks": [song("b")] } }))[0].id,
            "b"
        );
        assert_eq!(
            rows(&json!({ "artist": { "top_tracks": [song("c")] } }))[0].id,
            "c"
        );
        assert_eq!(rows(&json!({ "track": song("d") }))[0].id, "d");
        assert!(rows(&json!({ "success": false, "error": "nope" })).is_empty());
        assert!(rows(&json!(null)).is_empty());
        assert!(rows(&json!({ "tracks": { "items": [] } })).is_empty());
        // Rows that are not songs are left out.
        let mixed = json!([song("a"), { "id": "z", "name": "Album", "item_type": "album" }, { "id": "y" }, 5]);
        assert_eq!(rows(&mixed).len(), 1);
    }
}
