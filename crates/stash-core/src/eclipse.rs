//! Reading add-ons made for Eclipse and its family (BitChord, LastWave): the add-ons that were there before Stash's
//! own contract, and that many listeners already have.
//!
//! An Eclipse add-on answers the same routes with looser shapes: an id may be a number, a length is in seconds, a
//! stream says "320kbps" or "LOSSLESS" where Stash says a tier. Everything here turns one of its answers into the
//! contract's own (a [`Manifest`], [`Track`]s, a [`StreamInfo`]) by the contract's mapping (section 10.2), so the
//! rest of the host deals with one kind of add-on. What cannot be read is left out, never guessed at; a stream is
//! never called better than it says it is.

use std::collections::HashSet;

use serde_json::Value;

use crate::addons::{
    AddonError, Artwork, Code, Delivery, Dialect, Manifest, StreamInfo, StreamRequest, Tier, Track,
    Wanted, checked_url,
};
use crate::identity::{clean_text, isrc};

const ID: usize = 200;
const TEXT: usize = 300;
const MAX_TRACKS: usize = 200;

fn text(value: Option<&Value>, cap: usize) -> Option<String> {
    let cleaned = clean_text(value?.as_str()?, cap);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// An id: text, or a whole number read as its digits.
fn id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(_) => text(value, ID),
        Value::Number(n) => n.as_u64().map(|n| n.to_string()).or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(f))
                .map(|f| format!("{f:.0}"))
        }),
        _ => None,
    }
}

/// A number, or text that is one.
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// A whole number within `range`, from a number or numeric text; outside it, nothing.
fn within(value: Option<&Value>, range: std::ops::RangeInclusive<f64>) -> Option<u64> {
    number(value)
        .map(f64::round)
        .filter(|n| range.contains(n))
        .map(|n| n as u64)
}

/// A flag as BitChord reads one (its `asFlag`): true or false; a number other than 0; the words 1, true, yes, on and
/// 0, false, no, off. Anything else (nothing, null, an object, another word) says nothing, and the flag's own default
/// holds.
fn flag(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => Some(n.as_f64() != Some(0.0)),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            other => other.parse::<f64>().ok().map(|n| n != 0.0),
        },
        _ => None,
    }
}

/// Whether a stream is marked as protected: any value but nothing, null, a "no" ([`flag`]) or the words none and
/// null. A word Stash does not know is taken as a yes: a protected stream played as if it were not is only noise.
fn protected(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::String(s))
            if matches!(s.trim().to_ascii_lowercase().as_str(), "" | "none" | "null") =>
        {
            false
        }
        Some(other) => flag(Some(other)) != Some(false),
    }
}

/// What is said of an add-on that asks for BitChord's app verification.
pub(crate) const ONLY_BITCHORD: &str = "this add-on asks for BitChord's app verification, which only the BitChord app on Android can give (its maker may have a link that works without it)";

/// The most settings whose defaults are sent back to an add-on, and the longest of their names and values.
const MAX_SENT: usize = 32;
const SENT_NAME: usize = 64;
const SENT_VALUE: usize = 200;

/// Whether a manifest without `stash` is an Eclipse add-on's: it has an id, and offers search or streams.
pub fn is_manifest(value: &Value) -> bool {
    // No list of what it offers, or an empty one, is given the benefit of the doubt, as BitChord gives it.
    let offers = match value.get("resources") {
        None | Some(Value::Null) => true,
        Some(Value::Array(listed)) => {
            listed.is_empty()
                || listed
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|r| matches!(r.trim().to_ascii_lowercase().as_str(), "search" | "stream"))
        }
        Some(_) => false,
    };
    value.get("stash").is_none() && id(value.get("id")).is_some() && offers
}

/// An Eclipse add-on's id as one the contract takes: lower-case letters, digits, dots and hyphens, at most 64,
/// beginning with a letter or a digit. Anything else in it becomes a hyphen (Eclipse and BitChord take any id).
fn contract_id(given: &str) -> Option<String> {
    let mut made = String::new();
    for c in given.trim().to_lowercase().chars() {
        let fits = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-';
        let c = if fits { c } else { '-' };
        // No run of hyphens, and none at the start.
        if c == '-' && (made.is_empty() || made.ends_with('-')) {
            continue;
        }
        made.push(c);
        if made.len() == 64 {
            break;
        }
    }
    let made = made.trim_matches(|c| c == '-' || c == '.').to_string();
    (!made.is_empty()).then_some(made)
}

/// The defaults of the settings an add-on declares, as they are sent back with every request (Eclipse and BitChord
/// send each declared setting as a query parameter; an add-on may rely on getting them). The quality is sent apart,
/// and the request's own words are never overridden.
fn sent(value: &Value) -> Vec<(String, String)> {
    let mut sent: Vec<(String, String)> = Vec::new();
    for setting in value
        .get("settings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(key) = setting.get("key").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        let named = (1..=SENT_NAME).contains(&key.len())
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
        if !named
            || matches!(
                key,
                "q" | "quality" | "title" | "artist" | "isrc" | "durationMs" | "limit"
            )
            || sent.iter().any(|(known, _)| known == key)
        {
            continue;
        }
        let default = match setting.get("default") {
            Some(Value::String(s)) => clean_text(s, SENT_VALUE),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Bool(b)) => b.to_string(),
            _ => continue,
        };
        sent.push((key.to_string(), default));
        if sent.len() == MAX_SENT {
            break;
        }
    }
    sent
}

