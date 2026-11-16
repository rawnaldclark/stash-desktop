//! Add-ons: where music comes from. Stash desktop ships with none; the listener adds each one from a link.
//!
//! An add-on is an HTTP server described by a manifest (Stash Add-ons v1). The core is the host: it reads the
//! manifest, asks only for what the manifest lists, checks every answer before using it, and treats every URL an
//! add-on names as untrusted. Nothing an add-on sends is run: it is data.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

/// The longest URL an add-on may name.
const URL_CAP: usize = 16_384;
/// The largest answer read from an add-on's API (not its media).
const BODY_CAP: u64 = 2 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
/// How long finding the add-on's server by its name, and then connecting to it, may take (together, less than
/// [`TIMEOUT`]).
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

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
    pub fn as_str(self) -> &'static str {
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
    pub fn as_str(self) -> &'static str {
        match self {
            Delivery::None => "none",
            Delivery::Dash => "dash",
            Delivery::Hls => "hls",
        }
    }
}

/// Which way an add-on speaks: Stash's own contract, or the looser one of Eclipse and its family, which
/// [`crate::eclipse`] reads into the same shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    #[default]
    V1,
    Eclipse,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub stash: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    #[serde(default)]
    pub description: Option<String>,
    pub resources: Vec<String>,
    /// The three below are the host's own notes on an Eclipse add-on, kept with the manifest; an add-on does not
    /// set them.
    #[serde(default)]
    pub dialect: Dialect,
    /// The add-on finds a song only by its ISRC.
    #[serde(default)]
    pub resolve_by_isrc: bool,
    /// The add-on's own word for the quality to ask for.
    #[serde(default)]
    pub quality: Option<String>,
    /// An Eclipse add-on that says its songs are not to be saved (`allowDownloads` set to nothing).
    #[serde(default)]
    pub no_downloads: bool,
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
    /// Its covers, as the add-on lists them.
    #[serde(default)]
    pub artwork: Vec<Artwork>,
}

/// A cover an add-on names.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Artwork {
    pub url: String,
}

impl Track {
    /// The first of its covers whose address keeps the URL rule (https, no oddities).
    pub fn art(&self) -> Option<&str> {
        self.artwork
            .iter()
            .map(|a| a.url.as_str())
            .find(|url| url.len() <= 2_048 && checked_url(url, "the cover").is_ok())
    }
}

/// A song the host knows, as it asks an add-on for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Wanted {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// The recording's own length. Left out when the host cannot vouch for it.
    pub duration_ms: Option<u64>,
    pub isrc: Option<String>,
    pub explicit: Option<bool>,
}

#[derive(Deserialize)]
struct ResolveResponse {
    item: Option<Track>,
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
    /// The host will not use this add-on, and the message says why in the listener's words.
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("add-on: {code:?}: {message}")]
pub struct AddonError {
    pub code: Code,
    pub message: String,
    /// How long the add-on asked to be left alone, in seconds.
    pub retry_after: Option<u32>,
}

impl Code {
    /// The code in the contract's spelling; the host's own three in the same style.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::BadRequest => "bad_request",
            Code::Unauthorized => "unauthorized",
            Code::NotFound => "not_found",
            Code::QualityUnavailable => "quality_unavailable",
            Code::RegionUnavailable => "region_unavailable",
            Code::RateLimited => "rate_limited",
            Code::QuotaExceeded => "quota_exceeded",
            Code::Unavailable => "unavailable",
            Code::Timeout => "timeout",
            Code::Network => "network",
            Code::InvalidResponse => "invalid_response",
            Code::Refused => "refused",
        }
    }
}

impl AddonError {
    /// The failure in a listener's words.
    pub fn in_words(&self) -> String {
        if self.code == Code::Refused {
            return self.message.clone();
        }
        let what = match self.code {
            Code::BadRequest => "the add-on did not understand the request",
            Code::Unauthorized => "the add-on did not accept your key",
            Code::NotFound => "the add-on does not have it",
            Code::QualityUnavailable => {
                "the add-on does not have it in a quality that was asked for"
            }
            Code::RegionUnavailable => "the add-on cannot give it in your region",
            Code::RateLimited => "the add-on is being asked too often",
            Code::QuotaExceeded => "your allowance with the add-on is used up",
            Code::Unavailable => "the add-on is not working right now",
            Code::Timeout => "the add-on did not answer in time",
            Code::Network => "the add-on could not be reached",
            Code::InvalidResponse => {
                "the add-on answered with something the contract does not allow"
            }
            Code::Refused => "",
        };
        match self.code {
            // The host's own messages say what was wrong with the answer.
            Code::InvalidResponse | Code::BadRequest if !self.message.is_empty() => {
                format!("{what} ({})", self.message)
            }
            _ => what.to_string(),
        }
    }

