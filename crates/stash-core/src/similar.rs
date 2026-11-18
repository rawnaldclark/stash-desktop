//! Similar music from Last.fm: which artists and songs are like the one a radio or autoplay starts from.
//!
//! Stash asks through its own Last.fm proxy, a server that holds the key (this program holds none), and asks with
//! the parameters the phones send, so the proxy can answer both from what it keeps. Here: what is asked, how an
//! answer is read, and which songs are never asked about (tags that say too little, such as "Unknown Artist" or a
//! voice note's file name). Network only: answers are kept by the library (`Library::kept_similar`), and what a
//! station does with them is `station.rs`.

use std::fmt;
use std::net::Ipv4Addr;
use std::sync::LazyLock;
use std::time::Duration;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::{Host, Url, form_urlencoded};

use crate::identity::clean_text;

/// The most similar artists asked for (artist.getsimilar), as the phones ask.
pub const ARTIST_LIMIT: u32 = 100;
/// The most similar songs asked for (track.getsimilar), as the phones ask.
pub const SONG_LIMIT: u32 = 30;
/// An artist's top songs asked for (artist.gettoptracks), as the phones ask for Daily Discover.
pub const TOP_SONGS_LIMIT: u32 = 3;
/// How long a kept answer is fresh, in seconds: 14 days, as long as the proxy itself keeps one.
pub const FRESH_S: i64 = 14 * 86_400;

/// The longest a question waits, from asking to the last byte of the answer.
const TIMEOUT: Duration = Duration::from_secs(8);
/// The most of an answer that is read.
const ANSWER_CAP: u64 = 1 << 20;
/// How alike a song or artist counts when Last.fm does not say.
const NO_MATCH: f32 = 0.05;
/// The longest Last.fm is left alone when it is busy, whatever it asks for.
const MAX_WAIT_S: u64 = 3_600;
/// The longest name or title taken from an answer.
const MAX_NAME: usize = 300;
/// Who asks, as the add-on and ListenBrainz requests say it.
const USER_AGENT: &str = concat!("Stash-desktop/", env!("CARGO_PKG_VERSION"));
/// Which Stash asks, so the proxy's log can tell this program from the phones.
const STASH_VERSION: &str = concat!("desktop/", env!("CARGO_PKG_VERSION"));
/// Trouble in Stash's own words: never the address, and never the network library's text, which may carry it.
const UNREACHABLE: &str = "Last.fm could not be reached";
const TOO_SLOW: &str = "Last.fm took too long to answer";
const UNREADABLE: &str = "Last.fm's answer could not be read";

/// One question for Last.fm: the artists like an artist, or the songs like a song.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ask {
    /// artist.getsimilar.
    Artist { artist: String },
    /// track.getsimilar.
    Song { artist: String, title: String },
    /// artist.gettoptracks: an artist's most played songs (Daily Discover's, `discover.rs`).
    TopSongs { artist: String },
}

impl Ask {
    /// The question as the proxy takes it: the method (in lower case), the artist, the song's title, autocorrect=1
    /// and the limit. These are the parameters the phones send.
    pub fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![("method", self.method().to_string())];
        match self {
            Ask::Artist { artist } | Ask::TopSongs { artist } => {
                params.push(("artist", artist.clone()))
            }
            Ask::Song { artist, title } => {
                params.push(("artist", artist.clone()));
                params.push(("track", title.clone()));
            }
        }
        params.push(("autocorrect", "1".to_string()));
        params.push(("limit", self.limit().to_string()));
        params
    }

    /// The name an answer is kept under on this computer: the parameters sorted by name, as `k=v&…`, with the
    /// values as sent. A "%" or "&" in a value is written %25 or %26, so two questions never share a name.
    pub fn key(&self) -> String {
        let mut params = self.params();
        params.sort_by_key(|(name, _)| *name);
        params
            .iter()
            .map(|(name, value)| {
                let value = value.replace('%', "%25").replace('&', "%26");
                format!("{name}={value}")
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    /// The page on Last.fm of the artist or the song asked about.
    pub fn link(&self) -> String {
        fn part(text: &str) -> String {
            form_urlencoded::byte_serialize(text.trim().as_bytes()).collect()
        }
        match self {
            Ask::Artist { artist } | Ask::TopSongs { artist } => {
                format!("https://www.last.fm/music/{}", part(artist))
            }
            Ask::Song { artist, title } => {
                format!(
                    "https://www.last.fm/music/{}/_/{}",
                    part(artist),
                    part(title)
                )
            }
        }
    }

    /// Whether Last.fm can know what is asked about: a named artist, and for a song a title that is a title
    /// ([`lookable`]).
    pub fn askable(&self) -> bool {
        match self {
            Ask::Artist { artist } | Ask::TopSongs { artist } => named(artist),
            Ask::Song { artist, title } => lookable(title, artist),
        }
    }

    fn method(&self) -> &'static str {
        match self {
            Ask::Artist { .. } => "artist.getsimilar",
            Ask::Song { .. } => "track.getsimilar",
            Ask::TopSongs { .. } => "artist.gettoptracks",
        }
    }

    fn limit(&self) -> u32 {
        match self {
            Ask::Artist { .. } => ARTIST_LIMIT,
            Ask::Song { .. } => SONG_LIMIT,
            Ask::TopSongs { .. } => TOP_SONGS_LIMIT,
        }
    }
}

/// An artist (or one of their songs) Last.fm finds like the one asked about, and how alike, 0 to 1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Like {
    pub artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub score: f32,
}