/// An Eclipse add-on's manifest as the contract's. Only the id is required: without a name the id is the name.
pub fn manifest(value: &Value) -> Result<Manifest, AddonError> {
    if flag(value.get("bitchordAuth")) == Some(true) {
        return Err(AddonError::host(Code::Refused, ONLY_BITCHORD));
    }
    let id = id(value.get("id"))
        .and_then(|id| contract_id(&id))
        .ok_or_else(|| AddonError::host(Code::InvalidResponse, "the add-on has no id"))?;
    let listed: Vec<String> = value
        .get("resources")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .filter_map(Value::as_str)
                .map(|r| r.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let lists = |name: &str| listed.iter().any(|r| r == name);
    let mut resources: Vec<String> = Vec::new();
    for known in ["search", "stream", "resolve"] {
        if lists(known) {
            resources.push(known.to_string());
        }
    }
    // One that lists nothing is taken to search and to play, as BitChord takes it.
    if listed.is_empty() {
        resources = vec!["search".to_string(), "stream".to_string()];
    }
    // An add-on that finds songs only by ISRC is asked that way, and only for songs that have one.
    let resolve_by_isrc = lists("isrc") && !lists("resolve");
    if resolve_by_isrc {
        resources.push("resolve".to_string());
    }
    // The add-on's own word for the quality to ask for: the first choice of its quality setting, when it has one.
    let quality = value
        .get("settings")
        .and_then(Value::as_array)
        .and_then(|settings| {
            // By its name alone: BitChord does not read a setting's `type`.
            settings
                .iter()
                .find(|s| s.get("key").and_then(Value::as_str) == Some("quality"))
        })
        .and_then(|select| {
            select
                .get("options")?
                .as_array()?
                .iter()
                .find_map(|o| o.get("value")?.as_str())
        })
        .filter(|word| (1..=100).contains(&word.chars().count()))
        .map(str::to_string);
    // `allowDownloads` given and not a yes ("no" and "off", an empty word, anything Stash cannot read as a yes):
    // never a download source, as its maker may have meant. Absent or null allows downloads.
    let no_downloads = value
        .get("allowDownloads")
        .is_some_and(|v| !v.is_null() && flag(Some(v)) != Some(true));
    Ok(Manifest {
        no_downloads,
        sent: sent(value),
        stash: 1,
        name: text(value.get("name"), 60).unwrap_or_else(|| id.clone()),
        version: text(value.get("version"), 40).unwrap_or_else(|| "0.0.0".to_string()),
        kind: "server".to_string(),
        description: text(value.get("description"), TEXT),
        resources,
        dialect: Dialect::Eclipse,
        resolve_by_isrc,
        quality,
        id,
        reach: None,
    })
}

/// A track from a search row or a resolve answer. A row without an id, a title or an artist cannot be read.
pub fn track(value: &Value) -> Option<Track> {
    // A length in milliseconds wins when it is given; one outside a day is dropped, not replaced by the seconds.
    let duration_ms = match value.get("durationMs").filter(|d| !d.is_null()) {
        Some(ms) => within(Some(ms), 1.0..=86_400_000.0),
        None => number(value.get("duration"))
            .map(|seconds| (seconds * 1000.0).round())
            .filter(|ms| (1.0..=86_400_000.0).contains(ms))
            .map(|ms| ms as u64),
    };
    Some(Track {
        id: id(value.get("id"))?,
        title: text(value.get("title"), TEXT)?,
        artist: text(value.get("artist"), TEXT)?,
        album: text(value.get("album"), TEXT),
        duration_ms,
        isrc: value
            .get("isrc")
            .and_then(Value::as_str)
            .and_then(|code| isrc(code.trim())),
        explicit: value.get("explicit").and_then(Value::as_bool),
        max_tier: None,
        artwork: ["artworkURL", "artworkUrl", "albumArtworkURL"]
            .iter()
            .filter_map(|name| value.get(name)?.as_str())
            .filter(|url| checked_url(url, "the cover").is_ok())
            .take(1)
            .map(|url| Artwork {
                url: url.to_string(),
            })
            .collect(),
    })
}

/// The tracks of a search answer. Rows that cannot be read, or that repeat an id, are dropped and the rest kept. An
/// answer with no result list at all is no answer: the add-on is not working, which is not the same as finding
/// nothing.
pub fn search(value: &Value) -> Result<Vec<Track>, AddonError> {
    let list = |name: &str| value.get(name).and_then(Value::as_array);
    match list("tracks") {
        Some(rows) => {
            let mut seen = HashSet::new();
            Ok(rows
                .iter()
                .filter_map(track)
                .filter(|t| seen.insert(t.id.clone()))
                .take(MAX_TRACKS)
                .collect())
        }
        None if ["albums", "artists", "playlists"]
            .iter()
            .any(|other| list(other).is_some()) =>
        {
            Ok(Vec::new())
        }
        None => Err(AddonError::host(
            Code::Unavailable,
            "the add-on's search gave no answer",
        )),
    }
}

/// The track of a resolve answer (`{ item }`); an item that cannot be read is no match.
pub fn resolved(value: &Value) -> Option<Track> {
    track(value.get("item")?)
}

/// The track of an answer to "which of your tracks has this ISRC" (`{ trackId }` or `{ id }`), as the song asked for.
pub fn resolved_by_isrc(value: &Value, wanted: &Wanted) -> Option<Track> {
    Some(Track {
        id: id(value.get("trackId")).or_else(|| id(value.get("id")))?,
        title: wanted.title.clone(),
        artist: wanted.artist.clone(),
        album: None,
        duration_ms: None,
        isrc: wanted.isrc.clone(),
        explicit: None,
        max_tier: None,
        artwork: Vec::new(),
    })
}

/// A stream answer as the contract's, and whether it said how good it is.
#[derive(Debug)]
pub struct Streamed {
    pub info: StreamInfo,
    /// False when the answer named no lossless codec, no bitrate and no quality: its tier is then `low` for want of
    /// better, which says nothing about its size.
    pub stated: bool,
}

impl Streamed {
    /// Whether the stream is one that was asked for. A stream that states no tier is judged as `high`: it passes
    /// where lossy is accepted, and not where only lossless is.
    pub fn asked_for(&self, want: &StreamRequest) -> bool {
        let tier = if self.stated {
            self.info.tier
        } else {
            Tier::High
        };
        want.min_tier <= tier && tier <= want.tier && want.deliveries.contains(&self.info.manifest)
    }
}

/// The codec an Eclipse word names, in the contract's word; `None` for a word that names none.
fn codec(word: &str) -> Option<&'static str> {
    let word = word.trim().to_ascii_lowercase();
    Some(match word.as_str() {
        "flac" => "flac",
        "alac" => "alac",
        "pcm" | "wav" => "pcm",
        "aac" | "m4a" => "aac",
        "he-aac" | "heaac" => "he-aac",
        "mp3" => "mp3",
        "opus" => "opus",
        "vorbis" => "vorbis",
        "eac3_joc" | "eac3" | "ec-3" => "eac3_joc",
        other if other.starts_with("mp4a") => "aac",
        _ => return None,
    })
}

