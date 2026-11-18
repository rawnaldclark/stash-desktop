//! Speaking to Last.fm (<https://www.last.fm/api>, API 2.0): the listener signs in on Last.fm's own page and Stash is
//! given a session for them, plays are sent ("scrobbled"), and the song playing now is told. What waits to be sent is
//! kept by `scrobble.rs`; the song is the one `listenbrainz::song` makes, read here as Last.fm takes it.
//!
//! Every request is signed with the application's key and secret. They come from the build (`STASH_LASTFM_KEY` and
//! `STASH_LASTFM_SECRET`), or from the environment Stash runs in, never from the source: a build without them has no
//! Last.fm. They are never printed, and go nowhere but Last.fm.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use md5::{Digest, Md5};
use serde_json::Value;

use crate::listenbrainz::{Agent, Sent};

/// How Stash names this way of sending.
pub const KIND: &str = "lastfm";
/// Last.fm's address for its API.
pub const ADDRESS: &str = "https://ws.audioscrobbler.com/2.0/";
/// Last.fm's page where the listener allows Stash.
const SIGN_IN: &str = "https://www.last.fm/api/auth/";
/// The most plays sent at once (Last.fm takes up to 50).
pub const AT_ONCE: usize = 50;
/// How long Last.fm is left when it kept as many plays as it takes in a day.
pub const NOT_TODAY: Duration = Duration::from_secs(3600);
/// The most of an answer that is read.
const ANSWER_CAP: u64 = 64 * 1024;
/// What is said of an answer that is not Last.fm's.
const NOT_LASTFM: &str = "it answered, but not as Last.fm does";

/// The application's key and secret, as the build or the environment gave them. Never shown: its `Debug` says
/// nothing of them.
#[derive(Clone)]
pub struct Keys {
    key: String,
    secret: String,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Keys(…)")
    }
}

impl Keys {
    /// The key and secret: the environment's, when it gives both, else the build's. None when neither gives both.
    pub fn found() -> Option<Keys> {
        let env = |name: &str| std::env::var(name).ok();
        Self::of(
            env("STASH_LASTFM_KEY").as_deref(),
            env("STASH_LASTFM_SECRET").as_deref(),
        )
        .or_else(|| {
            Self::of(
                option_env!("STASH_LASTFM_KEY"),
                option_env!("STASH_LASTFM_SECRET"),
            )
        })
    }

    fn of(key: Option<&str>, secret: Option<&str>) -> Option<Keys> {
        let key = key.map(str::trim).filter(|k| !k.is_empty())?;
        let secret = secret.map(str::trim).filter(|s| !s.is_empty())?;
        Some(Keys {
            key: key.to_string(),
            secret: secret.to_string(),
        })
    }
}

/// What Last.fm did with one of the plays sent.
#[derive(Debug, Clone, PartialEq)]
pub enum Play {
    /// Kept.
    Kept,
    /// Not kept, and it never will be as it is: why.
    Ignored(String),
    /// Not kept today: Last.fm kept as many plays as it takes in a day. It waits.
    NotToday,
}

/// What came of asking Last.fm whether the listener allowed Stash. (No `Debug`: it holds the session.)
pub enum Allowed {
    /// Allowed: the session (kept as the service's token) and the name Last.fm knows the listener by.
    Yes { session: String, user: String },
    /// Not yet: the listener has not said yes on Last.fm's page.
    NotYet,
    /// That sign-in is over (it ran out, or Last.fm does not know it): it is begun again.
    Over,
    /// Last.fm could not be asked: why.
    Failed(String),
}

/// Begins a sign-in (`auth.getToken`): the token Last.fm's page is opened with.
pub fn begin(agent: &Agent, keys: &Keys) -> Result<String, String> {
    let json = read(ask(agent, body(keys, "auth.getToken", Vec::new()))).map_err(why)?;
    text(&json, "token")
        // Last.fm's tokens are 32 characters, letters and digits but also others (about half of them): any printable
        // character is taken (the page's address and the form encode it).
        .filter(|t| !t.is_empty() && t.len() <= 200 && t.chars().all(|c| c.is_ascii_graphic()))
        .map(str::to_string)
        .ok_or_else(|| NOT_LASTFM.to_string())
}

