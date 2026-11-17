//! YouTube's own API ("InnerTube"), spoken the way its apps speak it: a JSON `context` naming the app that asks, sent
//! to `music.youtube.com` for everything about music (search, pages) and to `www.youtube.com` for where a song plays
//! from. An id YouTube hands out to any visitor is sent, so that it answers the same visitor the same way; and, when the
//! listener signed in and the request is one asked as them ([`Signed`]), their login ([`super::login`]), to YouTube
//! Music's own address only.
//!
//! The apps named here are the ones Stash for Android asks as (its `InnerTubeClient.kt`): YouTube Music on the web
//! for music, and four of YouTube's own apps whose answers carry plain addresses for the sound.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::login::{Login, ORIGIN};
use crate::addons::{AddonError, Code};

/// Who the built-in source is, in the listener's words.
pub(crate) const WHO: &str = "YouTube";

/// YouTube Music's web key: public, the same for every visitor of music.youtube.com.
const MUSIC_KEY: &str = "AIzaSyC9XL3ZjWddXya6X74dJoCTL-WEYFDNX30";
const MUSIC: &str = "https://music.youtube.com/youtubei/v1";
const WWW: &str = "https://www.youtube.com/youtubei/v1";

/// The longest answer read: a search is about a quarter of a megabyte, a long playlist's page three.
const BODY_CAP: u64 = 16 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(12);
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// How long YouTube is left alone after it answers "too many requests".
const AFTER_TOO_MANY: Duration = Duration::from_secs(60);
/// How long after it asks whether the listener is a bot (the web player's measure: asking on makes it ask longer).
pub(crate) const AFTER_BOT_CHECK: Duration = Duration::from_secs(240);
/// What is said while YouTube is left alone for asking whether the listener is a bot (the start of it: yt-dlp may get
/// past that, where it could not get past "too many requests").
pub(crate) const BOT_CHECK: &str = "YouTube is checking that you are not a bot";
/// The longest visitor id kept.
const VISITOR_CAP: usize = 512;

/// One of YouTube's apps, as a request names it.
pub(crate) struct Client {
    client_name: &'static str,
    /// Empty for the web app, whose version is the day's date.
    version: &'static str,
    user_agent: &'static str,
    /// The app's number, as the `X-YouTube-Client-Name` header gives it.
    number: &'static str,
    device: &'static [(&'static str, &'static str)],
    android_sdk: Option<u32>,
    /// Asked on music.youtube.com with the web key; the others on www.youtube.com with none.
    music: bool,
}

/// YouTube Music on the web: search and pages. Its addresses for the sound are scrambled, so it is never asked for
/// them.
pub(crate) const WEB_REMIX: Client = Client {
    client_name: "WEB_REMIX",
    version: "",
    user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    number: "67",
    device: &[],
    android_sdk: None,
    music: true,
};

/// The apps asked where a song plays from, in the order they are tried (the one that last gave a stream goes first).
pub(crate) const PLAYERS: [Client; 4] = [
    Client {
        client_name: "ANDROID_VR",
        version: "1.60.19",
        user_agent: "com.google.android.apps.youtube.vr.oculus/1.60.19 (Linux; U; Android 12L; eureka-user Build/SQ3A.220705.001.B1) gzip",
        number: "28",
        device: &[
            ("deviceMake", "Oculus"),
            ("deviceModel", "Quest 3"),
            ("osName", "Android"),
            ("osVersion", "12L"),
        ],
        android_sdk: Some(32),
        music: false,
    },
    Client {
        client_name: "IOS",
        version: "21.02.3",
        user_agent: "com.google.ios.youtube/21.02.3 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
        number: "5",
        device: &[
            ("deviceMake", "Apple"),
            ("deviceModel", "iPhone16,2"),
            ("osName", "iPhone"),
            ("osVersion", "18.3.2.22D82"),
        ],
        android_sdk: None,
        music: false,
    },
    Client {
        client_name: "VISIONOS",
        version: "0.1",
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
        number: "101",
        device: &[
            ("deviceMake", "Apple"),
            ("deviceModel", "RealityDevice14,1"),
            ("osName", "visionOS"),
            ("osVersion", "1.3.21O771"),
        ],
        android_sdk: None,
        music: false,
    },
    Client {
        client_name: "IOS",
        version: "19.22.3",
        user_agent: "com.google.ios.youtube/19.22.3 (iPad7,6; U; CPU iPadOS 17_7_10 like Mac OS X; en-US)",
        number: "5",
        device: &[
            ("deviceMake", "Apple"),
            ("deviceModel", "iPad7,6"),
            ("osName", "iPadOS"),
            ("osVersion", "17.7.10.21H450"),
        ],
        android_sdk: None,
        music: false,
    },
];

