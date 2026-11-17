//! Where a YouTube video's sound plays from.
//!
//! YouTube's apps are asked in turn (the one that last gave a stream first) until one names a file of sound by a plain
//! address that also gives the file's last byte: an app can answer with addresses that play a megabyte and then
//! refuse, and only asking for the end tells. Scrambled addresses (the web's) are never taken: unscrambling them
//! is a moving target that Stash leaves alone, as Stash for Android does (`InnerTubeClient.playerForAudio`,
//! `PreviewUrlExtractor`, `AudioUrlTailProbe`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::innertube::{AFTER_BOT_CHECK, BOT_CHECK, Client, InnerTube, PLAYERS, trouble};
use super::parse::video_id;
use crate::addons::{AddonError, Code, Delivery, StreamInfo, Tier};
use crate::identity::clean_text;

/// How long the end of a file is waited for.
const PROBE: Duration = Duration::from_secs(4);
/// The longest address taken.
const URL_CAP: usize = 4_096;
/// Lossy at 256 kbps or more is the higher tier.
const HIGH_BITRATE: u64 = 256_000;

/// One file of sound a player answer names.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Format {
    pub url: String,
    pub codec: &'static str,
    pub container: &'static str,
    pub bitrate: u64,
    pub sample_rate: Option<u32>,
    pub length: Option<u64>,
    pub duration_ms: Option<u64>,
}

fn number(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

/// The best file of sound in a player answer: sound only, named by a plain https address, the song as it was made
/// (no levelled-out or dubbed version); Opus at the highest bitrate, else whatever has the highest.
pub(crate) fn pick(answer: &Value) -> Option<Format> {
    let formats = answer
        .pointer("/streamingData/adaptiveFormats")?
        .as_array()?;
    let mut best: Option<(bool, Format)> = None;
    for format in formats {
        let Some(mime) = format.get("mimeType").and_then(Value::as_str) else {
            continue;
        };
        let Some(url) = format.get("url").and_then(Value::as_str) else {
            continue;
        };
        // https (or this computer, for a test's server), and no sign-in inside it.
        let plain = mime.starts_with("audio/")
            && url.len() <= URL_CAP
            && crate::addons::checked_url(url, "the stream").is_ok()
            && format.get("isDrc").and_then(Value::as_bool) != Some(true)
            && format
                .pointer("/audioTrack/audioIsDefault")
                .and_then(Value::as_bool)
                != Some(false);
        if !plain {
            continue;
        }
        let opus = mime.contains("opus");
        let found = Format {
            url: url.to_string(),
            codec: if opus {
                "opus"
            } else if mime.contains("mp4a") {
                "aac"
            } else {
                "other"
            },
            container: if mime.starts_with("audio/webm") {
                "webm"
            } else {
                "mp4"
            },
            bitrate: number(format.get("bitrate")).unwrap_or(0),
            sample_rate: number(format.get("audioSampleRate")).and_then(|n| u32::try_from(n).ok()),
            length: number(format.get("contentLength")),
            duration_ms: number(format.get("approxDurationMs")),
        };
        let better = best
            .as_ref()
            .is_none_or(|(was_opus, was)| (opus, found.bitrate) > (*was_opus, was.bitrate));
        if better {
            best = Some((opus, found));
        }
    }
    best.map(|(_, format)| format)
}

/// What a player answer says of the video, when it gives no stream.
#[derive(Debug, PartialEq)]
pub(crate) enum Refusal {
    /// YouTube asks whether the listener is a bot: it is left alone for a while.
    BotCheck,
    /// The video is gone, or private.
    Gone(String),
    /// This app may not play it (its age, its country, its label), in YouTube's words.
    NotHere(String),
}

/// Why a player answer gives no stream; nothing when it is playable.
pub(crate) fn refusal(answer: &Value) -> Option<Refusal> {
    let status = answer
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if status == "OK" {
        return None;
    }
    let reason = answer
        .pointer("/playabilityStatus/reason")
        .and_then(Value::as_str)
        .map(|said| clean_text(said, 200))
        .unwrap_or_default();
    let lower = reason.to_lowercase();
    // "Sign in to confirm you're not a bot" is the check; "Sign in to confirm your age" is not.
    let bot_check = lower.contains("not a bot")
        || [
            "confirm you\u{2019}re not",
            "confirm you're not",
            "confirm youre not",
        ]
        .iter()
        .any(|said| lower.contains(said));
    Some(if bot_check {
        Refusal::BotCheck
    } else if status == "ERROR" {
        Refusal::Gone(reason)
    } else {
        Refusal::NotHere(reason)
    })
}

/// When the address stops working, from its own `expire=` (seconds), in milliseconds.
fn expiry(url: &str) -> Option<i64> {
    let url = url::Url::parse(url).ok()?;
    let (_, at) = url.query_pairs().find(|(name, _)| name == "expire")?;
    at.parse::<i64>().ok()?.checked_mul(1_000)
}

/// The line to YouTube's player, and which app last gave a stream.
pub(crate) struct Players {
    winner: AtomicUsize,
    prober: ureq::Agent,
}

impl Default for Players {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(PROBE))
            .http_status_as_error(false)
            .build();
        Self {
            winner: AtomicUsize::new(0),
            prober: config.into(),
        }
    }
}