/// How a stream is delivered, from the words add-ons use for it: a transport's name, a file ending or a media type.
fn delivery(word: &str) -> Option<Delivery> {
    Some(match word {
        "dash" | "mpd" | "application/dash+xml" => Delivery::Dash,
        "hls" | "m3u8" | "application/x-mpegurl" | "application/vnd.apple.mpegurl" => Delivery::Hls,
        "none" | "file" | "progressive" | "http" | "https" => Delivery::None,
        _ => return None,
    })
}

/// The codec a media type names ("audio/flac"), when it names one for certain.
fn codec_of_type(media_type: &str) -> Option<&'static str> {
    let kind = media_type.split(';').next()?.trim();
    Some(match kind.strip_prefix("audio/")? {
        "flac" | "x-flac" => "flac",
        "mpeg" | "mp3" => "mp3",
        "wav" | "x-wav" | "wave" => "pcm",
        "opus" => "opus",
        "aac" | "aacp" => "aac",
        _ => return None,
    })
}

/// What an add-on's free words say of a stream's quality ("Tidal · Hi-Res FLAC (DASH)", "HI_RES_LOSSLESS",
/// "24-bit / 96 kHz"): whether they name it lossless, whether they call it hi-res, and the bit depth and sample rate
/// they give. Add-ons for Tidal and Qobuz often say how good a stream is nowhere else; BitChord reads the same words.
#[derive(Debug, Default, PartialEq)]
struct Label {
    lossless: bool,
    hi_res: bool,
    bit_depth: Option<u32>,
    sample_rate: Option<u32>,
    kbps: Option<f64>,
}

fn label(words: &[String]) -> Label {
    static BITS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\b(16|24|32)[- ]?bits?\b").unwrap());
    static KHZ: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\b(\d{2,3}(?:\.\d+)?)\s*khz\b").unwrap());
    static KBPS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\b(\d{2,4})\s*kbps\b").unwrap());
    static LOSSLESS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(^|[^a-z])(lossless|flac|alac|wav)([^a-z]|$)").unwrap()
    });
    static HI_RES: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(^|[^a-z])hi[-_ ]?res([^a-z]|$)").unwrap());
    let mut said = Label::default();
    for word in words {
        let word = word.to_ascii_lowercase();
        if let Some(bits) = BITS.captures(&word).and_then(|c| c[1].parse::<u32>().ok()) {
            said.bit_depth.get_or_insert(bits);
            // A depth is only said of a lossless stream.
            said.lossless = true;
        }
        if let Some(khz) = KHZ.captures(&word).and_then(|c| c[1].parse::<f64>().ok()) {
            said.sample_rate
                .get_or_insert((khz * 1000.0).round() as u32);
        }
        if let Some(kbps) = KBPS.captures(&word).and_then(|c| c[1].parse::<f64>().ok()) {
            said.kbps.get_or_insert(kbps);
        }
        said.lossless |= LOSSLESS.is_match(&word);
        said.hi_res |= HI_RES.is_match(&word);
    }
    // "Hi-Res" alone says lossless too; a bitrate in kbps says lossy whatever else the words hold.
    said.lossless = (said.lossless || said.hi_res) && said.kbps.is_none();
    said
}

