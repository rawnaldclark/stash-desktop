//! Speaking to ListenBrainz (<https://listenbrainz.readthedocs.io/en/latest/users/api/core.html>), and to the servers
//! that speak its language at their own address (Maloja, Koito and others people run themselves): a listener's token
//! is checked, plays are sent, and the song playing now is told. What waits to be sent is kept by `scrobble.rs`.

use std::time::Duration;

use serde_json::{Value, json};
use url::Url;

/// How Stash names this way of sending.
pub const KIND: &str = "listenbrainz";
/// ListenBrainz's own address.
pub const ADDRESS: &str = "https://api.listenbrainz.org";
/// The most plays sent at once (ListenBrainz takes up to 1,000).
pub const AT_ONCE: usize = 100;
/// The most of an answer that is read.
const ANSWER_CAP: u64 = 64 * 1024;

/// The way to the services, as [`agent`] makes it.
pub type Agent = ureq::Agent;

/// What a try came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    /// Taken.
    Taken,
    /// The service refused the token.
    Refused(String),
    /// The service says what was sent is wrong: it will never take it as it is.
    Wrong(String),
    /// Not now: no connection, or the service is down or busy. `wait`: how long the service asked to be left.
    Later { why: String, wait: Option<Duration> },
}

/// The way to the services: every answer is read whatever its status, a redirect is not followed (the token goes
/// only where the listener said), and nothing waits long.
pub fn agent() -> Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(0)
        .http_status_as_error(false)
        .user_agent(concat!("Stash-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// The address a listener gave, as the root the API's paths go under: "https://host[:port][/path]", without "/1/…"
/// and without a slash at the end. An address with a name or password in it is refused: the token says who the
/// listener is.
pub fn root(given: &str) -> Result<String, String> {
    let given = given.trim();
    let url = Url::parse(given).map_err(|_| "That is not a web address".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err("That is not a web address (http or https)".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "An address with a name or password in it is not used: the token says who you are"
                .into(),
        );
    }
    let mut root = url[..url::Position::AfterPath]
        .trim_end_matches('/')
        .to_string();
    // The address of one of the API's own paths, pasted whole, is cut back to its root.
    for tail in ["/1/submit-listens", "/1/validate-token", "/1"] {
        if let Some(cut) = root.strip_suffix(tail) {
            root = cut.trim_end_matches('/').to_string();
            break;
        }
    }
    Ok(root)
}

/// The song as a service takes it ("track_metadata"): its title, artist and album, and what else is known of it.
pub fn song(
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration_ms: Option<i64>,
    isrc: Option<&str>,
    track_no: Option<i64>,
) -> Value {
    let mut info = json!({
        "media_player": "Stash",
        "submission_client": "Stash desktop",
        "submission_client_version": env!("CARGO_PKG_VERSION"),
    });
    if let Some(ms) = duration_ms.filter(|ms| *ms > 0) {
        info["duration_ms"] = json!(ms);
    }
    if let Some(isrc) = isrc.map(str::trim).filter(|i| !i.is_empty()) {
        info["isrc"] = json!(isrc);
    }
    if let Some(n) = track_no.filter(|n| *n > 0) {
        info["tracknumber"] = json!(n);
    }
    let mut song = json!({
        "artist_name": artist.trim(),
        "track_name": title.trim(),
        "additional_info": info,
    });
    if let Some(album) = album.map(str::trim).filter(|a| !a.is_empty()) {
        song["release_name"] = json!(album);
    }
    song
}

/// Whether a service can take the song at all: it needs a title and an artist.
pub fn sendable(song: &Value) -> bool {
    let named = |key: &str| {
        song.get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
    };
    named("track_name") && named("artist_name")
}

/// Checks `token` with the service at `root`: the name it knows the listener by.
pub fn check(agent: &Agent, root: &str, token: &str) -> Result<String, Sent> {
    match ask(agent, &format!("{root}/1/validate-token"), token, None) {
        Ok(answer) if (200..300).contains(&answer.status) => {
            if answer.json.get("valid").and_then(Value::as_bool) == Some(true) {
                Ok(answer
                    .json
                    .get("user_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .chars()
                    .take(100)
                    .collect())
            } else {
                Err(Sent::Refused(said(&answer.json, "the token was not taken")))
            }
        }
        Ok(answer) if matches!(answer.status, 400 | 401 | 403) => {
            Err(Sent::Refused(said(&answer.json, "the token was not taken")))
        }
        Ok(answer) if answer.status == 404 => Err(Sent::Wrong(
            "Nothing at that address answers as ListenBrainz does".into(),
        )),
        Ok(answer) if (300..400).contains(&answer.status) => Err(Sent::Wrong(match answer.moved {
            Some(to) => format!("That address sends elsewhere ({to}): give that one"),
            None => "That address sends elsewhere".into(),
        })),
        other => Err(outcome(other)),
    }
}

/// Sends plays (when each began, in seconds; the song) to the service at `root`. One goes as "single", more as
/// "import".
pub fn send(agent: &Agent, root: &str, token: &str, plays: &[(i64, &Value)]) -> Sent {
    let payload: Vec<Value> = plays
        .iter()
        .map(|(at, song)| json!({ "listened_at": at, "track_metadata": song }))
        .collect();
    let kind = if plays.len() == 1 { "single" } else { "import" };
    let body = json!({ "listen_type": kind, "payload": payload });
    outcome(ask(
        agent,
        &format!("{root}/1/submit-listens"),
        token,
        Some(&body),
    ))
}

/// Tells the service the song playing now: it shows it as playing, and does not keep it as a play.
pub fn now_playing(agent: &Agent, root: &str, token: &str, song: &Value) -> Sent {
    let body = json!({ "listen_type": "playing_now", "payload": [{ "track_metadata": song }] });
    outcome(ask(
        agent,
        &format!("{root}/1/submit-listens"),
        token,
        Some(&body),
    ))
}

/// An answer, as far as it is read.
struct Answer {
    status: u16,
    /// How long the service asked to be left (when it is busy).
    wait: Option<Duration>,
    /// Where it sends the request instead (a redirect).
    moved: Option<String>,
    json: Value,
}

fn ask(agent: &Agent, url: &str, token: &str, body: Option<&Value>) -> Result<Answer, String> {
    let auth = format!("Token {token}");
    let response = match body {
        Some(body) => agent
            .post(url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .send(body.to_string()),
        None => agent.get(url).header("Authorization", &auth).call(),
    };
    let mut response = response.map_err(|e| e.to_string())?;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .map(str::to_string)
    };
    let wait = header("X-RateLimit-Reset-In")
        .and_then(|v| v.parse::<u64>().ok())
        .map(|s| Duration::from_secs(s.min(3600)));
    let moved = header("Location").map(|l| l.chars().take(200).collect());
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(ANSWER_CAP)
        .read_to_string()
        .unwrap_or_default();
    Ok(Answer {
        status,
        wait,
        moved,
        json: serde_json::from_str(&text).unwrap_or(Value::Null),
    })
}

/// What the service said went wrong, else `otherwise`.
fn said(json: &Value, otherwise: &str) -> String {
    json.get("error")
        .or_else(|| json.get("message"))
        .and_then(Value::as_str)
        .map(|s| s.trim().chars().take(200).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| otherwise.to_string())
}

fn outcome(answer: Result<Answer, String>) -> Sent {
    match answer {
        Err(why) => Sent::Later {
            why: format!(
                "no connection ({})",
                why.chars().take(160).collect::<String>()
            ),
            wait: None,
        },
        Ok(a) => match a.status {
            200..=299 => Sent::Taken,
            401 | 403 => Sent::Refused(said(&a.json, "the token was refused")),
            400 | 413 | 422 => Sent::Wrong(said(&a.json, "it said the play is wrong")),
            429 => Sent::Later {
                why: said(&a.json, "it asked to be left a while"),
                wait: a.wait,
            },
            status => Sent::Later {
                why: said(&a.json, &format!("it answered {status}")),
                wait: a.wait,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_cut_to_the_root_its_paths_go_under() {
        assert_eq!(root("https://api.listenbrainz.org/").unwrap(), ADDRESS);
        assert_eq!(
            root("  https://api.listenbrainz.org/1/submit-listens ").unwrap(),
            ADDRESS
        );
        assert_eq!(
            root("http://192.168.1.20:42010/apis/listenbrainz/1/validate-token?x=1#y").unwrap(),
            "http://192.168.1.20:42010/apis/listenbrainz"
        );
        assert_eq!(
            root("https://example.org/apis/listenbrainz/1").unwrap(),
            "https://example.org/apis/listenbrainz"
        );
        assert!(root("ftp://example.org").is_err());
        assert!(root("not an address").is_err());
        assert!(root("https://me:secret@example.org").is_err());
    }

    #[test]
    fn a_song_says_what_is_known_and_nothing_empty() {
        let full = song(
            " Paper Boats ",
            "The Lanterns",
            Some("Night Drive"),
            Some(184_000),
            Some("GBAAA2400001"),
            Some(3),
        );
        assert_eq!(full["track_name"], "Paper Boats");
        assert_eq!(full["artist_name"], "The Lanterns");
        assert_eq!(full["release_name"], "Night Drive");
        assert_eq!(full["additional_info"]["duration_ms"], 184_000);
        assert_eq!(full["additional_info"]["isrc"], "GBAAA2400001");
        assert_eq!(full["additional_info"]["tracknumber"], 3);
        assert_eq!(full["additional_info"]["media_player"], "Stash");
        let bare = song("Afterglow", "Mira Sol", Some("  "), Some(0), Some(""), None);
        assert!(bare.get("release_name").is_none());
        for key in ["duration_ms", "isrc", "tracknumber"] {
            assert!(bare["additional_info"].get(key).is_none(), "{key}");
        }
        assert!(sendable(&bare));
        assert!(!sendable(&song("Afterglow", " ", None, None, None, None)));
        assert!(!sendable(&Value::Null));
    }

    #[test]
    fn answers_are_read_as_what_to_do_next() {
        let answer = |status: u16, json: Value| {
            Ok(Answer {
                status,
                wait: Some(Duration::from_secs(7)),
                moved: None,
                json,
            })
        };
        assert_eq!(outcome(answer(200, json!({ "status": "ok" }))), Sent::Taken);
        assert_eq!(
            outcome(answer(
                401,
                json!({ "code": 401, "error": "Invalid authorization token." })
            )),
            Sent::Refused("Invalid authorization token.".into())
        );
        assert_eq!(
            outcome(answer(400, Value::Null)),
            Sent::Wrong("it said the play is wrong".into())
        );
        assert_eq!(
            outcome(answer(429, Value::Null)),
            Sent::Later {
                why: "it asked to be left a while".into(),
                wait: Some(Duration::from_secs(7))
            }
        );
        assert!(
            matches!(outcome(answer(503, Value::Null)), Sent::Later { ref why, .. } if why == "it answered 503")
        );
        assert!(matches!(
            outcome(Err("refused".into())),
            Sent::Later { wait: None, .. }
        ));
    }
}
