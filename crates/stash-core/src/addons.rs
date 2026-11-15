//! Add-ons: where music comes from. Stash desktop ships with none; the listener adds each one from a link.
//!
//! An add-on is an HTTP server described by a manifest (Stash Add-ons v1). The core is the host: it reads the
//! manifest, asks only for what the manifest lists, checks every answer before using it, and treats every URL an
//! add-on names as untrusted. Nothing an add-on sends is run: it is data.

use std::time::Duration;

use serde::Deserialize;
use url::Url;

/// The longest URL an add-on may name.
const URL_CAP: usize = 16_384;
/// The largest answer read from an add-on's API (not its media).
const BODY_CAP: u64 = 2 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

/// A quality tier, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Lossy below 256 kbps.
    Low,
    /// Lossy at 256 kbps or more.
    High,
    /// Lossless up to 16-bit / 48 kHz.
    Lossless,
    /// Lossless above that.
    Hires,
}

impl Tier {
    fn as_str(self) -> &'static str {
        match self {
            Tier::Low => "low",
            Tier::High => "high",
            Tier::Lossless => "lossless",
            Tier::Hires => "hires",
        }
    }
}

/// How a stream is delivered: one file, or a DASH or HLS manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    None,
    Dash,
    Hls,
}

impl Delivery {
    fn as_str(self) -> &'static str {
        match self {
            Delivery::None => "none",
            Delivery::Dash => "dash",
            Delivery::Hls => "hls",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub stash: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    #[serde(default)]
    pub description: Option<String>,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub explicit: Option<bool>,
    #[serde(default)]
    pub max_tier: Option<Tier>,
}

#[derive(Deserialize)]
struct SearchResponse {
    tracks: Vec<Track>,
}

/// What a stream request accepts.
#[derive(Debug, Clone)]
pub struct StreamRequest {
    /// The highest tier wanted.
    pub tier: Tier,
    /// The lowest tier accepted.
    pub min_tier: Tier,
    /// The deliveries the engine can play.
    pub deliveries: Vec<Delivery>,
}

impl Default for StreamRequest {
    /// The best there is, in any delivery mpv plays.
    fn default() -> Self {
        Self {
            tier: Tier::Hires,
            min_tier: Tier::Low,
            deliveries: vec![Delivery::None, Delivery::Dash, Delivery::Hls],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub url: String,
    pub manifest: Delivery,
    pub tier: Tier,
    #[serde(default)]
    pub codec: Option<String>,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default)]
    pub bit_depth: Option<u32>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub bitrate: Option<u64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub encrypted: Option<bool>,
}

/// Why a call to an add-on did not give an answer. The first eight are the add-on's own words (its `error.code`);
/// the last three are the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    BadRequest,
    Unauthorized,
    NotFound,
    QualityUnavailable,
    RegionUnavailable,
    RateLimited,
    QuotaExceeded,
    Unavailable,
    Timeout,
    Network,
    /// The add-on answered, but not with what the contract allows.
    InvalidResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("add-on: {code:?}: {message}")]
pub struct AddonError {
    pub code: Code,
    pub message: String,
    /// How long the add-on asked to be left alone, in seconds.
    pub retry_after: Option<u32>,
}

impl AddonError {
    fn host(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::host(Code::InvalidResponse, message)
    }

    /// A miss for this song (try the next add-on), as opposed to trouble with the add-on itself.
    pub fn is_miss(&self) -> bool {
        matches!(
            self.code,
            Code::NotFound | Code::QualityUnavailable | Code::RegionUnavailable
        )
    }
}

type Result<T> = std::result::Result<T, AddonError>;

/// An add-on the listener added, ready to be asked.
pub struct Addon {
    pub manifest: Manifest,
    base: String,
    agent: ureq::Agent,
}