/// Reads a stream answer. `Err` with [`Code::NotFound`] for an answer that is an error in a 200 (the add-on does not
/// have the track; with the add-on's own words when it gave any), with [`Code::Unavailable`] when that error asks to
/// be asked again later, and with [`Code::QualityUnavailable`] for a stream this host does not play (protected).
pub fn stream(value: &Value) -> Result<Streamed, AddonError> {
    let Some(url) = value.get("url").and_then(Value::as_str) else {
        let Some(error) = value.get("error") else {
            return Err(AddonError::host(
                Code::InvalidResponse,
                "the add-on answered a stream without an address",
            ));
        };
        // "Not yet" (still preparing it, too busy): asked again later, never taken for "it is not there".
        let wait = [error.get("retryAfter"), value.get("retryAfter")]
            .into_iter()
            .find_map(|seconds| within(seconds, 1.0..=86_400.0));
        // The add-on's own words (often what the listener has to set right).
        let words = text(Some(error), 200)
            .or_else(|| text(error.get("message"), 200))
            .unwrap_or_default();
        return Err(match wait {
            Some(seconds) => AddonError {
                retry_after: Some(seconds as u32),
                ..AddonError::host(Code::Unavailable, words)
            },
            None => AddonError::host(Code::NotFound, words),
        });
    };
    // Protected, or said to be video. (A `video` object beside the address describes a video that goes with the
    // sound, which Eclipse plays apart: the address is still the sound.)
    if ["encrypted", "drm", "hybrid"]
        .iter()
        .any(|mark| protected(value.get(mark)))
        || flag(value.get("video")) == Some(true)
    {
        return Err(AddonError::host(
            Code::QualityUnavailable,
            "the stream is protected or is not sound alone",
        ));
    }
    checked_url(url, "the stream URL")?;

    let word = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .map(|w| w.trim().to_ascii_lowercase())
    };
    let format = word("format");
    // What the add-on says of the quality in its own words, wherever it says it.
    let said_freely = label(
        &["quality", "streamQuality", "audioQuality", "format"]
            .iter()
            .filter_map(|name| text(value.get(name), 120))
            .collect::<Vec<_>>(),
    );
    let named = word("codec")
        .as_deref()
        .and_then(codec)
        .or_else(|| word("fileCodec").as_deref().and_then(codec))
        .or_else(|| {
            // `format` names the codec only when `codec` named none, and only with a codec's word.
            format
                .as_deref()
                .filter(|f| matches!(*f, "mp3" | "flac" | "aac" | "m4a"))
                .and_then(codec)
        })
        .or_else(|| {
            // A container that is its codec.
            ["container", "containerFormat"]
                .iter()
                .filter_map(|name| word(name))
                .find_map(|c| match c.as_str() {
                    "flac" => Some("flac"),
                    "mp3" => Some("mp3"),
                    "wav" | "aiff" => Some("pcm"),
                    _ => None,
                })
        })
        .or_else(|| word("mimeType").as_deref().and_then(codec_of_type))
        // Named nowhere, but called lossless in the add-on's words: FLAC, as BitChord reads it.
        .or_else(|| said_freely.lossless.then_some("flac"));
    let manifest = ["manifest", "mediaType", "format"]
        .iter()
        .filter_map(|name| word(name))
        .find_map(|w| delivery(&w))
        .unwrap_or(Delivery::None);
    let container = ["container", "containerFormat"]
        .iter()
        .filter_map(|name| word(name))
        .find_map(|c| {
            Some(match c.as_str() {
                "flac" | "mp3" | "mp4" | "fmp4" | "ogg" | "webm" | "wav" => c,
                "m4a" => "mp4".to_string(),
                _ => return None,
            })
        });
    let bit_depth = within(value.get("bitDepth"), 1.0..=64.0)
        .map(|b| b as u32)
        .or(said_freely.bit_depth);
    // A rate under 1,000 is in kHz ("96", "44.1").
    let sample_rate = number(value.get("sampleRate"))
        .map(|r| if r < 1_000.0 { r * 1_000.0 } else { r })
        .map(f64::round)
        .filter(|r| (1.0..=1_000_000.0).contains(r))
        .map(|r| r as u32)
        .or(said_freely.sample_rate);
    // A bitrate under 10,000 is in kbps.
    let bitrate = number(value.get("bitrate"))
        .map(|b| if b < 10_000.0 { b * 1000.0 } else { b })
        .map(f64::round)
        .filter(|b| (1.0..=100_000_000.0).contains(b))
        .map(|b| b as u64);
    // A time past the year 5000 in seconds is a time in milliseconds.
    let expires_at = number(value.get("expiresAt"))
        .map(|at| {
            if at > 100_000_000_000.0 {
                at / 1000.0
            } else {
                at
            }
        })
        .map(f64::round)
        .filter(|at| (1.0..=4_102_444_800.0).contains(at))
        .map(|at| at as i64);
    let quality = text(value.get("quality"), 40);

    // The tier: from a lossless codec and its format; else from what the add-on calls the quality; else from the
    // bitrate; else low, unstated.
    let lossless = matches!(named, Some("flac" | "alac" | "pcm"));
    let said = quality
        .as_deref()
        .map(str::to_ascii_lowercase)
        .and_then(|q| {
            let q = q.trim();
            if q == "lossless" {
                // A lossy codec outranks the word.
                return named.is_none().then_some(Tier::Lossless);
            }
            if let Some(kbps) = q
                .strip_suffix("kbps")
                .and_then(|n| n.trim().parse::<f64>().ok())
            {
                return Some(if kbps >= 256.0 { Tier::High } else { Tier::Low });
            }
            match q {
                "high" => Some(Tier::High),
                "low" => Some(Tier::Low),
                _ => None,
            }
        })
        .or_else(|| {
            said_freely
                .kbps
                .map(|kbps| if kbps >= 256.0 { Tier::High } else { Tier::Low })
        });
    let (tier, stated) = if lossless {
        let above_cd = bit_depth.is_some_and(|b| b >= 24)
            || sample_rate.is_some_and(|r| r > 48_000)
            // Called hi-res, with no numbers to say otherwise.
            || (said_freely.hi_res && bit_depth.is_none() && sample_rate.is_none());
        (
            if above_cd {
                Tier::Hires
            } else {
                Tier::Lossless
            },
            true,
        )
    } else if let Some(tier) = said {
        (tier, true)
    } else if let Some(bitrate) = bitrate {
        (
            if bitrate >= 256_000 {
                Tier::High
            } else {
                Tier::Low
            },
            true,
        )
    } else {
        (Tier::Low, false)
    };

    Ok(Streamed {
        info: StreamInfo {
            url: url.to_string(),
            manifest,
            tier,
            codec: named.map(str::to_string),
            container,
            bit_depth,
            sample_rate,
            bitrate,
            duration_ms: None,
            expires_at,
            encrypted: None,
        },
        stated,
    })
}

/// The word sent as `quality` with a stream request: the add-on's own first choice when it has a quality setting,
/// else Eclipse's words for what the host wants.
pub fn quality(manifest: &Manifest, want: &StreamRequest) -> String {
    manifest.quality.clone().unwrap_or_else(|| {
        match want.tier {
            Tier::Hires | Tier::Lossless => "LOSSLESS",
            Tier::High => "HIGH",
            Tier::Low => "LOW",
        }
        .to_string()
    })
}

