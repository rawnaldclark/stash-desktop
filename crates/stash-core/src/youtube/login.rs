//! The YouTube account the listener signed in with: the cookies of a web session of theirs on YouTube, as Stash for
//! Android keeps them (`YouTubeCookieHelper`, `YouTubeLoginWebView`). They are sent to YouTube Music's own address,
//! with the header its web page sends beside them (`SAPISIDHASH`: proof the request comes from a page of its own
//! origin), and to nothing else.
//!
//! Signed in, YouTube Music answers its pages as the listener sees them (their liked songs, their playlists, their
//! home). Searches are still sent signed out: a search sent signed in lands in the account's search history, and Stash
//! searches for every song it looks for.

use sha1::{Digest, Sha1};

/// YouTube Music's origin, as the signed request names it.
pub(crate) const ORIGIN: &str = "https://music.youtube.com";

/// The most cookies kept, and the longest login: a browser's session for YouTube is a few dozen cookies and a few
/// kilobytes.
const MOST_COOKIES: usize = 120;
const LONGEST: usize = 24 * 1024;

/// A signed-in YouTube session.
#[derive(Clone, PartialEq, Eq)]
pub struct Login {
    cookies: Vec<(String, String)>,
}

// Never printed: what it holds signs the listener in.
impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Login({} cookies)", self.cookies.len())
    }
}

/// Whether `name` can be a cookie's name (a token, as RFC 6265 has it).
fn fit_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={}".contains(&b))
}

/// Whether `value` can be a cookie's value in a header (nothing that would end it, or the header).
fn fit_value(value: &str) -> bool {
    value.len() <= 8 * 1024
        && value
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b';' && b != b',' && b != b'\\')
}

impl Login {
    /// The login `text` gives: a `Cookie` header as a browser sends it ("NAME=value; NAME=value"), or a `cookies.txt`
    /// file (the Netscape form a browser extension or yt-dlp writes, of which only YouTube's cookies are taken).
    /// Nothing when it holds no `SAPISID`: without it, nothing can be asked as the listener.
    pub fn read(text: &str) -> Option<Login> {
        let text = text.trim();
        if text.is_empty() || text.len() > 4 * LONGEST {
            return None;
        }
        let tabbed = text
            .lines()
            .any(|line| !line.starts_with('#') && line.split('\t').count() == 7);
        let mut cookies: Vec<(String, String)> = Vec::new();
        let mut take = |name: &str, value: &str| {
            let (name, value) = (name.trim(), value.trim());
            if fit_name(name) && fit_value(value) && cookies.len() < MOST_COOKIES {
                // The last of a name wins, as in a jar.
                cookies.retain(|(known, _)| known != name);
                cookies.push((name.to_string(), value.to_string()));
            }
        };
        if tabbed {
            for line in text.lines() {
                // "#HttpOnly_" marks a cookie a page cannot read; it is a cookie all the same.
                let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
                if line.starts_with('#') {
                    continue;
                }
                let fields: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
                if fields.len() != 7 {
                    continue;
                }
                let domain = fields[0].trim_start_matches('.').to_ascii_lowercase();
                if domain == "youtube.com" || domain.ends_with(".youtube.com") {
                    take(fields[5], fields[6]);
                }
            }
        } else {
            let header = text
                .strip_prefix("Cookie:")
                .or_else(|| text.strip_prefix("cookie:"))
                .unwrap_or(text);
            for pair in header.split(';') {
                if let Some((name, value)) = pair.split_once('=') {
                    take(name, value);
                }
            }
        }
        let login = Login { cookies };
        (login.sapisid().is_some() && login.header().len() <= LONGEST).then_some(login)
    }

    fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }

    /// The session's `SAPISID` (a browser that keeps only the newer name has it as `__Secure-3PAPISID`).
    fn sapisid(&self) -> Option<&str> {
        self.cookie("SAPISID")
            .or_else(|| self.cookie("__Secure-3PAPISID"))
    }

    /// Whether the session has `LOGIN_INFO`, without which yt-dlp takes the cookies for spent.
    pub fn has_login_info(&self) -> bool {
        self.cookie("LOGIN_INFO").is_some()
    }

    /// The `Cookie` header that sends it.
    pub fn header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The `Authorization` header YouTube's web page sends beside the cookies at `now` (seconds since 1970): the
    /// SHA-1 of the time, the session's `SAPISID` and the origin, as ytmusicapi and Stash for Android compute it.
    pub(crate) fn authorization(&self, now: u64) -> String {
        let sapisid = self.sapisid().unwrap_or_default();
        let hash = Sha1::digest(format!("{now} {sapisid} {ORIGIN}").as_bytes());
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        format!("SAPISIDHASH {now}_{hex}")
    }

    /// The session as a `cookies.txt` file, for yt-dlp: every cookie for all of YouTube, lasting a year from `now`.
    pub fn cookies_txt(&self, now: u64) -> String {
        let until = now + 365 * 86_400;
        let mut file = String::from("# Netscape HTTP Cookie File\n");
        for (name, value) in &self.cookies {
            file.push_str(&format!(
                ".youtube.com\tTRUE\t/\tTRUE\t{until}\t{name}\t{value}\n"
            ));
        }
        file
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_is_read_from_a_header_or_a_cookies_file() {
        let login = Login::read(
            "Cookie: HSID=a1; SAPISID=sap/ID-9; LOGIN_INFO=AFm:QU; bad name=x; SID=s\r\n",
        )
        .unwrap();
        assert_eq!(
            login.header(),
            "HSID=a1; SAPISID=sap/ID-9; LOGIN_INFO=AFm:QU; SID=s"
        );
        assert!(login.has_login_info());

        let file = "# Netscape HTTP Cookie File\n\
            .youtube.com\tTRUE\t/\tTRUE\t1900000000\t__Secure-3PAPISID\tthree\n\
            #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1900000000\tSID\tsid\n\
            .google.com\tTRUE\t/\tTRUE\t1900000000\tNID\tgoogle-only\n\
            music.youtube.com\tFALSE\t/\tTRUE\t0\tPREF\tf6=40000000\n";
        let login = Login::read(file).unwrap();
        assert_eq!(
            login.header(),
            "__Secure-3PAPISID=three; SID=sid; PREF=f6=40000000"
        );
        assert!(!login.has_login_info());

        // No SAPISID: nothing can be asked as the listener.
        assert_eq!(Login::read("HSID=a; SID=b"), None);
        assert_eq!(Login::read(""), None);
        // A value that would end the header is left out.
        assert_eq!(Login::read("SAPISID=a\r\nX-Other: 1"), None);
    }

    #[test]
    fn the_authorization_is_the_hash_of_the_time_the_session_and_the_origin() {
        let login = Login::read("SAPISID=abc").unwrap();
        // sha1("1700000000 abc https://music.youtube.com")
        let expected = {
            let hash = Sha1::digest(b"1700000000 abc https://music.youtube.com");
            hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        assert_eq!(
            login.authorization(1_700_000_000),
            format!("SAPISIDHASH 1700000000_{expected}")
        );
        assert_eq!(expected.len(), 40);
    }

    #[test]
    fn the_session_is_written_for_yt_dlp() {
        let login = Login::read("SAPISID=abc; LOGIN_INFO=x").unwrap();
        assert_eq!(
            login.cookies_txt(1_000),
            "# Netscape HTTP Cookie File\n\
             .youtube.com\tTRUE\t/\tTRUE\t31537000\tSAPISID\tabc\n\
             .youtube.com\tTRUE\t/\tTRUE\t31537000\tLOGIN_INFO\tx\n"
        );
        assert_eq!(format!("{login:?}"), "Login(2 cookies)");
    }
}