impl Players {
    /// Whether the file at `url` gives its last byte. Only a plain refusal counts as no: a file of unknown length,
    /// a slow answer or a broken line passes, and the engine finds out.
    fn gives_its_end(&self, format: &Format) -> bool {
        let Some(length) = format.length.filter(|length| *length > 0) else {
            return true;
        };
        let asked = self
            .prober
            .get(&format.url)
            .header("Range", format!("bytes={}-", length - 1))
            .call();
        !matches!(asked, Ok(answer) if matches!(answer.status().as_u16(), 401 | 403 | 410))
    }

    /// Where the video `id` plays from.
    pub(crate) fn stream(&self, line: &InnerTube, id: &str) -> Result<StreamInfo, AddonError> {
        let Some(id) = video_id(id) else {
            return Err(trouble(Code::NotFound, "that is not a YouTube video"));
        };
        let first = self.winner.load(Ordering::Relaxed) % PLAYERS.len();
        let order = std::iter::once(first).chain((0..PLAYERS.len()).filter(|n| *n != first));
        // What is said when no app gives a stream: the video's own trouble before the line's.
        let mut not_here: Option<String> = None;
        let mut failed: Option<AddonError> = None;
        // How many apps asked whether the listener is a bot. One app asking is that app's trouble (the next is
        // tried); all of them asking is YouTube's word on this visitor, and it is left alone for a while.
        let mut bot_checks = 0;
        for n in order {
            let client: &Client = &PLAYERS[n];
            let answer = match line.call(
                client,
                "player",
                json!({ "videoId": id, "contentCheckOk": true, "racyCheckOk": true }),
            ) {
                Ok(answer) => answer,
                // Left alone, or out of reach: the next app would hear the same.
                Err(e) if matches!(e.code, Code::RateLimited | Code::Network) => return Err(e),
                Err(e) => {
                    failed = Some(e);
                    continue;
                }
            };
            match refusal(&answer) {
                Some(Refusal::BotCheck) => {
                    bot_checks += 1;
                    continue;
                }
                Some(Refusal::Gone(why)) => {
                    return Err(trouble(
                        Code::NotFound,
                        said("the video is no longer on YouTube", &why),
                    ));
                }
                Some(Refusal::NotHere(why)) => {
                    not_here.get_or_insert(why);
                    continue;
                }
                None => {}
            }
            let Some(format) = pick(&answer) else {
                continue;
            };
            if !self.gives_its_end(&format) {
                continue;
            }
            self.winner.store(n, Ordering::Relaxed);
            return Ok(StreamInfo {
                expires_at: expiry(&format.url),
                url: format.url,
                manifest: Delivery::None,
                tier: if format.bitrate >= HIGH_BITRATE {
                    Tier::High
                } else {
                    Tier::Low
                },
                codec: Some(format.codec.to_string()),
                container: Some(format.container.to_string()),
                bit_depth: None,
                sample_rate: format.sample_rate,
                bitrate: Some(format.bitrate).filter(|rate| *rate > 0),
                duration_ms: format.duration_ms,
                encrypted: None,
            });
        }
        if bot_checks == PLAYERS.len() {
            line.quiet_for(AFTER_BOT_CHECK, BOT_CHECK);
            return Err(AddonError {
                retry_after: Some(AFTER_BOT_CHECK.as_secs() as u32),
                ..trouble(
                    Code::RateLimited,
                    format!("{BOT_CHECK}: it is left alone for a few minutes"),
                )
            });
        }
        Err(match (not_here, failed) {
            (Some(why), _) => trouble(
                Code::RegionUnavailable,
                said("YouTube does not play this video without signing in", &why),
            ),
            (None, Some(e)) => e,
            (None, None) => trouble(
                Code::Unavailable,
                "YouTube gave no address its sound plays from",
            ),
        })
    }
}