impl Client {
    fn version(&self) -> String {
        if self.version.is_empty() {
            // The web app's version is the day it was built: today's, as a fresh page would say.
            format!("1.{}.01.00", today())
        } else {
            self.version.to_string()
        }
    }
}

/// Today's date (UTC) as YYYYMMDD.
fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let (y, m, d) = civil(days);
    format!("{y:04}{m:02}{d:02}")
}

/// The date of a day counted from 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The line to YouTube: one for the whole core.
pub struct InnerTube {
    agent: ureq::Agent,
    music: String,
    www: String,
    /// The id YouTube gave this visitor: sent back, so that it answers one visitor and not a new one each time.
    visitor: Mutex<Option<String>>,
    /// YouTube asked to be left alone until then, and why (in the listener's words).
    quiet: Mutex<Option<(Instant, &'static str)>>,
    /// Told the visitor id when YouTube first gives one, so that it can be kept.
    learned: Mutex<Option<Learned>>,
    /// The account the listener signed in with, and whether YouTube stopped taking it.
    login: Mutex<Option<Login>>,
    spent: AtomicBool,
}

/// Whether a request is asked as the listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signed {
    /// Signed out, whatever the listener signed in with (a search: it would land in their history).
    No,
    /// As the listener when they signed in, else signed out (a page: the same page, with what is theirs on it).
    IfAny,
    /// Only as the listener (their library): refused here when they have not signed in.
    Must,
}

/// What is told a visitor id YouTube gave.
pub type Learned = Box<dyn Fn(&str) + Send + Sync>;

impl Default for InnerTube {
    fn default() -> Self {
        Self::at(MUSIC, WWW)
    }
}

impl InnerTube {
    /// A line to YouTube's API at these two addresses (YouTube's own, or a test's).
    pub fn at(music: &str, www: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .timeout_resolve(Some(RESOLVE_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .max_redirects(0)
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
            music: music.trim_end_matches('/').to_string(),
            www: www.trim_end_matches('/').to_string(),
            visitor: Mutex::default(),
            quiet: Mutex::default(),
            learned: Mutex::default(),
            login: Mutex::default(),
            spent: AtomicBool::new(false),
        }
    }

    /// Signs in with `login`, or out (nothing).
    pub fn set_login(&self, login: Option<Login>) {
        *self.login.lock().unwrap() = login;
        self.spent.store(false, Ordering::Relaxed);
    }

    /// The login signed in with, and whether YouTube stopped taking it (the listener signed out on the web, or it
    /// ran out).
    pub fn login(&self) -> Option<Login> {
        self.login.lock().unwrap().clone()
    }

    pub fn login_spent(&self) -> bool {
        self.spent.load(Ordering::Relaxed)
    }

    /// Notes that YouTube no longer takes the login: it is not sent again until the listener signs in again.
    pub(crate) fn spend(&self) {
        self.spent.store(true, Ordering::Relaxed);
    }

    /// The visitor id YouTube gave, to keep for the next start.
    pub fn visitor(&self) -> Option<String> {
        self.visitor.lock().unwrap().clone()
    }

    /// Takes up the visitor id kept from before.
    pub fn set_visitor(&self, visitor: &str) {
        if let Some(visitor) = visitor_id(visitor) {
            *self.visitor.lock().unwrap() = Some(visitor);
        }
    }

    /// Has `learned` told the visitor id when YouTube first gives one.
    pub fn on_visitor(&self, learned: Learned) {
        *self.learned.lock().unwrap() = Some(learned);
    }

    /// Leaves YouTube alone for a while: every call until then is answered here, without asking it.
    pub(crate) fn quiet_for(&self, time: Duration, why: &'static str) {
        *self.quiet.lock().unwrap() = Some((Instant::now() + time, why));
    }

    fn quiet(&self) -> Option<AddonError> {
        let mut quiet = self.quiet.lock().unwrap();
        let (until, why) = (*quiet)?;
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            *quiet = None;
            return None;
        }
        Some(AddonError {
            code: Code::RateLimited,
            message: why.to_string(),
            retry_after: Some(left.as_secs().max(1) as u32),
            who: Some(WHO),
        })
    }