impl Addon {
    /// Reads the add-on at `link`, the URL of its `manifest.json`.
    ///
    /// The link must be https (plain http only on this computer, for development), and the manifest must be a v1
    /// server add-on with a well-formed id. Redirects are not followed: an add-on is the origin its link names.
    pub fn from_link(link: &str) -> Result<Self> {
        let url = checked_url(link, "the add-on link")?;
        let Some(base) = url
            .as_str()
            .split('?')
            .next()
            .and_then(|u| u.strip_suffix("/manifest.json"))
        else {
            return Err(AddonError::host(
                Code::BadRequest,
                "an add-on link ends in /manifest.json",
            ));
        };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(concat!("Stash-desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let manifest: Manifest = get_json(&agent, url.as_str())?;
        check_manifest(&manifest)?;
        Ok(Self {
            manifest,
            base: base.to_string(),
            agent,
        })
    }

    fn offers(&self, resource: &str) -> Result<()> {
        if self.manifest.resources.iter().any(|r| r == resource) {
            Ok(())
        } else {
            Err(AddonError::host(
                Code::NotFound,
                format!("{} does not offer {resource}", self.manifest.name),
            ))
        }
    }

    /// Tracks matching `query`, at most `limit` (1 to 200).
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<Track>> {
        self.offers("search")?;
        let mut url = Url::parse(&format!("{}/search", self.base))
            .map_err(|e| AddonError::invalid(e.to_string()))?;
        let q: String = query.chars().take(200).collect();
        url.query_pairs_mut()
            .append_pair("q", &q)
            .append_pair("limit", &limit.clamp(1, 200).to_string());
        let response: SearchResponse = get_json(&self.agent, url.as_str())?;
        Ok(response.tracks)
    }

    /// Where to play the add-on's track `id` from, within what `want` accepts. The answer is checked: a tier outside
    /// the asked range, a delivery that was not asked for, an encrypted stream, a URL that is not https, or a tier
    /// its own codec contradicts are all refused as [`Code::InvalidResponse`].
    pub fn stream(&self, id: &str, want: &StreamRequest) -> Result<StreamInfo> {
        self.offers("stream")?;
        let mut url = Url::parse(&format!("{}/stream/", self.base))
            .map_err(|e| AddonError::invalid(e.to_string()))?;
        url.path_segments_mut()
            .map_err(|_| AddonError::invalid("the add-on's address has no path"))?
            .pop_if_empty()
            .push(id);
        let deliveries: Vec<&str> = want.deliveries.iter().map(|d| d.as_str()).collect();
        url.query_pairs_mut()
            .append_pair("tier", want.tier.as_str())
            .append_pair("minTier", want.min_tier.as_str())
            .append_pair("manifest", &deliveries.join(","));
        let info: StreamInfo = get_json(&self.agent, url.as_str())?;
        check_stream(&info, want)?;
        Ok(info)
    }
}

/// A URL an add-on (or the listener's link) names: https, or http on this computer only; no credentials in it.
fn checked_url(text: &str, what: &str) -> Result<Url> {
    if text.len() > URL_CAP {
        return Err(AddonError::invalid(format!(
            "{what} is longer than {URL_CAP} characters"
        )));
    }
    let url =
        Url::parse(text).map_err(|e| AddonError::invalid(format!("{what} is not a URL: {e}")))?;
    let loopback = match url.host() {
        Some(url::Host::Domain(name)) => name == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        other => {
            return Err(AddonError::invalid(format!(
                "{what} must be https (it is {other})"
            )));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AddonError::invalid(format!(
            "{what} carries a user name or password"
        )));
    }
    Ok(url)
}

fn check_manifest(m: &Manifest) -> Result<()> {
    if m.stash != 1 {
        return Err(AddonError::invalid(format!(
            "this add-on speaks contract {}, Stash desktop speaks 1",
            m.stash
        )));
    }
    let id = m.id.as_bytes();
    let id_ok = (1..=64).contains(&id.len())
        && (id[0].is_ascii_lowercase() || id[0].is_ascii_digit())
        && id
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'.' || *b == b'-');
    if !id_ok {
        return Err(AddonError::invalid("the add-on's id is not well-formed"));
    }
    if m.kind != "server" {
        return Err(AddonError::invalid(format!(
            "a {} add-on cannot be used here (server add-ons can)",
            m.kind
        )));
    }
    if m.name.trim().is_empty() || m.version.trim().is_empty() {
        return Err(AddonError::invalid("the add-on has no name or version"));
    }
    let mut seen = std::collections::HashSet::new();
    if !m.resources.iter().all(|r| seen.insert(r.as_str())) {
        return Err(AddonError::invalid("the add-on lists a resource twice"));
    }
    Ok(())
}

fn check_stream(info: &StreamInfo, want: &StreamRequest) -> Result<()> {
    checked_url(&info.url, "the stream URL")?;
    if !want.deliveries.contains(&info.manifest) {
        return Err(AddonError::invalid(format!(
            "the add-on answered a {} stream, which was not asked for",
            info.manifest.as_str()
        )));
    }
    if info.tier < want.min_tier || info.tier > want.tier {
        return Err(AddonError::invalid(format!(
            "the add-on answered tier {}, outside what was asked",
            info.tier.as_str()
        )));
    }
    if info.encrypted == Some(true) {
        return Err(AddonError::invalid(
            "the stream is encrypted; the contract defines no keys",
        ));
    }
    if let Some(codec) = info.codec.as_deref() {
        let lossless = matches!(codec, "flac" | "alac" | "pcm");
        if lossless != (info.tier >= Tier::Lossless) {
            return Err(AddonError::invalid(format!(
                "tier {} does not go with codec {codec}",
                info.tier.as_str()
            )));
        }
    }
    let above_cd =
        info.bit_depth.is_some_and(|b| b > 16) || info.sample_rate.is_some_and(|r| r > 48_000);
    if info.tier == Tier::Lossless && above_cd {
        return Err(AddonError::invalid(
            "a lossless (CD) tier was answered with a hi-res format",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorDetail {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    retry_after: Option<u64>,
}

/// GETs `url` and reads a JSON answer of type `T`, or the add-on's error (contract section 7).
fn get_json<T: serde::de::DeserializeOwned>(agent: &ureq::Agent, url: &str) -> Result<T> {
    let mut response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::Timeout(_) => {
            AddonError::host(Code::Timeout, "the add-on did not answer in time")
        }
        other => AddonError::host(Code::Network, other.to_string()),
    })?;
    let status = response.status().as_u16();
    let header_retry = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let body = response
        .body_mut()
        .with_config()
        .limit(BODY_CAP)
        .read_to_string()
        .map_err(|e| AddonError::invalid(format!("the answer could not be read: {e}")))?;

    if (200..300).contains(&status) {
        return serde_json::from_str(&body).map_err(|e| {
            AddonError::invalid(format!("the answer is not what the contract says: {e}"))
        });
    }
    if (300..400).contains(&status) {
        return Err(AddonError::invalid(format!(
            "the add-on redirected ({status}); redirects are not followed"
        )));
    }
    let detail = serde_json::from_str::<ErrorBody>(&body)
        .ok()
        .map(|b| b.error);
    let by_status = match status {
        401 => Code::Unauthorized,
        404 => Code::NotFound,
        429 => Code::RateLimited,
        451 => Code::RegionUnavailable,
        400..=499 => Code::BadRequest,
        _ => Code::Unavailable,
    };
    let code = match detail.as_ref().and_then(|d| d.code.as_deref()) {
        Some("bad_request") => Code::BadRequest,
        Some("unauthorized") => Code::Unauthorized,
        Some("not_found") => Code::NotFound,
        Some("quality_unavailable") => Code::QualityUnavailable,
        Some("region_unavailable") => Code::RegionUnavailable,
        Some("rate_limited") => Code::RateLimited,
        Some("quota_exceeded") => Code::QuotaExceeded,
        Some("unavailable") => Code::Unavailable,
        _ => by_status,
    };
    let retry_after = detail
        .as_ref()
        .and_then(|d| d.retry_after)
        .or(header_retry)
        .filter(|s| (1..=86_400).contains(s))
        .map(|s| s as u32);
    let message: String = detail
        .and_then(|d| d.message)
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect();
    Err(AddonError {
        code,
        message,
        retry_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(json: &str) -> Result<()> {
        check_manifest(&serde_json::from_str::<Manifest>(json).unwrap())
    }

    #[test]
    fn a_manifest_must_be_a_v1_server_add_on_with_a_well_formed_id() {
        let good = r#"{"stash":1,"id":"example-lossless","name":"Example","version":"1.0.0","kind":"server","resources":["search","stream"]}"#;
        assert!(manifest(good).is_ok());
        assert!(manifest(&good.replace("\"stash\":1", "\"stash\":2")).is_err());
        assert!(manifest(&good.replace("example-lossless", "Example Lossless")).is_err());
        assert!(manifest(&good.replace("\"server\"", "\"frame\"")).is_err());
        assert!(
            manifest(&good.replace("[\"search\",\"stream\"]", "[\"search\",\"search\"]")).is_err()
        );
    }

    #[test]
    fn urls_are_https_or_this_computer_and_never_carry_credentials() {
        assert!(checked_url("https://addon.example.org/manifest.json", "link").is_ok());
        assert!(checked_url("http://127.0.0.1:8787/manifest.json", "link").is_ok());
        assert!(checked_url("http://localhost:8787/manifest.json", "link").is_ok());
        assert!(checked_url("http://addon.example.org/manifest.json", "link").is_err());
        assert!(checked_url("http://192.168.1.10/manifest.json", "link").is_err());
        assert!(checked_url("file:///C:/music/a.flac", "link").is_err());
        assert!(checked_url("https://user:pw@addon.example.org/manifest.json", "link").is_err());
    }

    fn info(json: &str) -> StreamInfo {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_stream_answer_is_checked_against_what_was_asked() {
        let want = StreamRequest {
            tier: Tier::Hires,
            min_tier: Tier::Lossless,
            deliveries: vec![Delivery::None],
        };
        let ok = r#"{"url":"https://cdn.example.org/a.flac","manifest":"none","tier":"lossless","codec":"flac","bitDepth":16,"sampleRate":44100,"cors":false}"#;
        assert!(check_stream(&info(ok), &want).is_ok());
        // Below the asked range; a delivery not asked for; encrypted; lossless tier with a lossy codec; CD tier, hi-res format.
        assert!(
            check_stream(
                &info(
                    &ok.replace("\"lossless\"", "\"high\"")
                        .replace("flac\"", "aac\"")
                ),
                &want
            )
            .is_err()
        );
        assert!(check_stream(&info(&ok.replace("\"none\"", "\"dash\"")), &want).is_err());
        assert!(
            check_stream(
                &info(&ok.replace("\"cors\":false", "\"cors\":false,\"encrypted\":true")),
                &want
            )
            .is_err()
        );
        assert!(
            check_stream(
                &info(&ok.replace("\"codec\":\"flac\"", "\"codec\":\"aac\"")),
                &want
            )
            .is_err()
        );
        assert!(
            check_stream(
                &info(&ok.replace("\"bitDepth\":16", "\"bitDepth\":24")),
                &want
            )
            .is_err()
        );
        assert!(
            check_stream(
                &info(&ok.replace("https://cdn.example.org", "http://cdn.example.org")),
                &want
            )
            .is_err()
        );
    }
}