    pub(crate) fn host(code: Code, message: impl Into<String>) -> Self {
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
    link: String,
    base: String,
    agent: ureq::Agent,
}

impl Addon {
    /// Reads the add-on at `link`, the URL of its `manifest.json`.
    ///
    /// The link must be https (plain http only on this computer, for development), and the manifest must be a v1
    /// server add-on with a well-formed id. Redirects are not followed: an add-on is the origin its link names.
    pub fn from_link(link: &str) -> Result<Self> {
        let (url, base) = base_of(link)?;
        let agent = agent();
        let given: serde_json::Value = get_json(&agent, url.as_str())?;
        let manifest = match given.get("stash").map(|v| v.as_u64()) {
            Some(Some(1)) => {
                let mut manifest: Manifest = serde_json::from_value(given).map_err(|e| {
                    AddonError::invalid(format!("the manifest is not what the contract says: {e}"))
                })?;
                // The host's own notes are not an add-on's to set.
                (manifest.dialect, manifest.resolve_by_isrc, manifest.quality) =
                    (Dialect::V1, false, None);
                manifest
            }
            Some(Some(0)) => {
                return Err(AddonError::host(
                    Code::Refused,
                    "this add-on is made for Stash on the web, which Stash desktop cannot use yet",
                ));
            }
            Some(_) => {
                return Err(AddonError::host(
                    Code::Refused,
                    "this add-on needs a newer Stash",
                ));
            }
            // No `stash` at all: an add-on made for Eclipse and its family, read through the contract's mapping.
            None if crate::eclipse::is_manifest(&given) => crate::eclipse::manifest(&given)?,
            None => {
                return Err(AddonError::host(
                    Code::Refused,
                    "that address does not hold an add-on's manifest",
                ));
            }
        };
        check_manifest(&manifest)?;
        Ok(Self {
            manifest,
            link: url.into(),
            base,
            agent,
        })
    }

    /// An add-on added earlier, from the link and the manifest that were kept. Nothing is asked of the add-on, so
    /// it is there when the computer is offline; the link and the manifest are checked as when they were first read.
    pub fn from_stored(link: &str, manifest: &str) -> Result<Self> {
        let (url, base) = base_of(link)?;
        let manifest: Manifest = serde_json::from_str(manifest).map_err(|e| {
            AddonError::invalid(format!("the manifest that was kept cannot be read: {e}"))
        })?;
        check_manifest(&manifest)?;
        Ok(Self {
            manifest,
            link: url.into(),
            base,
            agent: agent(),
        })
    }

    /// The address of the manifest. It can carry the listener's own key: keep it, do not show it.
    pub fn link(&self) -> &str {
        &self.link
    }

    /// The manifest as JSON, for keeping.
    pub fn manifest_json(&self) -> String {
        serde_json::to_string(&self.manifest).unwrap_or_else(|_| "{}".into())
    }

    /// The server the add-on is on: the part of its link that is safe to show.
    pub fn host(&self) -> String {
        Url::parse(&self.link)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default()
    }

    pub fn offers_resource(&self, resource: &str) -> bool {
        self.manifest.resources.iter().any(|r| r == resource)
    }

    /// Whether songs can be saved from it: an add-on that lists `download`, or one made for Eclipse (whose stream
    /// is the whole file) that does not say no.
    pub fn offers_download(&self) -> bool {
        match self.manifest.dialect {
            Dialect::Eclipse => self.offers_resource("stream") && !self.manifest.no_downloads,
            _ => self.offers_resource("download"),
        }
    }