    /// Asks `action` ("search", "player", "browse") as `client`, with `more` beside the context, signed out. Answers
    /// YouTube's JSON, whatever it says.
    pub(crate) fn call(
        &self,
        client: &Client,
        action: &str,
        more: Value,
    ) -> Result<Value, AddonError> {
        self.call_as(client, action, more, Signed::No)
    }

    /// The same, asked as the listener as `signed` says. A login YouTube refuses is not sent again (until the listener
    /// signs in again): a page asked as them if they signed in is asked again signed out; their library is refused.
    pub(crate) fn call_as(
        &self,
        client: &Client,
        action: &str,
        more: Value,
        signed: Signed,
    ) -> Result<Value, AddonError> {
        if let Some(quiet) = self.quiet() {
            return Err(quiet);
        }
        let login = match signed {
            Signed::No => None,
            _ if !client.music || self.login_spent() => None,
            _ => self.login(),
        };
        if signed == Signed::Must && login.is_none() {
            return Err(trouble(
                Code::Unauthorized,
                if self.login_spent() {
                    "YouTube no longer takes your sign-in: sign in again in Settings"
                } else {
                    "Sign in to YouTube Music in Settings to see your library"
                },
            ));
        }
        match self.ask(client, action, &more, login.as_ref()) {
            Err(e) if login.is_some() && e.code == Code::Unauthorized => {
                self.spent.store(true, Ordering::Relaxed);
                if signed == Signed::IfAny {
                    self.ask(client, action, &more, None)
                } else {
                    Err(e)
                }
            }
            other => other,
        }
    }