/// Stash's words, with YouTube's own after them when it gave any.
fn said(words: &str, youtube: &str) -> String {
    if youtube.is_empty() {
        words.to_string()
    } else {
        format!("{words} (“{youtube}”)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(mime: &str, bitrate: u64, url: Option<&str>) -> Value {
        let mut format = json!({ "mimeType": mime, "bitrate": bitrate, "contentLength": "1000", "audioSampleRate": "48000", "approxDurationMs": "200000" });
        if let Some(url) = url {
            format["url"] = json!(url);
        }
        format
    }

    #[test]
    fn the_best_plain_sound_is_picked() {
        let opus = "audio/webm; codecs=\"opus\"";
        // Scrambled (no plain address), not https, levelled out, dubbed: never taken.
        let mut levelled = format(opus, 162_000, Some("https://v.example/drc"));
        levelled["isDrc"] = json!(true);
        let mut dubbed = format(opus, 163_000, Some("https://v.example/dub"));
        dubbed["audioTrack"] = json!({ "audioIsDefault": false });
        let answer = json!({ "streamingData": { "adaptiveFormats": [
            format("video/mp4; codecs=\"avc1\"", 900_000, Some("https://v.example/video")),
            format("audio/mp4; codecs=\"mp4a.40.2\"", 130_000, Some("https://v.example/aac")),
            format(opus, 70_000, Some("https://v.example/opus-low")),
            format(opus, 128_000, Some("https://v.example/opus")),
            format(opus, 160_000, None),
            format(opus, 161_000, Some("http://v.example/plain")),
            levelled,
            dubbed,
        ] } });
        let best = pick(&answer).unwrap();
        assert_eq!(
            (best.url.as_str(), best.codec, best.container, best.bitrate),
            ("https://v.example/opus", "opus", "webm", 128_000)
        );
        assert_eq!(
            (best.sample_rate, best.length, best.duration_ms),
            (Some(48_000), Some(1_000), Some(200_000))
        );

        // Without Opus, the highest of the rest.
        let answer = json!({ "streamingData": { "adaptiveFormats": [
            format("audio/mp4; codecs=\"mp4a.40.5\"", 50_000, Some("https://v.example/low")),
            format("audio/mp4; codecs=\"mp4a.40.2\"", 130_000, Some("https://v.example/aac")),
        ] } });
        let best = pick(&answer).unwrap();
        assert_eq!(
            (best.url.as_str(), best.codec, best.container),
            ("https://v.example/aac", "aac", "mp4")
        );
        assert_eq!(pick(&json!({ "streamingData": {} })), None);
    }

    #[test]
    fn a_refusal_is_told_by_what_youtube_says() {
        let says = |status: &str, reason: &str| {
            refusal(&json!({ "playabilityStatus": { "status": status, "reason": reason } }))
        };
        assert_eq!(says("OK", ""), None);
        assert_eq!(
            says("LOGIN_REQUIRED", "Sign in to confirm you’re not a bot"),
            Some(Refusal::BotCheck)
        );
        assert_eq!(
            says("LOGIN_REQUIRED", "Sign in to confirm your age"),
            Some(Refusal::NotHere("Sign in to confirm your age".to_string()))
        );
        assert_eq!(
            says("ERROR", "This video is unavailable"),
            Some(Refusal::Gone("This video is unavailable".to_string()))
        );
        assert_eq!(
            says("UNPLAYABLE", "The page needs to be reloaded."),
            Some(Refusal::NotHere(
                "The page needs to be reloaded.".to_string()
            ))
        );
        assert_eq!(refusal(&json!({})), Some(Refusal::NotHere(String::new())));
    }

    #[test]
    fn an_address_says_when_it_ends() {
        assert_eq!(
            expiry("https://r1.example/videoplayback?expire=1794900000&itag=251"),
            Some(1_794_900_000_000)
        );
        assert_eq!(expiry("https://r1.example/videoplayback?itag=251"), None);
    }
}