    /// A file to keep of one of its tracks (contract section 4.5): one progressive file, the best it has within
    /// `want`'s tiers. An Eclipse add-on is asked for its stream, which is the whole file.
    pub fn download(&self, id: &str, want: &StreamRequest) -> Result<StreamInfo> {
        let want = StreamRequest {
            deliveries: vec![Delivery::None],
            ..want.clone()
        };
        if self.manifest.dialect == Dialect::Eclipse {
            if !self.offers_download() {
                return Err(AddonError::host(
                    Code::NotFound,
                    format!("{} does not let its songs be saved", self.manifest.name),
                ));
            }
            return self.stream(id, &want);
        }
        self.offers("download")?;
        let mut url = Url::parse(&format!("{}/download/", self.base))
            .map_err(|e| AddonError::invalid(e.to_string()))?;
        url.path_segments_mut()
            .map_err(|_| AddonError::invalid("the add-on's address has no path"))?
            .pop_if_empty()
            .push(id);
        url.query_pairs_mut()
            .append_pair("tier", want.tier.as_str())
            .append_pair("minTier", want.min_tier.as_str())
            .append_pair("manifest", "none");
        let info: StreamInfo = get_json(&self.agent, url.as_str())?;
        check_stream(&info, &want)?;
        Ok(info)
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
        if self.manifest.dialect == Dialect::Eclipse {
            // An Eclipse add-on is sent only what Eclipse sends.
            url.query_pairs_mut().append_pair("q", &q);
            let mut tracks = crate::eclipse::search(&self.eclipse_get(url.as_str())?)?;
            tracks.truncate(limit.clamp(1, 200) as usize);
            return Ok(tracks);
        }
        url.query_pairs_mut()
            .append_pair("q", &q)
            .append_pair("limit", &limit.clamp(1, 200).to_string());
        let response: SearchResponse = get_json(&self.agent, url.as_str())?;
        Ok(response.tracks)
    }