    fn ask(
        &self,
        client: &Client,
        action: &str,
        more: &Value,
        login: Option<&Login>,
    ) -> Result<Value, AddonError> {
        let visitor = self.visitor();
        let mut about = json!({
            "clientName": client.client_name,
            "clientVersion": client.version(),
            "hl": "en",
            "gl": "US",
        });
        for (name, value) in client.device {
            about[*name] = json!(value);
        }
        if let Some(sdk) = client.android_sdk {
            about["androidSdkVersion"] = json!(sdk);
        }
        if let Some(visitor) = &visitor {
            about["visitorData"] = json!(visitor);
        }
        let mut body = json!({ "context": { "client": about, "user": {} } });
        if let (Some(body), Value::Object(more)) = (body.as_object_mut(), more) {
            body.extend(more.clone());
        }
        let url = if login.is_some() {
            // Signed in, the web page sends no key: the login says who asks.
            format!("{}/{action}?prettyPrint=false", self.music)
        } else if client.music {
            format!("{}/{action}?key={MUSIC_KEY}&prettyPrint=false", self.music)
        } else {
            format!("{}/{action}?prettyPrint=false", self.www)
        };
        let mut request = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json")
            .header("User-Agent", client.user_agent)
            .header("X-YouTube-Client-Name", client.number)
            .header("X-YouTube-Client-Version", client.version());
        if let Some(visitor) = &visitor {
            request = request.header("X-Goog-Visitor-Id", visitor);
        }
        if let Some(login) = login {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            request = request
                .header("Cookie", &login.header())
                .header("Authorization", &login.authorization(now))
                .header("Origin", ORIGIN)
                .header("X-Origin", ORIGIN)
                .header("Referer", &format!("{ORIGIN}/"))
                .header("X-Goog-AuthUser", "0");
        }
        let mut response = request.send(body.to_string()).map_err(unanswered)?;
        let status = response.status().as_u16();
        if login.is_some() && matches!(status, 401 | 403) {
            return Err(trouble(
                Code::Unauthorized,
                "YouTube no longer takes your sign-in: sign in again in Settings",
            ));
        }
        if status == 429 {
            self.quiet_for(AFTER_TOO_MANY, "YouTube is being asked too often");
            return Err(self
                .quiet()
                .unwrap_or_else(|| trouble(Code::RateLimited, "")));
        }
        if !(200..300).contains(&status) {
            return Err(trouble(
                Code::Unavailable,
                format!("YouTube did not answer the request (it said {status})"),
            ));
        }
        let text = response
            .body_mut()
            .with_config()
            .limit(BODY_CAP)
            .read_to_string()
            .map_err(|e| match e {
                ureq::Error::Timeout(_) => trouble(Code::Timeout, ""),
                other => trouble(
                    Code::Unavailable,
                    format!("YouTube's answer stopped coming in ({other})"),
                ),
            })?;
        let answer: Value = serde_json::from_str(&text)
            .map_err(|_| trouble(Code::InvalidResponse, "YouTube's answer could not be read"))?;
        if visitor.is_none()
            && let Some(given) = answer
                .pointer("/responseContext/visitorData")
                .and_then(Value::as_str)
                .and_then(visitor_id)
        {
            let mut kept = self.visitor.lock().unwrap();
            if kept.is_none() {
                *kept = Some(given.clone());
                drop(kept);
                if let Some(learned) = self.learned.lock().unwrap().as_ref() {
                    learned(&given);
                }
            }
        }
        Ok(answer)
    }
}

/// A visitor id as YouTube writes them, or nothing.
fn visitor_id(text: &str) -> Option<String> {
    let fits = !text.is_empty()
        && text.len() <= VISITOR_CAP
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'%' | b'='));
    fits.then(|| text.to_string())
}

/// Trouble with YouTube, in the listener's words (`words` empty: the usual words for `code`).
pub(crate) fn trouble(code: Code, words: impl Into<String>) -> AddonError {
    AddonError {
        code,
        message: words.into(),
        retry_after: None,
        who: Some(WHO),
    }
}

/// A request that got no answer.
fn unanswered(e: ureq::Error) -> AddonError {
    let asked = crate::addons::unanswered(e);
    trouble(
        asked.code,
        match asked.code {
            Code::Network | Code::Timeout => String::new(),
            _ => format!("YouTube could not be asked ({})", asked.message),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_read_from_days() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(civil(20_773), (2026, 11, 16));
        assert_eq!(today().len(), 8);
    }

    #[test]
    fn a_visitor_id_is_kept_only_when_it_looks_like_one() {
        assert_eq!(
            visitor_id("Cgt0ZEpn%3D%3D").as_deref(),
            Some("Cgt0ZEpn%3D%3D")
        );
        assert_eq!(visitor_id(""), None);
        assert_eq!(visitor_id("a b"), None);
        assert_eq!(visitor_id("a\r\nX-Other: 1"), None);
        assert_eq!(visitor_id(&"a".repeat(VISITOR_CAP + 1)), None);
    }
}