/// What Last.fm said: what it finds alike, in its order (most alike first), or that it does not know what was
/// asked about.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Found(Vec<Like>),
    NotFound,
}

/// Why there is no answer: not "Last.fm does not know it", but that it could not say just now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// Too many questions: Last.fm (or the proxy) asked to be left alone, for this many seconds when it said.
    Busy { retry_after_s: Option<u64> },
    /// No answer, or one that is not Last.fm's, said in Stash's own words.
    Away(String),
}

impl fmt::Display for Trouble {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trouble::Busy { .. } => f.write_str("Last.fm is busy"),
            Trouble::Away(words) => f.write_str(words),
        }
    }
}

/// Stash's Last.fm proxy: the address every question goes to, and the way there.
#[derive(Clone)]
pub struct Proxy {
    base: Url,
    agent: ureq::Agent,
}

/// The address is never shown, so it stays out of logs.
impl fmt::Debug for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Proxy { .. }")
    }
}

impl Proxy {
    /// The proxy at `base`, the whole address the questions go to (its path included). Only https, or plain http
    /// to this computer (127.0.0.1 or localhost, for a stand-in); never an address with a name or password, a
    /// query or a fragment in it. The refusal never repeats the address.
    pub fn new(base: &str) -> Result<Proxy, String> {
        let base = Url::parse(base.trim())
            .map_err(|_| "The Last.fm proxy address is not a web address".to_string())?;
        let here = match base.host() {
            Some(Host::Domain(name)) => name == "localhost",
            Some(Host::Ipv4(ip)) => ip == Ipv4Addr::LOCALHOST,
            _ => false,
        };
        let safe = match base.scheme() {
            "https" => true,
            "http" => here,
            _ => false,
        };
        if !safe {
            return Err(
                "The Last.fm proxy address must be https (plain http only on this computer)".into(),
            );
        }
        if base.host_str().is_none_or(str::is_empty) {
            return Err("The Last.fm proxy address names no server".into());
        }
        if !base.username().is_empty() || base.password().is_some() {
            return Err("The Last.fm proxy address must not carry a name or password".into());
        }
        if base.query().is_some() || base.fragment().is_some() {
            return Err("The Last.fm proxy address must not carry a query or a fragment".into());
        }
        // Every answer is read whatever its status, a redirect is not followed, and nothing waits long.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .into();
        Ok(Proxy { base, agent })
    }

    /// Asks Last.fm `ask`, once (no second try). Nothing is sent for what Last.fm cannot know ([`Ask::askable`]):
    /// that is "not found", as the proxy itself would answer.
    pub fn ask(&self, ask: &Ask) -> Result<Answer, Trouble> {
        if !ask.askable() {
            return Ok(Answer::NotFound);
        }
        let url = self.address(ask);
        let sent = self
            .agent
            .get(url.as_str())
            .header("X-Stash-Version", STASH_VERSION)
            .call();
        let mut answer = match sent {
            Ok(answer) => answer,
            Err(ureq::Error::Timeout(_)) => return Err(Trouble::Away(TOO_SLOW.into())),
            Err(_) => return Err(Trouble::Away(UNREACHABLE.into())),
        };
        let status = answer.status().as_u16();
        let retry_after = answer
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = match answer
            .body_mut()
            .with_config()
            .limit(ANSWER_CAP)
            .read_to_string()
        {
            Ok(body) => body,
            // A refusal is read from its status alone.
            Err(_) if !(200..300).contains(&status) => String::new(),
            Err(_) => return Err(Trouble::Away(UNREADABLE.into())),
        };
        read(ask, status, retry_after.as_deref(), &body)
    }

    /// The address `ask` is sent to: the proxy's, with the question as its query (each value encoded).
    fn address(&self, ask: &Ask) -> Url {
        let mut url = self.base.clone();
        {
            let mut query = url.query_pairs_mut();
            for (name, value) in ask.params() {
                query.append_pair(name, &value);
            }
        }
        url
    }
}