/// What an answer that is not a success means. An Eclipse error is read by its status: its body has no fixed shape.
pub fn error(
    status: u16,
    retry_after_header: Option<u64>,
    www_authenticate: Option<&str>,
    body: &str,
) -> AddonError {
    let body: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let seconds = |value: Option<&Value>| {
        number(value)
            .map(f64::trunc)
            .filter(|s| (1.0..=86_400.0).contains(s))
            .map(|s| s as u32)
    };
    // How long the add-on asks to be left alone: the contract's own place for it, else the header, else the top of
    // the body.
    let retry_after = seconds(body.get("error").and_then(|e| e.get("retryAfter")))
        .or_else(|| {
            retry_after_header
                .filter(|s| (1..=86_400).contains(s))
                .map(|s| s as u32)
        })
        .or_else(|| seconds(body.get("retryAfter")));
    if status == 401
        && www_authenticate.is_some_and(|scheme| {
            scheme
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("bitchord")
        })
    {
        return AddonError::host(Code::Refused, ONLY_BITCHORD);
    }
    let code = match status {
        404 => Code::NotFound,
        429 => Code::RateLimited,
        401 => Code::Unauthorized,
        300..=399 => Code::InvalidResponse,
        400..=499 => Code::BadRequest,
        _ => Code::Unavailable,
    };
    AddonError {
        code,
        message: if (300..400).contains(&status) {
            format!("the add-on redirected ({status}); redirects are not followed")
        } else {
            String::new()
        },
        retry_after,
        who: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn songs_are_saved_from_an_eclipse_add_on_unless_it_says_no() {
        let no = |allow: Option<serde_json::Value>| {
            let mut m = json!({ "id": "a.b", "resources": ["search", "stream"] });
            if let Some(allow) = allow {
                m["allowDownloads"] = allow;
            }
            manifest(&m).unwrap().no_downloads
        };
        assert!(!no(None));
        assert!(!no(Some(json!(null))));
        assert!(!no(Some(json!(true))));
        assert!(!no(Some(json!("yes"))));
        assert!(no(Some(json!(false))));
        assert!(no(Some(json!(0))));
        assert!(no(Some(json!("false"))));
        assert!(no(Some(json!("null"))));
        assert!(no(Some(json!(""))));
        // As BitChord reads the flag: "no" and "off" are a no, "on" and 1 a yes.
        assert!(no(Some(json!("no"))) && no(Some(json!("Off"))));
        assert!(!no(Some(json!("on"))) && !no(Some(json!(1))) && !no(Some(json!("TRUE"))));
    }

    #[test]
    fn an_eclipse_manifest_is_read_as_the_contracts() {
        let read = manifest(&json!({
            "id": "Com.Example.Music", "name": "  Example\tMusic ", "resources": ["search", "stream", "isrc", "catalog", "settings"],
            "settings": [{ "key": "quality", "type": "select", "options": [{ "value": "hi_res" }, { "value": "cd" }] }]
        }))
        .unwrap();
        assert_eq!(
            (
                read.id.as_str(),
                read.name.as_str(),
                read.version.as_str(),
                read.kind.as_str()
            ),
            ("com.example.music", "Example Music", "0.0.0", "server")
        );
        // Finding a song by its ISRC counts as resolve; what the host has no use for is left out.
        assert_eq!(read.resources, ["search", "stream", "resolve"]);
        assert!(read.resolve_by_isrc && read.dialect == Dialect::Eclipse);
        assert_eq!(read.quality.as_deref(), Some("hi_res"));

        assert!(is_manifest(
            &json!({ "id": "a.b", "resources": ["stream"] })
        ));
        assert!(!is_manifest(
            &json!({ "stash": 1, "id": "a.b", "resources": ["stream"] })
        ));
        assert!(!is_manifest(
            &json!({ "id": "a.b", "resources": ["lyrics"] })
        ));
        // An add-on made for BitChord alone is refused in words; one that says it is not, is not.
        let only = manifest(&json!({ "id": "a.b", "resources": ["stream"], "bitchordAuth": true }))
            .unwrap_err();
        assert_eq!(
            (only.code, only.in_words().as_str()),
            (Code::Refused, ONLY_BITCHORD)
        );
        assert!(
            manifest(&json!({ "id": "a.b", "resources": ["stream"], "bitchordAuth": "false" }))
                .is_ok()
        );
    }

    #[test]
    fn search_rows_are_read_loosely_and_never_guessed() {
        let found = search(&json!({ "tracks": [
            { "id": 125, "title": " Song\tTitle ", "artist": "Artist", "album": "", "duration": "240.5", "isrc": " usrc12345678 ", "explicit": "yes" },
            { "id": "b", "title": "Zero length", "artist": "Artist", "durationMs": 0, "duration": 200, "explicit": true },
            { "id": "b", "title": "The same id again", "artist": "Artist" },
            { "id": "c", "title": "No artist" },
            { "title": "No id", "artist": "Artist" }
        ], "albums": [] }))
        .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(
            (
                found[0].id.as_str(),
                found[0].title.as_str(),
                found[0].album.as_deref(),
                found[0].duration_ms,
                found[0].isrc.as_deref(),
                found[0].explicit
            ),
            (
                "125",
                "Song Title",
                None,
                Some(240_500),
                Some("USRC12345678"),
                None
            )
        );
        // A length in milliseconds that cannot be right is dropped, not replaced by the seconds beside it.
        assert_eq!(
            (found[1].duration_ms, found[1].explicit),
            (None, Some(true))
        );
        // Finding nothing is an answer; saying nothing is not.
        assert_eq!(search(&json!({ "albums": [] })).unwrap(), []);
        assert_eq!(
            search(&json!({ "error": "busy" })).unwrap_err().code,
            Code::Unavailable
        );
    }

    #[test]
    fn a_stream_is_never_called_better_than_it_says() {
        let tier = |answer: Value| {
            stream(&answer).map(|s| (s.info.tier, s.stated, s.info.codec, s.info.manifest))
        };
        let url = "https://media.example.org/a";
        assert_eq!(tier(json!({ "url": url, "codec": "FLAC", "bitDepth": "24", "sampleRate": 96000, "format": "dash" })).unwrap(), (Tier::Hires, true, Some("flac".into()), Delivery::Dash));
        assert_eq!(
            tier(json!({ "url": url, "codec": "wav", "quality": "CD quality" })).unwrap(),
            (Tier::Lossless, true, Some("pcm".into()), Delivery::None)
        );
        assert_eq!(
            tier(json!({ "url": url, "format": "mp3", "quality": "320kbps" })).unwrap(),
            (Tier::High, true, Some("mp3".into()), Delivery::None)
        );
        assert_eq!(
            tier(json!({ "url": url, "codec": "m4a", "bitrate": 128 })).unwrap(),
            (Tier::Low, true, Some("aac".into()), Delivery::None)
        );
        // "lossless" beside a lossy codec is not lossless.
        assert_eq!(
            tier(json!({ "url": url, "codec": "aac", "bitrate": 256, "quality": "lossless" }))
                .unwrap()
                .0,
            Tier::High
        );
        assert_eq!(
            tier(json!({ "url": url, "quality": "LOSSLESS" }))
                .unwrap()
                .0,
            Tier::Lossless
        );
        // Nothing said: low, and marked as unstated.
        let unstated = stream(&json!({ "url": url, "codec": "mp3" })).unwrap();
        assert_eq!((unstated.info.tier, unstated.stated), (Tier::Low, false));
        let lossless_only = StreamRequest {
            tier: Tier::Hires,
            min_tier: Tier::Lossless,
            deliveries: vec![Delivery::None],
        };
        assert!(
            unstated.asked_for(&StreamRequest::default()) && !unstated.asked_for(&lossless_only)
        );
        // What this host does not play, and what is no stream at all.
        assert_eq!(
            stream(&json!({ "url": url, "codec": "flac", "encrypted": "cenc" }))
                .unwrap_err()
                .code,
            Code::QualityUnavailable
        );
        assert_eq!(
            stream(&json!({ "url": url, "video": true }))
                .unwrap_err()
                .code,
            Code::QualityUnavailable
        );
        assert_eq!(
            stream(&json!({ "error": "Track not found" }))
                .unwrap_err()
                .code,
            Code::NotFound
        );
        assert_eq!(
            stream(&json!({ "url": "http://plain.example.org/a" }))
                .unwrap_err()
                .code,
            Code::InvalidResponse
        );
    }

    #[test]
    fn an_error_is_read_by_its_status() {
        assert_eq!(error(404, None, None, "").code, Code::NotFound);
        assert_eq!(error(400, None, None, "{}").code, Code::BadRequest);
        let busy = error(503, Some(10), None, r#"{"error":"preparing"}"#);
        assert_eq!((busy.code, busy.retry_after), (Code::Unavailable, Some(10)));
        assert_eq!(
            error(503, None, None, r#"{"retryAfter":"20.9"}"#).retry_after,
            Some(20)
        );
        assert_eq!(
            error(429, Some(30), None, r#"{"error":{"retryAfter":60}}"#).retry_after,
            Some(60)
        );
        assert_eq!(error(500, Some(100_000), None, "").retry_after, None);
        assert_eq!(error(401, None, None, "").code, Code::Unauthorized);
        assert_eq!(
            error(401, None, Some("BitChord realm=\"addon\""), "").code,
            Code::Refused
        );
    }

    /// The contract's own examples of Eclipse answers, with what a host is to make of each. They live with the
    /// contract, not here: the test runs when `STASH_ADDON_SPEC_FIXTURES` names their folder, and passes without
    /// having run when it does not.
    #[test]
    fn the_contracts_own_eclipse_examples_are_read_as_it_says() {
        let Some(folder) =
            std::env::var_os("STASH_ADDON_SPEC_FIXTURES").map(std::path::PathBuf::from)
        else {
            eprintln!("SKIPPED: STASH_ADDON_SPEC_FIXTURES is not set");
            return;
        };
        let load = |name: &str| -> Value {
            serde_json::from_slice(
                &std::fs::read(folder.join(name)).unwrap_or_else(|e| panic!("{name}: {e}")),
            )
            .unwrap()
        };
        let expect = load("expect.json");
        // The parts of a track this host keeps.
        let kept = |t: &Value| json!({ "id": t["id"], "title": t["title"], "artist": t["artist"], "album": t.get("album"), "durationMs": t.get("durationMs"), "isrc": t.get("isrc"), "explicit": t.get("explicit") });
        let ours = |t: &Track| json!({ "id": t.id, "title": t.title, "artist": t.artist, "album": t.album, "durationMs": t.duration_ms, "isrc": t.isrc, "explicit": t.explicit });
        let mut checked = 0;
        for (name, expected) in expect.as_object().unwrap() {
            let given = load(name);
            let (kind, _) = name.split_once('/').unwrap();
            match kind {
                "manifest" => match (manifest(&given), expected.get("refused")) {
                    (Err(e), Some(_)) => assert_eq!(e.code, Code::Refused, "{name}"),
                    (Ok(read), None) => {
                        let value = &expected["value"];
                        let wanted_resources: Vec<&str> = value["resources"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter_map(Value::as_str)
                            .filter(|r| matches!(*r, "search" | "stream" | "resolve"))
                            .collect();
                        assert_eq!(
                            (read.id.as_str(), read.name.as_str(), read.version.as_str()),
                            (
                                value["id"].as_str().unwrap(),
                                value["name"].as_str().unwrap(),
                                value["version"].as_str().unwrap()
                            ),
                            "{name}"
                        );
                        assert_eq!(read.resources, wanted_resources, "{name}");
                        assert_eq!(
                            read.resolve_by_isrc,
                            expected["viaIsrc"].as_bool().unwrap(),
                            "{name}"
                        );
                        let lossless = StreamRequest::default();
                        let lossy = StreamRequest {
                            tier: Tier::High,
                            ..StreamRequest::default()
                        };
                        let saver = StreamRequest {
                            tier: Tier::Low,
                            ..StreamRequest::default()
                        };
                        assert_eq!(
                            json!({ "lossless": quality(&read, &lossless), "high": quality(&read, &lossy), "low": quality(&read, &saver) }),
                            expected["quality"],
                            "{name}"
                        );
                    }
                    (got, _) => panic!("{name}: {:?}", got.map(|m| m.id)),
                },
                "search" => match search(&given) {
                    Err(e) => assert!(
                        expected["unavailable"].as_bool() == Some(true)
                            && e.code == Code::Unavailable,
                        "{name}"
                    ),
                    Ok(found) => assert_eq!(
                        found.iter().map(&ours).collect::<Vec<_>>(),
                        expected["value"]["tracks"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(&kept)
                            .collect::<Vec<_>>(),
                        "{name}"
                    ),
                },
                "resolve" => {
                    let asked = &expected["asked"];
                    let wanted = Wanted {
                        title: asked["title"].as_str().unwrap().into(),
                        artist: asked["artist"].as_str().unwrap().into(),
                        isrc: asked
                            .get("isrc")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        ..Wanted::default()
                    };
                    let found = if name.contains("isrc") {
                        resolved_by_isrc(&given, &wanted)
                    } else {
                        resolved(&given)
                    };
                    match &expected["value"]["item"] {
                        Value::Null => assert!(found.is_none(), "{name}"),
                        item => assert_eq!(
                            ours(&found.unwrap_or_else(|| panic!("{name}: no item"))),
                            kept(item),
                            "{name}"
                        ),
                    }
                }
                "stream" => match stream(&given) {
                    Err(e) if expected.get("miss").is_some() => {
                        assert_eq!(e.code, Code::NotFound, "{name}")
                    }
                    Err(e) => assert!(
                        expected["playable"].as_bool() == Some(false)
                            && e.code == Code::QualityUnavailable,
                        "{name}: {e}"
                    ),
                    Ok(read) => {
                        let value = &expected["value"];
                        let info = &read.info;
                        assert_eq!(
                            json!({ "url": info.url, "manifest": info.manifest.as_str(), "tier": info.tier.as_str(), "codec": info.codec, "container": info.container, "sampleRate": info.sample_rate, "bitDepth": info.bit_depth, "bitrate": info.bitrate, "expiresAt": info.expires_at }),
                            json!({ "url": value["url"], "manifest": value["manifest"], "tier": value["tier"], "codec": value.get("codec"), "container": value.get("container"), "sampleRate": value.get("sampleRate"), "bitDepth": value.get("bitDepth"), "bitrate": value.get("bitrate"), "expiresAt": value.get("expiresAt") }),
                            "{name}"
                        );
                        assert_eq!(
                            !read.stated,
                            expected
                                .get("defaultTier")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            "{name}"
                        );
                    }
                },
                "error" => {
                    let header = |wanted: &str| {
                        given["headers"]
                            .as_object()
                            .and_then(|h| h.iter().find(|(k, _)| k.eq_ignore_ascii_case(wanted)))
                            .and_then(|(_, v)| v.as_str())
                            .map(str::to_string)
                    };
                    let status = given["status"].as_u64().unwrap() as u16;
                    let value = &expected["value"];
                    if status == 200 {
                        // An error in a 200 is read where the answer is read: on a stream it is a miss, on a search
                        // it is no answer.
                        match given["route"].as_str() {
                            Some("search") => assert_eq!(
                                search(&given["body"]).unwrap_err().code.as_str(),
                                value["code"].as_str().unwrap(),
                                "{name}"
                            ),
                            _ => assert_eq!(
                                stream(&given["body"]).unwrap_err().code.as_str(),
                                value["code"].as_str().unwrap(),
                                "{name}"
                            ),
                        }
                    } else {
                        let read = error(
                            status,
                            header("retry-after").and_then(|v| v.trim().parse().ok()),
                            header("www-authenticate").as_deref(),
                            &given["body"].to_string(),
                        );
                        let code = if value.get("onlyBitChord").is_some() {
                            "refused"
                        } else {
                            value["code"].as_str().unwrap()
                        };
                        assert_eq!(read.code.as_str(), code, "{name}");
                        assert_eq!(read.is_miss(), value["miss"].as_bool().unwrap(), "{name}");
                        assert_eq!(
                            read.retry_after.map(u64::from),
                            value.get("retryAfter").and_then(Value::as_u64),
                            "{name}"
                        );
                    }
                }
                other => panic!("an example of a kind this test does not know: {other}"),
            }
            checked += 1;
        }
        assert!(checked >= 50, "only {checked} examples were found");
    }

    #[test]
    fn an_add_on_made_for_bitchord_is_read_as_bitchord_reads_it() {
        let with = |name: &str, value: Value| {
            let mut m = json!({ "id": "a.b", "resources": ["search", "stream"] });
            m[name] = value;
            manifest(&m)
        };
        // BitChord's own app verification: asked for by a yes in any spelling, and only by a yes.
        for yes in [json!(true), json!("yes"), json!("ON"), json!(1), json!("1")] {
            let refused = with("bitchordAuth", yes.clone()).unwrap_err();
            assert_eq!(
                (refused.code, refused.in_words().as_str()),
                (Code::Refused, ONLY_BITCHORD),
                "{yes}"
            );
        }
        for no in [
            json!(false),
            json!("no"),
            json!("off"),
            json!("False"),
            json!(0),
            json!({}),
            json!("maybe"),
            json!(null),
        ] {
            assert!(with("bitchordAuth", no.clone()).is_ok(), "{no}");
        }

        // No list of what it offers: it searches and plays. The words in any case; any id made one the contract takes.
        for bare in [
            json!({ "id": "x" }),
            json!({ "id": "x", "resources": [] }),
            json!({ "id": "x", "resources": null }),
        ] {
            assert!(is_manifest(&bare), "{bare}");
            assert_eq!(
                manifest(&bare).unwrap().resources,
                ["search", "stream"],
                "{bare}"
            );
        }
        assert!(!is_manifest(&json!({ "id": "x", "resources": "search" })));
        assert!(!is_manifest(&json!({ "resources": ["search"] })));
        let cased = manifest(&json!({ "id": "My Addon_v2!", "resources": ["Search", " STREAM "] }))
            .unwrap();
        assert_eq!(
            (cased.id.as_str(), cased.name.as_str()),
            ("my-addon-v2", "my-addon-v2")
        );
        assert_eq!(cased.resources, ["search", "stream"]);
        assert_eq!(
            manifest(&json!({ "id": "---" })).unwrap_err().code,
            Code::InvalidResponse
        );
        assert_eq!(contract_id(&"x".repeat(80)).map(|id| id.len()), Some(64));

        // The settings it declares: the first quality is asked for (whatever the setting's type), and every other
        // default is sent back, once, but never over the request's own words.
        let set = manifest(&json!({ "id": "x", "settings": [
            { "key": "quality", "options": [{ "value": "HI_RES" }, { "value": "LOSSLESS" }] },
            { "key": "region", "default": "US" },
            { "key": "atmos", "default": false },
            { "key": "limit", "default": 5 },
            { "key": "q", "default": "nothing" },
            { "key": "count", "default": 25 },
            { "key": "bad key", "default": "x" },
            { "key": "obj", "default": {} },
            { "key": "none" },
            { "key": "region", "default": "DE" }
        ] }))
        .unwrap();
        assert_eq!(set.quality.as_deref(), Some("HI_RES"));
        let sent: Vec<(&str, &str)> = set
            .sent
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            sent,
            [("region", "US"), ("atmos", "false"), ("count", "25")]
        );
        assert!(manifest(&json!({ "id": "x" })).unwrap().sent.is_empty());
    }

    #[test]
    fn a_stream_s_quality_is_read_from_the_add_on_s_own_words() {
        let url = "https://media.example.org/a";
        let read = |mut answer: Value| {
            answer["url"] = json!(url);
            stream(&answer).unwrap()
        };
        let told = |answer: Value| {
            let s = read(answer);
            (s.info.tier, s.info.codec, s.info.manifest, s.stated)
        };
        // What an add-on for Tidal answers: hi-res FLAC over DASH, said only in its own words.
        assert_eq!(
            told(
                json!({ "format": "dash", "quality": "Tidal · Hi-Res FLAC (DASH)", "streamQuality": "[Tidal] HI_RES_LOSSLESS" })
            ),
            (Tier::Hires, Some("flac".to_string()), Delivery::Dash, true)
        );
        let cd = read(json!({ "audioQuality": "LOSSLESS", "quality": "16-bit / 44.1 kHz FLAC" }));
        assert_eq!(
            (
                cd.info.tier,
                cd.info.bit_depth,
                cd.info.sample_rate,
                cd.info.codec.as_deref()
            ),
            (Tier::Lossless, Some(16), Some(44_100), Some("flac"))
        );
        let high = read(json!({ "streamQuality": "24-bit 96kHz" }));
        assert_eq!(
            (high.info.tier, high.info.bit_depth, high.info.sample_rate),
            (Tier::Hires, Some(24), Some(96_000))
        );
        // Its fields win over its words; a bitrate in kbps is lossy whatever else is said; a lossy codec too.
        let fields = read(
            json!({ "codec": "flac", "bitDepth": 16, "sampleRate": 44100, "quality": "Hi-Res 24-bit" }),
        );
        assert_eq!(
            (fields.info.tier, fields.info.bit_depth),
            (Tier::Lossless, Some(16))
        );
        assert_eq!(
            told(json!({ "quality": "MP3 320 kbps (from FLAC)" })).0,
            Tier::High
        );
        assert_eq!(
            told(json!({ "quality": "AAC 128kbps" })),
            (Tier::Low, None, Delivery::None, true)
        );
        assert_eq!(
            told(json!({ "codec": "aac", "streamQuality": "LOSSLESS" })),
            (Tier::Low, Some("aac".to_string()), Delivery::None, false)
        );
        // Words that only look like it name nothing.
        assert_eq!(
            told(json!({ "quality": "Flacid Thrash Edition" })),
            (Tier::Low, None, Delivery::None, false)
        );
        assert_eq!(told(json!({ "quality": "Chires" })).0, Tier::Low);

        // The codec, wherever it is named; a rate in kHz.
        assert_eq!(
            read(json!({ "fileCodec": "FLAC" })).info.codec.as_deref(),
            Some("flac")
        );
        assert_eq!(
            read(json!({ "containerFormat": "wav" }))
                .info
                .codec
                .as_deref(),
            Some("pcm")
        );
        assert_eq!(
            read(json!({ "mimeType": "audio/flac; rate=44100" }))
                .info
                .codec
                .as_deref(),
            Some("flac")
        );
        assert_eq!(read(json!({ "mimeType": "audio/mp4" })).info.codec, None);
        let khz = read(json!({ "codec": "flac", "sampleRate": 96, "bitDepth": 24 }));
        assert_eq!(
            (khz.info.sample_rate, khz.info.tier),
            (Some(96_000), Tier::Hires)
        );
        assert_eq!(
            read(json!({ "codec": "flac", "sampleRate": "44.1" }))
                .info
                .sample_rate,
            Some(44_100)
        );

        // How it is delivered, in each spelling.
        for (name, word, delivered) in [
            ("mediaType", "application/vnd.apple.mpegurl", Delivery::Hls),
            ("manifest", "mpd", Delivery::Dash),
            ("manifest", "M3U8", Delivery::Hls),
            ("format", "application/dash+xml", Delivery::Dash),
            ("mediaType", "file", Delivery::None),
            ("format", "flac", Delivery::None),
        ] {
            assert_eq!(
                read(json!({ name: word })).info.manifest,
                delivered,
                "{name} {word}"
            );
        }
        // `manifest` wins over the older spellings.
        assert_eq!(
            read(json!({ "manifest": "none", "format": "dash" }))
                .info
                .manifest,
            Delivery::None
        );

        // Not protected said in any way plays; protected, or a word Stash does not know, does not. A video that
        // goes with the sound is passed over.
        for clear in [
            json!(false),
            json!("false"),
            json!("none"),
            json!("None"),
            json!("no"),
            json!(0),
            json!(null),
            json!(""),
        ] {
            assert!(
                stream(&json!({ "url": url, "encrypted": clear.clone() })).is_ok(),
                "{clear}"
            );
        }
        for locked in [
            json!(true),
            json!("cenc"),
            json!("yes"),
            json!(1),
            json!({ "scheme": "cenc" }),
        ] {
            let refused = stream(&json!({ "url": url, "encrypted": locked.clone() })).unwrap_err();
            assert_eq!(refused.code, Code::QualityUnavailable, "{locked}");
        }
        assert!(
            stream(&json!({ "url": url, "video": { "url": "https://media.example.org/v" } }))
                .is_ok()
        );

        // An error in a 200: "it is not there" in the add-on's own words, or "not yet" when it says when.
        let missing = stream(&json!({ "error": "Add your debrid key in the add-on's settings" }))
            .unwrap_err();
        assert_eq!(missing.code, Code::NotFound);
        assert_eq!(
            missing.in_words(),
            "the add-on does not have it (it says: “Add your debrid key in the add-on's settings”)"
        );
        assert_eq!(
            stream(&json!({ "error": { "message": "No match" } }))
                .unwrap_err()
                .in_words(),
            "the add-on does not have it (it says: “No match”)"
        );
        assert_eq!(
            stream(&json!({ "error": true })).unwrap_err().in_words(),
            "the add-on does not have it"
        );
        for later in [
            json!({ "error": "preparing", "retryAfter": 20 }),
            json!({ "error": { "message": "busy", "retryAfter": "20" } }),
        ] {
            let not_yet = stream(&later).unwrap_err();
            assert_eq!(
                (not_yet.code, not_yet.retry_after, not_yet.is_miss()),
                (Code::Unavailable, Some(20), false),
                "{later}"
            );
        }
    }
}