/// Last.fm's page where the listener allows Stash, for the sign-in `token` began.
pub fn sign_in_page(keys: &Keys, token: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("api_key", &keys.key)
        .append_pair("token", token)
        .finish();
    format!("{SIGN_IN}?{query}")
}

/// Asks whether the listener allowed Stash for the sign-in `token` began (`auth.getSession`).
pub fn allowed(agent: &Agent, keys: &Keys, token: &str) -> Allowed {
    let params = vec![("token".to_string(), token.to_string())];
    let answer = ask(agent, body(keys, "auth.getSession", params));
    // 14: "This token has not been authorized"; 15: it ran out; 4: Last.fm does not know it.
    if let Ok(a) = &answer
        && let Some((code, _)) = error(&a.json)
    {
        match code {
            14 => return Allowed::NotYet,
            4 | 15 => return Allowed::Over,
            _ => {}
        }
    }
    let json = match read(answer) {
        Ok(json) => json,
        Err(sent) => return Allowed::Failed(why(sent)),
    };
    let Some(session) = json.get("session") else {
        return Allowed::Failed(NOT_LASTFM.into());
    };
    match text(session, "key").filter(|k| k.len() <= 200) {
        Some(key) => Allowed::Yes {
            session: key.to_string(),
            user: text(session, "name")
                .unwrap_or_default()
                .chars()
                .take(100)
                .collect(),
        },
        None => Allowed::Failed(NOT_LASTFM.into()),
    }
}

/// Sends plays (when each began, in seconds; the song) with the listener's `session` (`track.scrobble`): what Last.fm
/// did with each, in the order sent, or why it took none now.
pub fn send(
    agent: &Agent,
    keys: &Keys,
    session: &str,
    plays: &[(i64, &Value)],
) -> Result<Vec<Play>, Sent> {
    let json = read(ask(
        agent,
        body(keys, "track.scrobble", scrobble_params(session, plays)),
    ))?;
    kept(&json, plays.len())
}

/// Tells Last.fm the song playing now (`track.updateNowPlaying`): it shows it as playing, and does not keep it as a
/// play.
pub fn now_playing(agent: &Agent, keys: &Keys, session: &str, song: &Value) -> Sent {
    let mut params = song_params(song, None);
    params.push(("sk".to_string(), session.to_string()));
    match read(ask(agent, body(keys, "track.updateNowPlaying", params))) {
        Ok(_) => Sent::Taken,
        Err(sent) => sent,
    }
}