/// Reads an answer to `ask` as the proxy passes Last.fm's on: its HTTP `status`, its Retry-After header, and its
/// body.
///
/// - track.getsimilar answers `{"similartracks":{"track":[{"name","match","artist":{"name"}}…]}}`, and
///   artist.getsimilar `{"similarartists":{"artist":[{"name","match"}…]}}`. The list may be one object instead,
///   or missing or empty (nothing alike). `match` may be a number or a string; it is kept to 0..1, and a missing
///   one counts as 0.05. An entry with no name is left out.
/// - Last.fm's error 6 or 7 (in a 200 or a 4xx) is "not found"; error 29, or HTTP 429, is busy. Any other error,
///   any other status, and a body that is not Last.fm's answer (a web page, nothing) is trouble.
pub fn read(
    ask: &Ask,
    status: u16,
    retry_after: Option<&str>,
    body: &str,
) -> Result<Answer, Trouble> {
    if status == 429 {
        return Err(Trouble::Busy {
            retry_after_s: wait(retry_after),
        });
    }
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if matches!(status, 200..=299 | 400..=499)
        && let Some(code) = error_code(&json)
    {
        return match code {
            6 | 7 => Ok(Answer::NotFound),
            29 => Err(Trouble::Busy {
                retry_after_s: wait(retry_after),
            }),
            code => Err(Trouble::Away(format!("Last.fm answered error {code}"))),
        };
    }
    // Last.fm says "not found" with a 400 or a 404 (and error 6 in the body), which Stash's proxy passes on as a 502
    // that keeps only the status: Last.fm does not know what was asked. Any other status it passes on (a 403 is a key
    // the proxy holds that Last.fm refuses) is trouble.
    if status == 502
        && json.get("error").and_then(Value::as_str) == Some("upstream_error")
        && matches!(json.get("status").and_then(Value::as_u64), Some(400 | 404))
    {
        return Ok(Answer::NotFound);
    }
    if !(200..=299).contains(&status) {
        return Err(Trouble::Away(format!("Last.fm answered {status}")));
    }
    if body.trim().is_empty() {
        return Err(Trouble::Away("Last.fm answered nothing".into()));
    }
    let (holder, list) = match ask {
        Ask::Song { .. } => ("similartracks", "track"),
        Ask::Artist { .. } => ("similarartists", "artist"),
        Ask::TopSongs { .. } => ("toptracks", "track"),
    };
    let Some(holder) = json.get(holder) else {
        return Err(Trouble::Away(UNREADABLE.into()));
    };
    let entries: Vec<&Value> = match holder.get(list) {
        Some(Value::Array(all)) => all.iter().collect(),
        Some(one) if one.is_object() => vec![one],
        _ => Vec::new(),
    };
    Ok(Answer::Found(
        entries
            .into_iter()
            .filter_map(|entry| like(ask, entry))
            .take(ask.limit() as usize)
            .collect(),
    ))
}

/// One entry of an answer, if it names an artist (and, for songs, a title).
fn like(ask: &Ask, entry: &Value) -> Option<Like> {
    let name = text(entry.get("name"))?;
    let score = score(entry.get("match"));
    match ask {
        Ask::Artist { .. } => Some(Like {
            artist: name,
            title: None,
            score,
        }),
        Ask::Song { .. } | Ask::TopSongs { .. } => {
            // track.getsimilar names the artist as {"name": …}; a plain name is taken too.
            let artist = entry.get("artist").and_then(|a| {
                if a.is_object() {
                    a.get("name")
                } else {
                    Some(a)
                }
            });
            Some(Like {
                artist: text(artist)?,
                title: Some(name),
                score,
            })
        }
    }
}