    /// The add-on's lyrics (contract section 4.6) for one of its own tracks (`id`), else for the song `wanted`.
    /// `None` when it has none. An add-on made for Eclipse is not asked: its lyrics are not part of what is read.
    pub fn lyrics(
        &self,
        id: Option<&str>,
        wanted: &Wanted,
    ) -> Result<Option<crate::lyrics::Lyrics>> {
        self.offers("lyrics")?;
        if self.manifest.dialect == Dialect::Eclipse {
            return Ok(None);
        }
        let mut url = Url::parse(&format!("{}/lyrics", self.base))
            .map_err(|e| AddonError::invalid(e.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            match id {
                Some(id) => {
                    query.append_pair("id", id);
                }
                None => {
                    query
                        .append_pair("title", &wanted.title)
                        .append_pair("artist", &wanted.artist);
                    if let Some(album) = &wanted.album {
                        query.append_pair("album", album);
                    }
                    if let Some(ms) = wanted.duration_ms {
                        query.append_pair("durationMs", &ms.to_string());
                    }
                    if let Some(isrc) = &wanted.isrc {
                        query.append_pair("isrc", isrc);
                    }
                }
            }
        }
        match get_json::<crate::lyrics::Lyrics>(&self.agent, url.as_str()) {
            Ok(found) => Ok(found.checked()),
            Err(e) if e.code == Code::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The add-on's own track for a song the host knows, or `None` when it does not have that song. Which of its
    /// tracks is the same song is the add-on's to decide (the contract's rules); the host checks the two things it
    /// can: a track whose length is off by more than 8 s or 3 % is refused, and so is one the add-on says it cannot
    /// give at `min_tier`.
    pub fn resolve(&self, wanted: &Wanted, min_tier: Option<Tier>) -> Result<Option<Track>> {
        self.offers("resolve")?;
        if self.manifest.dialect == Dialect::Eclipse {
            return self.eclipse_resolve(wanted);
        }
        let mut url = Url::parse(&format!("{}/resolve", self.base))
            .map_err(|e| AddonError::invalid(e.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("title", &wanted.title)
                .append_pair("artist", &wanted.artist);
            if let Some(album) = &wanted.album {
                query.append_pair("album", album);
            }
            if let Some(ms) = wanted.duration_ms {
                query.append_pair("durationMs", &ms.to_string());
            }
            if let Some(isrc) = &wanted.isrc {
                query.append_pair("isrc", isrc);
            }
            if let Some(explicit) = wanted.explicit {
                query.append_pair("explicit", if explicit { "true" } else { "false" });
            }
            if let Some(tier) = min_tier {
                query.append_pair("minTier", tier.as_str());
            }
        }
        let response: ResolveResponse = get_json(&self.agent, url.as_str())?;
        let Some(item) = response.item else {
            return Ok(None);
        };
        checked_item(item, wanted, min_tier).map(Some)
    }

    /// An Eclipse add-on's answer to a request, or what its failure means.
    fn eclipse_get(&self, url: &str) -> Result<serde_json::Value> {
        let fetched = fetch(&self.agent, url)?;
        if !(200..300).contains(&fetched.status) {
            return Err(crate::eclipse::error(
                fetched.status,
                fetched.retry_after,
                fetched.www_authenticate.as_deref(),
                &fetched.body,
            ));
        }
        serde_json::from_str(&fetched.body)
            .map_err(|e| AddonError::invalid(format!("the answer is not JSON: {e}")))
    }

    /// Asks an Eclipse add-on for a song: by its ISRC when that is the only way the add-on has (and the song has
    /// one), else by its words.
    fn eclipse_resolve(&self, wanted: &Wanted) -> Result<Option<Track>> {
        let by_isrc = self.manifest.resolve_by_isrc;
        if by_isrc && wanted.isrc.is_none() {
            return Ok(None);
        }
        let route = if by_isrc { "resolve-isrc" } else { "resolve" };
        let mut url = Url::parse(&format!("{}/{route}", self.base))
            .map_err(|e| AddonError::invalid(e.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            if !by_isrc {
                query
                    .append_pair("title", &wanted.title)
                    .append_pair("artist", &wanted.artist);
            }
            if let Some(isrc) = &wanted.isrc {
                query.append_pair("isrc", isrc);
            }
            if let (false, Some(ms)) = (by_isrc, wanted.duration_ms) {
                query.append_pair("durationMs", &ms.to_string());
            }
        }
        let answer = match self.eclipse_get(url.as_str()) {
            Ok(answer) => answer,
            // Not having the song is an answer, not a failure.
            Err(e) if e.code == Code::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let item = if by_isrc {
            crate::eclipse::resolved_by_isrc(&answer, wanted)
        } else {
            crate::eclipse::resolved(&answer)
        };
        item.map(|item| checked_item(item, wanted, None))
            .transpose()
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
        if self.manifest.dialect == Dialect::Eclipse {
            url.query_pairs_mut()
                .append_pair("quality", &crate::eclipse::quality(&self.manifest, want));
            return self.eclipse_stream(url.as_str(), want);
        }
        let deliveries: Vec<&str> = want.deliveries.iter().map(|d| d.as_str()).collect();
        url.query_pairs_mut()
            .append_pair("tier", want.tier.as_str())
            .append_pair("minTier", want.min_tier.as_str())
            .append_pair("manifest", &deliveries.join(","));
        let info: StreamInfo = get_json(&self.agent, url.as_str())?;
        check_stream(&info, want)?;
        Ok(info)
    }

    fn eclipse_stream(&self, url: &str, want: &StreamRequest) -> Result<StreamInfo> {
        let mut fetched = fetch(&self.agent, url)?;
        // "Still preparing this song": a 503 that asks for a short wait is waited out once.
        if fetched.status == 503
            && let Some(wait) = fetched.retry_after.filter(|s| (1..=30).contains(s))
        {
            std::thread::sleep(Duration::from_secs(wait.min(5)));
            fetched = fetch(&self.agent, url)?;
        }
        if !(200..300).contains(&fetched.status) {
            return Err(crate::eclipse::error(
                fetched.status,
                fetched.retry_after,
                fetched.www_authenticate.as_deref(),
                &fetched.body,
            ));
        }
        let answer: serde_json::Value = serde_json::from_str(&fetched.body)
            .map_err(|e| AddonError::invalid(format!("the answer is not JSON: {e}")))?;
        let streamed = crate::eclipse::stream(&answer)?;
        if !streamed.asked_for(want) {
            return Err(AddonError::host(
                Code::QualityUnavailable,
                "the add-on has it only in a quality that was not asked for",
            ));
        }
        Ok(streamed.info)
    }
}

/// The host's own checks on a track an add-on says is the song asked for: its length within 8 s or 3 %, and (when
/// the add-on says how good it can give it) not below the quality asked for.
fn checked_item(item: Track, wanted: &Wanted, min_tier: Option<Tier>) -> Result<Track> {
    if item.id.is_empty() {
        return Err(AddonError::invalid(
            "the add-on answered a track without an id",
        ));
    }
    if let (Some(asked), Some(theirs)) = (wanted.duration_ms, item.duration_ms) {
        let allowed = 8_000.max(asked * 3 / 100);
        if asked.abs_diff(theirs) > allowed {
            return Err(AddonError::invalid(format!(
                "the add-on answered a track of another length ({theirs} ms for {asked} ms)"
            )));
        }
    }
    if let (Some(least), Some(best)) = (min_tier, item.max_tier)
        && best < least
    {
        return Err(AddonError::invalid(
            "the add-on answered a track below the quality that was asked for",
        ));
    }
    Ok(item)
}

/// The link as a URL, and the address everything else of the add-on is under.
fn base_of(link: &str) -> Result<(Url, String)> {
    let url = checked_url(link, "the add-on link")?;
    let base = url
        .as_str()
        .split('?')
        .next()
        .and_then(|u| u.strip_suffix("/manifest.json"))
        .map(str::to_string);
    match base {
        Some(base) => Ok((url, base)),
        None => Err(AddonError::host(
            Code::BadRequest,
            "an add-on link ends in /manifest.json",
        )),
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        // Within the whole wait, so a name that is not found or a connection that is not made in time is told apart
        // from an add-on that is slow to answer (see `unanswered`).
        .timeout_resolve(Some(RESOLVE_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .max_redirects(0)
        .http_status_as_error(false)
        .user_agent(concat!("Stash-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// A URL an add-on (or the listener's link) names: https, or http on this computer only; no credentials in it.
pub(crate) fn checked_url(text: &str, what: &str) -> Result<Url> {
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

/// A request that got no answer. [`Code::Network`] only when nothing reached the add-on: its server's name was not
/// found, nothing listens there, there is no way to it, or connecting took too long (no connection, or its server is
/// down: the same for every request, and nothing was asked of it). A line that broke once the request was on its way,
/// or an answer that is not HTTP, is the add-on not working: it may have been asked, and have spent something on it.
fn unanswered(e: ureq::Error) -> AddonError {
    use std::io::ErrorKind;
    match e {
        ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::ConnectProxyFailed(_)
        | ureq::Error::Timeout(ureq::Timeout::Resolve | ureq::Timeout::Connect) => {
            AddonError::host(Code::Network, e.to_string())
        }
        // A name that could not be looked up comes as one of these too, with no kind of its own.
        ureq::Error::Io(ref io)
            if !matches!(
                io.kind(),
                ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::WriteZero
                    | ErrorKind::TimedOut
            ) =>
        {
            AddonError::host(Code::Network, e.to_string())
        }
        ureq::Error::Timeout(_) => {
            AddonError::host(Code::Timeout, "the add-on did not answer in time")
        }
        other => AddonError::host(Code::Unavailable, other.to_string()),
    }
}

/// An answer as it came: its status, what it says about waiting and signing in, and its text.
struct Fetched {
    status: u16,
    retry_after: Option<u64>,
    www_authenticate: Option<String>,
    body: String,
}

fn fetch(agent: &ureq::Agent, url: &str) -> Result<Fetched> {
    let mut response = agent.get(url).call().map_err(unanswered)?;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let retry_after = header("retry-after").and_then(|v| v.trim().parse::<u64>().ok());
    let www_authenticate = header("www-authenticate");
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(BODY_CAP)
        .read_to_string()
        .map_err(|e| AddonError::invalid(format!("the answer could not be read: {e}")))?;
    Ok(Fetched {
        status,
        retry_after,
        www_authenticate,
        body,
    })
}

/// GETs `url` and reads a JSON answer of type `T`, or the add-on's error (contract section 7).
fn get_json<T: serde::de::DeserializeOwned>(agent: &ureq::Agent, url: &str) -> Result<T> {
    let Fetched {
        status,
        retry_after: header_retry,
        body,
        ..
    } = fetch(agent, url)?;

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