/// The signature Last.fm checks (`api_sig`): md5 of every parameter but `format` and `callback`, sorted by name, each
/// as its name then its value, then the secret; in lowercase hex.
pub fn signature(params: &[(String, String)], secret: &str) -> String {
    let sorted: BTreeMap<&str, &str> = params
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "format" | "callback"))
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let mut signed = String::new();
    for (name, value) in sorted {
        signed.push_str(name);
        signed.push_str(value);
    }
    signed.push_str(secret);
    hex(&Md5::digest(signed.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(32), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// A request's body: `params`, the method and the application's key, signed, as a form.
fn body(keys: &Keys, method: &str, mut params: Vec<(String, String)>) -> String {
    params.push(("method".to_string(), method.to_string()));
    params.push(("api_key".to_string(), keys.key.clone()));
    let signed = signature(&params, &keys.secret);
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in &params {
        form.append_pair(name, value);
    }
    form.append_pair("api_sig", &signed)
        .append_pair("format", "json");
    form.finish()
}

/// The plays as `track.scrobble` takes them, each by its place (`artist[0]`, `track[0]`, `timestamp[0]`, …), with the
/// listener's session.
fn scrobble_params(session: &str, plays: &[(i64, &Value)]) -> Vec<(String, String)> {
    let mut params = vec![("sk".to_string(), session.to_string())];
    for (i, (at, song)) in plays.iter().enumerate() {
        params.extend(song_params(song, Some(i)));
        params.push((format!("timestamp[{i}]"), at.to_string()));
    }
    params
}

/// The song (as `listenbrainz::song` makes it) as Last.fm takes it: artist, title, and the album and length (seconds)
/// when known. `place`: its place among the plays sent at once.
fn song_params(song: &Value, place: Option<usize>) -> Vec<(String, String)> {
    let name = |n: &str| match place {
        Some(i) => format!("{n}[{i}]"),
        None => n.to_string(),
    };
    let mut params = vec![
        (
            name("artist"),
            text(song, "artist_name").unwrap_or_default().to_string(),
        ),
        (
            name("track"),
            text(song, "track_name").unwrap_or_default().to_string(),
        ),
    ];
    if let Some(album) = text(song, "release_name") {
        params.push((name("album"), album.to_string()));
    }
    if let Some(ms) = song
        .get("additional_info")
        .and_then(|info| info.get("duration_ms"))
        .and_then(Value::as_i64)
        .filter(|ms| *ms > 0)
    {
        params.push((name("duration"), ((ms + 500) / 1000).to_string()));
    }
    params
}

/// A field's words, trimmed, when there are any.
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// A number, as Last.fm writes one (often in quotes).
fn number(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

/// An answer, as far as it is read.
struct Answer {
    status: u16,
    json: Value,
}

fn ask(agent: &Agent, body: String) -> Result<Answer, String> {
    let mut response = agent
        .post(ADDRESS)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(body)
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(ANSWER_CAP)
        .read_to_string()
        .unwrap_or_default();
    Ok(Answer {
        status,
        json: serde_json::from_str(&text).unwrap_or(Value::Null),
    })
}

/// The error Last.fm answered with: its number, and its words.
fn error(json: &Value) -> Option<(i64, String)> {
    let code = json.get("error").and_then(number)?;
    let said = text(json, "message")
        .map(|s| s.chars().take(200).collect::<String>())
        .unwrap_or_else(|| format!("Last.fm said no ({code})"));
    Some((code, said))
}

/// Last.fm's answer, when it is one it gives when all went well; else what to do next. 9: the session is no longer
/// good (the listener signs in again); 6: what was sent is wrong; 11 and 16 (down for now), 29 (too many requests) and
/// any other: later.
fn read(answer: Result<Answer, String>) -> Result<Value, Sent> {
    let a = match answer {
        Ok(a) => a,
        Err(why) => {
            return Err(Sent::Later {
                why: format!(
                    "no connection ({})",
                    why.chars().take(160).collect::<String>()
                ),
                wait: None,
            });
        }
    };
    if let Some((code, said)) = error(&a.json) {
        return Err(match code {
            9 => Sent::Refused(said),
            6 => Sent::Wrong(said),
            _ => Sent::Later {
                why: said,
                wait: None,
            },
        });
    }
    match a.status {
        // Taken only when it says so: a hotel's sign-in page answers 200 too.
        200..=299 if a.json.is_object() => Ok(a.json),
        200..=299 => Err(Sent::Later {
            why: NOT_LASTFM.into(),
            wait: None,
        }),
        status => Err(Sent::Later {
            why: format!("it answered {status}"),
            wait: None,
        }),
    }
}

/// What a try came to, in words.
fn why(sent: Sent) -> String {
    match sent {
        Sent::Refused(why) | Sent::Wrong(why) | Sent::Later { why, .. } => why,
        Sent::Taken => String::new(),
    }
}

/// What Last.fm did with each of the `sent` plays, from its answer to `track.scrobble`. One play is answered as an
/// object, more as a list; an answer that does not tell each is believed only when it says it kept them all.
fn kept(json: &Value, sent: usize) -> Result<Vec<Play>, Sent> {
    let not_lastfm = || Sent::Later {
        why: NOT_LASTFM.into(),
        wait: None,
    };
    let scrobbles = json.get("scrobbles").ok_or_else(not_lastfm)?;
    let each: Vec<&Value> = match scrobbles.get("scrobble") {
        Some(Value::Array(all)) => all.iter().collect(),
        Some(one @ Value::Object(_)) => vec![one],
        _ => Vec::new(),
    };
    if each.len() == sent {
        return Ok(each.into_iter().map(play).collect());
    }
    let accepted = scrobbles
        .get("@attr")
        .and_then(|attr| attr.get("accepted"))
        .and_then(number);
    if accepted == Some(sent as i64) {
        Ok(vec![Play::Kept; sent])
    } else {
        Err(not_lastfm())
    }
}

/// What Last.fm did with one play, from its part of the answer (`ignoredMessage`).
fn play(scrobble: &Value) -> Play {
    let ignored = scrobble.get("ignoredMessage");
    let code = ignored
        .and_then(|m| m.get("code"))
        .and_then(number)
        .unwrap_or(0);
    match code {
        0 => Play::Kept,
        5 => Play::NotToday,
        1 => Play::Ignored("Last.fm does not take this artist".into()),
        2 => Play::Ignored("Last.fm does not take this song".into()),
        3 => Play::Ignored("Last.fm takes plays from the last two weeks only".into()),
        4 => Play::Ignored("its time is later than now".into()),
        _ => Play::Ignored(
            ignored
                .and_then(|m| text(m, "#text"))
                .map(|s| s.chars().take(200).collect())
                .unwrap_or_else(|| "Last.fm did not keep it".into()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listenbrainz::song;
    use serde_json::json;

    fn pair(name: &str, value: &str) -> (String, String) {
        (name.to_string(), value.to_string())
    }

    #[test]
    fn a_request_is_signed_as_last_fm_checks_it() {
        let params = vec![
            pair("method", "auth.getToken"),
            pair("api_key", "k"),
            pair("format", "json"),
            pair("callback", "f"),
        ];
        // Sorted by name, each name then its value, then the secret; `format` and `callback` left out.
        let expected = hex(&Md5::digest(b"api_keykmethodauth.getTokens"));
        assert_eq!(signature(&params, "s"), expected);
        assert_eq!(expected.len(), 32);
        assert!(
            expected
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
    }

    #[test]
    fn plays_go_as_one_signed_form_each_by_its_place() {
        let keys = Keys::of(Some(" k "), Some("s")).unwrap();
        let first = song(
            "Paper Boats",
            "The Lanterns",
            Some("Night Drive"),
            Some(184_400),
            None,
            None,
        );
        let second = song("Afterglow", "Mira Sol", None, None, None, None);
        let plays = [(1_700_000_000, &first), (1_700_000_300, &second)];
        let form = body(&keys, "track.scrobble", scrobble_params("sk-1", &plays));
        let sent: Vec<(String, String)> = url::form_urlencoded::parse(form.as_bytes())
            .into_owned()
            .collect();
        let get = |name: &str| {
            sent.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("method"), Some("track.scrobble"));
        assert_eq!(get("api_key"), Some("k"));
        assert_eq!(get("sk"), Some("sk-1"));
        assert_eq!(get("format"), Some("json"));
        assert_eq!(get("artist[0]"), Some("The Lanterns"));
        assert_eq!(get("track[0]"), Some("Paper Boats"));
        assert_eq!(get("album[0]"), Some("Night Drive"));
        assert_eq!(get("duration[0]"), Some("184"));
        assert_eq!(get("timestamp[0]"), Some("1700000000"));
        assert_eq!(get("artist[1]"), Some("Mira Sol"));
        assert_eq!(get("track[1]"), Some("Afterglow"));
        assert_eq!(get("timestamp[1]"), Some("1700000300"));
        assert_eq!(get("album[1]"), None);
        assert_eq!(get("duration[1]"), None);
        // The secret is never sent; the signature is of everything else but `format`.
        assert!(!sent.iter().any(|(_, v)| v == "s"));
        let signed: Vec<(String, String)> = sent
            .iter()
            .filter(|(n, _)| n != "api_sig")
            .cloned()
            .collect();
        assert_eq!(get("api_sig"), Some(signature(&signed, "s").as_str()));
    }

    #[test]
    fn the_song_playing_now_goes_without_a_place() {
        let one = song("Paper Boats", "The Lanterns", None, Some(1_000), None, None);
        assert_eq!(
            song_params(&one, None),
            vec![
                pair("artist", "The Lanterns"),
                pair("track", "Paper Boats"),
                pair("duration", "1"),
            ]
        );
    }

    #[test]
    fn answers_are_read_as_what_to_do_next() {
        let answer = |status: u16, json: Value| Ok(Answer { status, json });
        assert!(read(answer(200, json!({ "token": "abc" }))).is_ok());
        let signed_out = "Invalid session - Please re-authenticate";
        assert_eq!(
            read(answer(403, json!({ "error": 9, "message": signed_out }))),
            Err(Sent::Refused(signed_out.into()))
        );
        assert!(matches!(
            read(answer(
                400,
                json!({ "error": 6, "message": "Invalid parameters" })
            )),
            Err(Sent::Wrong(_))
        ));
        for code in [11, 16, 29] {
            let said = read(answer(503, json!({ "error": code })));
            assert!(matches!(said, Err(Sent::Later { .. })), "{code}");
        }
        // A page that is not Last.fm's (a hotel's sign-in page) is not taken for a yes.
        assert!(matches!(
            read(answer(200, Value::Null)),
            Err(Sent::Later { .. })
        ));
        assert!(matches!(
            read(Err("refused".into())),
            Err(Sent::Later { wait: None, .. })
        ));
    }

    #[test]
    fn each_play_is_read_as_kept_ignored_or_not_today() {
        let one = json!({ "scrobbles": {
            "@attr": { "accepted": 1, "ignored": 0 },
            "scrobble": { "ignoredMessage": { "code": "0", "#text": "" } }
        }});
        assert_eq!(kept(&one, 1), Ok(vec![Play::Kept]));
        let three = json!({ "scrobbles": {
            "@attr": { "accepted": 1, "ignored": 2 },
            "scrobble": [
                { "ignoredMessage": { "code": "0", "#text": "" } },
                { "ignoredMessage": { "code": "3", "#text": "Timestamp too old" } },
                { "ignoredMessage": { "code": "5", "#text": "Daily scrobble limit exceeded" } }
            ]
        }});
        assert_eq!(
            kept(&three, 3),
            Ok(vec![
                Play::Kept,
                Play::Ignored("Last.fm takes plays from the last two weeks only".into()),
                Play::NotToday,
            ])
        );
        // Not told play by play: believed only when it says it kept them all.
        let told = json!({ "scrobbles": { "@attr": { "accepted": "2", "ignored": "0" } } });
        assert_eq!(kept(&told, 2), Ok(vec![Play::Kept, Play::Kept]));
        assert!(kept(&told, 3).is_err());
        assert!(kept(&json!({}), 1).is_err());
    }

    #[test]
    fn the_keys_need_both_and_say_nothing_of_themselves() {
        assert!(Keys::of(Some("k"), None).is_none());
        assert!(Keys::of(Some(" "), Some("s")).is_none());
        let keys = Keys::of(Some("a-key"), Some("a-secret")).unwrap();
        let shown = format!("{keys:?}");
        assert!(!shown.contains("a-key") && !shown.contains("a-secret"));
        let page = sign_in_page(&keys, "t0k");
        assert!(page.starts_with("https://www.last.fm/api/auth/?"));
        assert!(page.contains("token=t0k") && !page.contains("a-secret"));
    }
}