/// A name or title from an answer, made safe to show, if there is one.
fn text(value: Option<&Value>) -> Option<String> {
    let text = match value? {
        Value::String(s) => clean_text(s, MAX_NAME),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// How alike, from a number or a string, kept to 0..1; 0.05 when it is missing or not a number.
fn score(value: Option<&Value>) -> f32 {
    let given = match value {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    match given {
        Some(m) if !m.is_nan() => m.clamp(0.0, 1.0) as f32,
        _ => NO_MATCH,
    }
}

/// Last.fm's error number in an answer (`{"error": 6, "message": …}`), as a number or a string of digits. The
/// proxy's own refusals name theirs in words ("not_found"), so they are never taken for Last.fm's.
fn error_code(json: &Value) -> Option<i64> {
    match json.get("error")? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The seconds a Retry-After header asks for (a date is not read), at most an hour.
fn wait(retry_after: Option<&str>) -> Option<u64> {
    retry_after?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|s| s.min(MAX_WAIT_S))
}

// ---- What is never asked (a port of the Android app's LastFmLookupRules)

/// Artist names a tagger or the phone writes when there is none. Lower case.
const PLACEHOLDER_ARTISTS: [&str; 3] = ["unknown artist", "<unknown>", "unknown"];

/// One of the app's patterns, read as Java reads it: letters in any case, and `\s` and `\d` only ASCII spaces and
/// digits.
fn rule(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .unicode(false)
        .build()
        .expect("a valid pattern")
}

/// A camera, messenger or recorder file name: `AUD-20240101-WA0001`, `PTT-…`, `VID-…`, `REC_20240101_101500`,
/// `Recording_12`. At the start, and followed by a digit, so "Recording Angel" and "Audition" are songs.
static FILE_NAME_TITLE: LazyLock<Regex> =
    LazyLock::new(|| rule(r"^(?:aud|ptt|vid|img|rec|recording|voice)[-_ ]?\d"));
/// A WhatsApp media suffix anywhere: `-WA0001`.
static WHATSAPP_SUFFIX: LazyLock<Regex> = LazyLock::new(|| rule(r"-WA\d{3,}"));
/// A title that is a file name: it ends in an audio or video file's extension.
static FILE_EXTENSION: LazyLock<Regex> = LazyLock::new(|| {
    rule(r"\.(?:mp3|m4a|aac|ogg|oga|opus|wav|flac|amr|3gp|3ga|mp4|m4b|wma|webm|mka|aiff?)$")
});
/// Only digits and separators (a timestamp has at least eight digits: see [`numeric_title`]).
static DIGITS_AND_SEPARATORS: LazyLock<Regex> = LazyLock::new(|| rule(r"^[\d\s._-]+$"));
/// What video uploads add to a title and Last.fm's names do not: "(Official Audio)", "[Official Music Video]",
/// "(Lyric Video)", "(Official Visualizer)", "(Official HD Video)", "(Audio)", "(Lyrics)". Only these: "(Remix)",
/// "(Live)", "(Acoustic)" and the like name another version, so they stay.
static PRESENTATION_TAG: LazyLock<Regex> = LazyLock::new(|| {
    rule(concat!(
        r"\s*[(\[]\s*(?:official\s+)?(?:hd\s+|4k\s+)?(?:music\s+video|lyrics?\s+video|lyric\s+visuali[sz]er|",
        r"visuali[sz]er|video|audio|lyrics?)(?:\s+(?:hd|4k))?\s*[)\]]",
    ))
});
/// Two spaces or more.
static SPACES: LazyLock<Regex> = LazyLock::new(|| rule(r"\s{2,}"));
/// "Artist - Topic", the name of YouTube's channels made for an artist.
static TOPIC_SUFFIX: LazyLock<Regex> = LazyLock::new(|| rule(r"\s+-\s+topic$"));

/// Whether Last.fm can be asked about the song `title` by `artist`. Not when either is blank, the artist is a
/// placeholder ("Unknown Artist", "<unknown>", "unknown"), or the title is a file's name: a camera, messenger or
/// recorder file (AUD, PTT, VID, IMG, REC, Recording or Voice, then a digit), a WhatsApp suffix (-WA and three
/// digits or more), an audio or video file's extension, or a timestamp (only digits and separators, at least
/// eight digits: "1979" is a song).
pub fn lookable(title: &str, artist: &str) -> bool {
    let title = title.trim();
    named(artist)
        && !title.is_empty()
        && !FILE_NAME_TITLE.is_match(title)
        && !WHATSAPP_SUFFIX.is_match(title)
        && !FILE_EXTENSION.is_match(title)
        && !numeric_title(title)
}

/// An artist that names someone: not blank, and not a placeholder.
fn named(artist: &str) -> bool {
    let artist = artist.trim();
    !artist.is_empty() && !PLACEHOLDER_ARTISTS.contains(&artist.to_lowercase().as_str())
}

/// A timestamp, not a title: only digits and separators, with at least eight digits ("20240101_101500").
fn numeric_title(title: &str) -> bool {
    DIGITS_AND_SEPARATORS.is_match(title) && title.bytes().filter(u8::is_ascii_digit).count() >= 8
}

/// `title` without what video uploads add to it ("(Official Audio)", "[Official Music Video]", …), as Last.fm
/// lists the song. The title as it was (trimmed) when that would leave nothing.
pub fn clean_title(title: &str) -> String {
    let cleaned = PRESENTATION_TAG.replace_all(title, "");
    let cleaned = SPACES.replace_all(&cleaned, " ");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        title.trim().to_string()
    } else {
        cleaned.to_string()
    }
}

/// `artist` without the " - Topic" of YouTube's channels made for an artist.
pub fn clean_artist(artist: &str) -> String {
    let artist = artist.trim();
    let cleaned = TOPIC_SUFFIX.replace_all(artist, "");
    if cleaned.is_empty() {
        artist.to_string()
    } else {
        cleaned.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    use super::*;

    fn teardrop() -> Ask {
        Ask::Song {
            artist: "Massive Attack".into(),
            title: "Teardrop".into(),
        }
    }

    fn massive_attack() -> Ask {
        Ask::Artist {
            artist: "Massive Attack".into(),
        }
    }

    fn like(artist: &str, title: Option<&str>, score: f32) -> Like {
        Like {
            artist: artist.into(),
            title: title.map(str::to_string),
            score,
        }
    }

    /// The likes of a found answer, with each score rounded to a thousandth (so "0.83" and 0.83 compare).
    fn found(answer: Result<Answer, Trouble>) -> Vec<(String, Option<String>, i32)> {
        match answer {
            Ok(Answer::Found(likes)) => likes
                .into_iter()
                .map(|l| (l.artist, l.title, (l.score * 1000.0).round() as i32))
                .collect(),
            other => panic!("not found answer: {other:?}"),
        }
    }

    fn away(answer: Result<Answer, Trouble>) -> String {
        match answer {
            Err(Trouble::Away(words)) => words,
            other => panic!("not trouble: {other:?}"),
        }
    }

    #[test]
    fn similar_reads_last_fms_shapes() {
        let tracks = r#"{"similartracks":{"track":[
            {"name":"Glory Box","match":"0.83","artist":{"name":"Portishead","mbid":"x"}},
            {"name":"Roads","match":10.95,"artist":{"name":"Portishead"}},
            {"name":"Unfinished Sympathy","artist":{"name":"Massive Attack"}},
            {"name":"  ","match":0.5,"artist":{"name":"Nobody"}},
            {"name":"No One's","match":0.5,"artist":{"name":""}},
            {"name":"No Artist","match":0.5},
            {"name":"Angel","match":-1,"artist":"Massive Attack"},
            {"name":"Overcome","match":"not a number","artist":{"name":"Tricky"}}
        ],"@attr":{"artist":"Massive Attack"}}}"#;
        let row = |artist: &str, title: &str, score: i32| {
            (artist.to_string(), Some(title.to_string()), score)
        };
        assert_eq!(
            found(read(&teardrop(), 200, None, tracks)),
            [
                row("Portishead", "Glory Box", 830),
                row("Portishead", "Roads", 1000),
                row("Massive Attack", "Unfinished Sympathy", 50),
                row("Massive Attack", "Angel", 0),
                row("Tricky", "Overcome", 50),
            ]
        );
        // One song alone is an object, not a list.
        let one = r#"{"similartracks":{"track":{"name":"Glory Box","match":0.83,"artist":{"name":"Portishead"}}}}"#;
        assert_eq!(
            found(read(&teardrop(), 200, None, one)),
            [row("Portishead", "Glory Box", 830)]
        );

        let artists = r#"{"similarartists":{"artist":[
            {"name":"Portishead","match":"1"},{"name":"Tricky","match":0.83},{"name":""},{"match":"0.5"}
        ],"@attr":{"artist":"Massive Attack"}}}"#;
        assert_eq!(
            found(read(&massive_attack(), 200, None, artists)),
            [
                ("Portishead".to_string(), None, 1000),
                ("Tricky".to_string(), None, 830)
            ]
        );
        let one = r#"{"similarartists":{"artist":{"name":"Tricky","match":"0.83"}}}"#;
        assert_eq!(
            found(read(&massive_attack(), 200, None, one)),
            [("Tricky".to_string(), None, 830)]
        );
        // Nothing alike: an empty list, or none at all.
        for nothing in [
            r#"{"similarartists":{"artist":[],"@attr":{"artist":"X"}}}"#,
            r##"{"similarartists":{"#text":"\n","@attr":{"artist":"X"}}}"##,
            r#"{"similarartists":{"artist":"   "}}"#,
        ] {
            assert_eq!(
                read(&massive_attack(), 200, None, nothing),
                Ok(Answer::Found(Vec::new())),
                "{nothing}"
            );
        }
        // No more than was asked for is taken.
        let many = format!(
            r#"{{"similartracks":{{"track":[{}]}}}}"#,
            vec![r#"{"name":"Song","match":0.5,"artist":{"name":"Someone"}}"#; 40].join(",")
        );
        assert_eq!(
            found(read(&teardrop(), 200, None, &many)).len(),
            SONG_LIMIT as usize
        );
        // The answer to the other question is not this one's.
        assert!(matches!(
            read(&teardrop(), 200, None, artists),
            Err(Trouble::Away(_))
        ));

        // Last.fm's errors: 6 and 7 are "not found", in a 200 or a 4xx, as a number or a string.
        let error = |code: &str| format!(r#"{{"error":{code},"message":"Something","links":[]}}"#);
        for (status, code) in [
            (200, "6"),
            (200, "7"),
            (400, "6"),
            (404, "7"),
            (200, "\"6\""),
        ] {
            assert_eq!(
                read(&teardrop(), status, None, &error(code)),
                Ok(Answer::NotFound),
                "{status} {code}"
            );
        }
        // 29 and HTTP 429 are busy, with the wait asked for (at most an hour; a date is not read).
        assert_eq!(
            read(&teardrop(), 200, Some("300"), &error("29")),
            Err(Trouble::Busy {
                retry_after_s: Some(300)
            })
        );
        assert_eq!(
            read(&teardrop(), 429, Some(" 60 "), &error("29")),
            Err(Trouble::Busy {
                retry_after_s: Some(60)
            })
        );
        for (header, wait) in [
            (None, None),
            (Some("Wed, 21 Oct 2026 07:28:00 GMT"), None),
            (Some("999999"), Some(3_600)),
        ] {
            assert_eq!(
                read(&teardrop(), 429, header, ""),
                Err(Trouble::Busy {
                    retry_after_s: wait
                })
            );
        }
        // Other errors may not hold a minute later: trouble, never kept.
        for code in ["8", "11", "16", "26"] {
            assert_eq!(
                away(read(&teardrop(), 200, None, &error(code))),
                format!("Last.fm answered error {code}")
            );
        }
        // Last.fm's "not found" comes as a 400 or a 404, which the proxy passes on as a 502 with that status.
        for upstream in ["400", "404"] {
            let body = format!(r#"{{"error":"upstream_error","status":{upstream}}}"#);
            assert_eq!(read(&teardrop(), 502, None, &body), Ok(Answer::NotFound));
        }
        // The proxy's own refusals and Last.fm's troubles are trouble too, never "not found": a key Last.fm refuses
        // (403), Last.fm down (5xx), or a 502 that says nothing of Last.fm's status.
        let not_found = error("6");
        for (status, body) in [
            (502, r#"{"error":"upstream_error","status":403}"#),
            (502, r#"{"error":"upstream_error","status":503}"#),
            (502, r#"{"error":"upstream_error"}"#),
            (
                502,
                r#"{"error":"upstream_unreachable","message":"Upstream service unreachable"}"#,
            ),
            (502, not_found.as_str()),
            (404, r#"{"error":"not_found"}"#),
            (405, r#"{"error":"method_not_allowed"}"#),
            (400, r#"{"error":"forbidden_param","message":"no"}"#),
            (500, r#"{"error":"server_misconfigured","message":"no"}"#),
            (503, "<html><body>Service Unavailable</body></html>"),
            (301, ""),
        ] {
            assert_eq!(
                away(read(&teardrop(), status, None, body)),
                format!("Last.fm answered {status}")
            );
        }
        // A page that is not Last.fm's answer (a hotel's sign-in page), and nothing at all.
        for body in [
            "<!doctype html><title>Sign in</title>",
            "",
            "  \n",
            "{}",
            "[]",
            "null",
            r#"{"error":"x"}"#,
        ] {
            assert!(
                matches!(read(&teardrop(), 200, None, body), Err(Trouble::Away(_))),
                "{body}"
            );
        }
    }

    #[test]
    fn a_kept_answer_is_its_likes_as_json() {
        let likes = vec![
            like("Portishead", None, 0.5),
            like("Tricky", Some("Overcome"), 0.25),
        ];
        let kept = serde_json::to_string(&likes).unwrap();
        assert_eq!(
            kept,
            r#"[{"artist":"Portishead","score":0.5},{"artist":"Tricky","title":"Overcome","score":0.25}]"#
        );
        assert_eq!(serde_json::from_str::<Vec<Like>>(&kept).unwrap(), likes);
    }

    #[test]
    fn asks_send_what_the_phones_send() {
        fn pairs(list: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
            list.iter().map(|(k, v)| (*k, v.to_string())).collect()
        }
        assert_eq!(
            teardrop().params(),
            pairs(&[
                ("method", "track.getsimilar"),
                ("artist", "Massive Attack"),
                ("track", "Teardrop"),
                ("autocorrect", "1"),
                ("limit", "30"),
            ])
        );
        assert_eq!(
            massive_attack().params(),
            pairs(&[
                ("method", "artist.getsimilar"),
                ("artist", "Massive Attack"),
                ("autocorrect", "1"),
                ("limit", "100"),
            ])
        );

        // The kept name: sorted, the same each time, and never the key or the format.
        let key = teardrop().key();
        assert_eq!(
            key,
            "artist=Massive Attack&autocorrect=1&limit=30&method=track.getsimilar&track=Teardrop"
        );
        assert_eq!(teardrop().key(), key);
        assert_eq!(
            massive_attack().key(),
            "artist=Massive Attack&autocorrect=1&limit=100&method=artist.getsimilar"
        );
        for ask in [teardrop(), massive_attack()] {
            let key = ask.key();
            assert!(!key.contains("api_key") && !key.contains("format"), "{key}");
        }
        // A value can never read as more parameters, so two questions never share a name.
        let song = |artist: &str, title: &str| Ask::Song {
            artist: artist.into(),
            title: title.into(),
        };
        let crafted = song(
            "A&autocorrect=1&limit=30&method=track.getsimilar&track=B",
            "T",
        );
        let other = song(
            "A",
            "B&autocorrect=1&limit=30&method=track.getsimilar&track=T",
        );
        assert_ne!(crafted.key(), other.key());
        assert_ne!(song("100%", "&").key(), song("100%26", "%").key());
        assert!(
            song("Simon & Garfunkel", "Mrs. Robinson")
                .key()
                .starts_with("artist=Simon %26 Garfunkel&")
        );

        // The page on Last.fm, every part encoded.
        assert_eq!(
            teardrop().link(),
            "https://www.last.fm/music/Massive+Attack/_/Teardrop"
        );
        assert_eq!(
            Ask::Artist {
                artist: "Simon & Garfunkel".into()
            }
            .link(),
            "https://www.last.fm/music/Simon+%26+Garfunkel"
        );
        assert_eq!(
            song("Sigur Rós", "Hoppípolla").link(),
            "https://www.last.fm/music/Sigur+R%C3%B3s/_/Hopp%C3%ADpolla"
        );
        assert_eq!(
            song("AC/DC", "What?#1+1").link(),
            "https://www.last.fm/music/AC%2FDC/_/What%3F%231%2B1"
        );
    }

    #[test]
    fn unlookable_songs_are_never_asked() {
        // The spec's own.
        assert!(!lookable("", "Massive Attack"));
        assert!(!lookable("   ", "Massive Attack"));
        assert!(!lookable("Teardrop", ""));
        assert!(!lookable("Teardrop", "Unknown Artist"));
        assert!(!lookable("Teardrop", "<unknown>"));
        for title in [
            "AUD-20231004-WA0012",
            "PTT-20240101",
            "VID_20240101",
            "track01.mp3",
            "20231004 123456",
        ] {
            assert!(!lookable(title, "Massive Attack"), "{title}");
        }
        assert!(lookable("1979", "The Smashing Pumpkins"));
        assert!(lookable("Teardrop", "Massive Attack"));

        // The Android app's own (LastFmLookupRulesTest), rule for rule.
        for title in [
            "AUD-20240101-WA0001",
            "PTT-20231212-WA0042",
            "VID-20230505-WA0007",
            "REC_20240101_101500",
            "Recording_12",
            "Voice 001",
            "some clip-WA0003",
            "my song.mp3",
            "Track 01.M4A",
            "voice note.opus",
            "20240101_101500",
            "IMG 4",
            "clip.webm",
        ] {
            assert!(!lookable(title, "Some Artist"), "{title}");
        }
        for artist in [
            "Unknown Artist",
            "<unknown>",
            "unknown",
            "  UNKNOWN ARTIST ",
            "",
        ] {
            assert!(!lookable("Real Song", artist), "{artist}");
        }
        for title in [
            "Audition",
            "Recording Angel",
            "Video Games",
            "Voices",
            "1979",
            "2112",
            "99 Luftballons",
            "Track 01",
            "Paint a Face",
            "Mp3 Player",
            "1234567",
            "2024-01-01 Live",
        ] {
            assert!(lookable(title, "Some Artist"), "{title}");
        }
        assert!(lookable("Song", "The Unknown Artists Band"));

        // What an artist radio asks about.
        assert!(massive_attack().askable());
        assert!(
            !Ask::Artist {
                artist: " Unknown ".into()
            }
            .askable()
        );
        assert!(
            !Ask::Song {
                artist: "Unknown artist".into(),
                title: "AUD-20231004-WA0012".into()
            }
            .askable()
        );

        // Titles are asked about as Last.fm lists them; versions stay.
        assert_eq!(clean_title("Teardrop (Official Audio)"), "Teardrop");
        assert_eq!(clean_title("Teardrop (Live)"), "Teardrop (Live)");
        for (raw, clean) in [
            ("Bad Guy (Official Audio)", "Bad Guy"),
            ("Bad Guy (Official Video)", "Bad Guy"),
            ("Bad Guy [Official Music Video]", "Bad Guy"),
            ("Bad Guy (Official Lyric Video)", "Bad Guy"),
            ("Bad Guy (Lyric Video)", "Bad Guy"),
            ("Bad Guy (Official Visualizer)", "Bad Guy"),
            ("Bad Guy (Visualiser)", "Bad Guy"),
            ("Bad Guy (Official HD Video)", "Bad Guy"),
            ("Bad Guy (Audio)", "Bad Guy"),
            ("Bad Guy (Lyrics)", "Bad Guy"),
            ("Bad Guy (official music video) (Lyrics)", "Bad Guy"),
            ("Bad Guy (Remix) (Official Video)", "Bad Guy (Remix)"),
            ("Bad Guy  [Official Music Video]  Extra", "Bad Guy Extra"),
            ("  Bad Guy (Video 4K) ", "Bad Guy"),
        ] {
            assert_eq!(clean_title(raw), clean, "{raw}");
        }
        for title in [
            "Bad Guy (Remix)",
            "Bad Guy (Live)",
            "Bad Guy (Acoustic)",
            "Bad Guy (feat. Justin Bieber)",
            "Video Killed the Radio Star",
            "Audio Love",
        ] {
            assert_eq!(clean_title(title), title);
        }
        // A title that is only a tag is left as it was.
        assert_eq!(clean_title(" (Official Video) "), "(Official Video)");

        assert_eq!(clean_artist("Massive Attack - Topic"), "Massive Attack");
        assert_eq!(clean_artist("  Billie Eilish - TOPIC "), "Billie Eilish");
        assert_eq!(clean_artist("Topic"), "Topic");
        assert_eq!(clean_artist("Billie Eilish"), "Billie Eilish");
        assert_eq!(clean_artist("Topic - Topic"), "Topic");
    }

    #[test]
    fn a_proxy_address_must_be_safe_to_use() {
        for usable in [
            "https://proxy.example/lastfm",
            "  https://proxy.example:8443/lastfm  ",
            "http://127.0.0.1:8080/lastfm",
            "http://localhost:9/lastfm",
            "HTTP://LOCALHOST/lastfm",
        ] {
            assert!(Proxy::new(usable).is_ok(), "{usable}");
        }
        for refused in [
            "http://proxy.example/lastfm",
            "http://192.168.1.20/lastfm",
            "http://127.0.0.2/lastfm",
            "https://proxy.example/lastfm?token=1",
            "https://proxy.example/lastfm?",
            "https://proxy.example/lastfm#top",
            "https://me:secret@proxy.example/lastfm",
            "https://me@proxy.example/lastfm",
            "http://me@127.0.0.1/lastfm",
            "ftp://proxy.example/lastfm",
            "file:///C:/lastfm",
            "javascript:alert(1)",
            "proxy.example/lastfm",
            "not an address",
            "",
        ] {
            let Err(words) = Proxy::new(refused) else {
                panic!("taken: {refused}");
            };
            for shown in ["proxy.example", "127.0.0", "secret"] {
                assert!(!words.contains(shown), "{words}");
            }
        }
        // The address is never shown, not even to a log.
        let proxy = Proxy::new("http://127.0.0.1:9/lastfm").unwrap();
        assert!(!format!("{proxy:?}").contains("127.0.0.1"));
    }

    #[test]
    fn the_question_is_the_query_of_the_proxys_address() {
        let proxy = Proxy::new("https://proxy.example/lastfm").unwrap();
        let ask = Ask::Song {
            artist: "Simon & Garfunkel".into(),
            title: "Mrs. Robinson".into(),
        };
        let url = proxy.address(&ask);
        assert_eq!(
            (url.host_str(), url.path()),
            (Some("proxy.example"), "/lastfm")
        );
        assert_eq!(
            url.query(),
            Some(concat!(
                "method=track.getsimilar&artist=Simon+%26+Garfunkel&track=Mrs.+Robinson",
                "&autocorrect=1&limit=30"
            ))
        );
        let sent: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        let asked: Vec<(String, String)> = ask
            .params()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        assert_eq!(sent, asked);
        // Asking again starts from the proxy's own address.
        assert_eq!(proxy.address(&massive_attack()).query_pairs().count(), 4);
        // Nothing is sent for what Last.fm cannot know (nothing listens at port 9).
        let junk = Ask::Song {
            artist: "Unknown Artist".into(),
            title: "AUD-20231004-WA0012".into(),
        };
        assert_eq!(
            Proxy::new("http://127.0.0.1:9/lastfm").unwrap().ask(&junk),
            Ok(Answer::NotFound)
        );
    }

    /// A stand-in for the proxy on this computer, for one request: it gives `answer` (or nothing, and hangs up),
    /// and hands back what the request said (its first line and headers).
    fn stand_in(answer: String) -> (Proxy, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let proxy = Proxy::new(&format!("http://127.0.0.1:{port}/lastfm")).unwrap();
        let seen = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head: Vec<u8> = Vec::new();
            let mut buf = [0u8; 4096];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            let _ = stream.write_all(answer.as_bytes());
            let _ = stream.flush();
            String::from_utf8_lossy(&head).to_lowercase()
        });
        (proxy, seen)
    }

    /// An HTTP answer: its status line, `headers` (each ending in CRLF), and `body`.
    fn http(status: &str, headers: &str, body: &str) -> String {
        let length = body.len();
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Type: application/json\r\nContent-Length: {length}\r\n\
             Connection: close\r\n\r\n{body}"
        )
    }

    #[test]
    fn one_get_with_stashs_own_headers_is_sent() {
        let body = r#"{"similarartists":{"artist":[{"name":"Portishead","match":"1"},{"name":"Tricky","match":"0.5"}]}}"#;
        let (proxy, seen) = stand_in(http("200 OK", "X-Stash-Cache: HIT\r\n", body));
        assert_eq!(
            proxy.ask(&massive_attack()),
            Ok(Answer::Found(vec![
                like("Portishead", None, 1.0),
                like("Tricky", None, 0.5)
            ]))
        );
        let head = seen.join().unwrap();
        assert!(
            head.starts_with(
                "get /lastfm?method=artist.getsimilar&artist=massive+attack&autocorrect=1&limit=100 http/1.1\r\n"
            ),
            "{head}"
        );
        assert!(head.contains("\r\nuser-agent: stash-desktop/"), "{head}");
        assert!(head.contains("\r\nx-stash-version: desktop/"), "{head}");
        assert!(!head.contains("api_key"), "{head}");
    }

    #[test]
    fn busy_redirected_and_unanswered_questions_are_trouble_in_stashs_words() {
        let (proxy, seen) = stand_in(http(
            "429 Too Many Requests",
            "Retry-After: 60\r\n",
            r#"{"error":29,"message":"Rate Limit Exceeded (upstream)"}"#,
        ));
        assert_eq!(
            proxy.ask(&teardrop()),
            Err(Trouble::Busy {
                retry_after_s: Some(60)
            })
        );
        seen.join().unwrap();

        // A redirect is not followed: the question goes only where it was sent.
        let moved = http(
            "302 Found",
            "Location: http://127.0.0.1:9/elsewhere\r\n",
            "",
        );
        let (proxy, seen) = stand_in(moved);
        assert_eq!(away(proxy.ask(&teardrop())), "Last.fm answered 302");
        seen.join().unwrap();

        // No answer at all: the words are Stash's own, never the address.
        let (proxy, seen) = stand_in(String::new());
        let words = away(proxy.ask(&teardrop()));
        seen.join().unwrap();
        assert_eq!(words, UNREACHABLE);

        // An answer too long to read is trouble, not an answer.
        let huge = format!(
            r#"{{"similarartists":{{"artist":[{}]}}}}"#,
            vec![r#"{"name":"Someone","match":"0.5"}"#; 40_000].join(",")
        );
        assert!(huge.len() as u64 > ANSWER_CAP);
        let (proxy, seen) = stand_in(http("200 OK", "", &huge));
        assert_eq!(away(proxy.ask(&massive_attack())), UNREADABLE);
        seen.join().unwrap();
    }
}
