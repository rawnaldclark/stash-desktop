//! Repositories of packages (spec §8.3; Lokal `morneye_packages.dart` 1093-1235).

use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::manifest::{is_name, text_of};
use crate::net::{Network, NetworkSetup, Permissions};

/// Mornye's own repository: always listed, never removed.
pub const OFFICIAL: &str =
    "https://raw.githubusercontent.com/mornyeapp/Mornye-Extension/refs/heads/main/registry.json";

const MAX_ENTRIES: usize = 2000;
const MAX_NAME: usize = 80;
/// Waiting for the address, the connection and the answer's headers (Lokal's request timeout).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Reading the whole answer.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const INVALID: &str = "Invalid Morneye registry";

/// One package a repository lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The package's name (`^[a-z0-9][a-z0-9_-]{0,99}$`).
    pub id: String,
    pub version: String,
    /// The SHA-256 the package's bytes must have (64 hex, lowercased).
    pub sha256: String,
    /// `https` only.
    pub download_url: String,
    pub display_name: String,
    pub description: String,
    pub min_app_version: Option<String>,
}

/// A repository's listing (`{"version": 1, "name"?, "extensions": [...]}`): its name (not the official one's) and
/// its `download` entries (other categories skipped; an invalid `download` entry fails the whole listing with
/// `Invalid registry package entry`; an invalid `integration` entry is skipped; `Duplicate registry package IDs`).
pub fn parse(text: &str, official: bool) -> Result<(Option<String>, Vec<Entry>), String> {
    let index: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|_| INVALID.to_string())?;
    let version_one = index.get("version").and_then(Value::as_f64) == Some(1.0);
    let extensions = match index.get("extensions") {
        Some(Value::Array(list))
            if index.is_object() && version_one && list.len() <= MAX_ENTRIES =>
        {
            list
        }
        _ => return Err(INVALID.into()),
    };
    let mut entries: Vec<Entry> = Vec::new();
    for value in extensions {
        if !value.is_object() || value.get("category").and_then(Value::as_str) != Some("download") {
            // Integration add-ons (playlist-link readers) and anything else: Stash plays songs, so they are not
            // listed; an invalid one never fails the listing.
            continue;
        }
        let entry = entry_of(value).ok_or_else(|| "Invalid registry package entry".to_string())?;
        if entries.iter().any(|known| known.id == entry.id) {
            return Err("Duplicate registry package IDs".into());
        }
        entries.push(entry);
    }
    let name = index
        .get("name")
        .and_then(Value::as_str)
        .map(|name| {
            name.chars()
                .filter(|c| (*c as u32) > 0x1f)
                .collect::<String>()
                .trim()
                .chars()
                .take(MAX_NAME)
                .collect::<String>()
        })
        .filter(|name| !name.is_empty() && !official);
    Ok((name, entries))
}

/// An entry, when it is valid (pkgs.dart:1169-1182).
fn entry_of(value: &Value) -> Option<Entry> {
    let present = |key: &str| value.get(key).filter(|v| !v.is_null());
    let id = text_of(present("id").or_else(|| present("name")));
    let version = value.get("version")?.as_str()?.to_string();
    let sha256 = text_of(present("sha256")).to_ascii_lowercase();
    let download_url = text_of(present("download_url"));
    let https = Url::parse(&download_url).is_ok_and(|url| url.scheme() == "https");
    let hex = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
    if !is_name(&id) || !hex || !https {
        return None;
    }
    let display_name = present("display_name")
        .or_else(|| present("displayName"))
        .or_else(|| present("name"))
        .map(|name| text_of(Some(name)))
        .unwrap_or_else(|| id.clone());
    Some(Entry {
        description: text_of(present("description")),
        min_app_version: present("min_app_version").map(|v| text_of(Some(v))),
        id,
        version,
        sha256,
        download_url,
        display_name,
    })
}

/// A repository's address made right (`https`, no userinfo, `/registry.json` added when the path does not end in
/// `.json`, no fragment) and its id (the first 16 hex characters of SHA-256 of it).
pub fn normalize(url: &str) -> Result<(String, String), String> {
    const HTTPS: &str = "Repository URLs must use HTTPS";
    let mut url = Url::parse(url.trim()).map_err(|_| HTTPS.to_string())?;
    let has_host = url.host_str().is_some_and(|host| !host.is_empty());
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || !has_host
    {
        return Err(HTTPS.into());
    }
    if !url.path().to_ascii_lowercase().ends_with(".json") {
        let path = url.path().to_string();
        let base = path.strip_suffix('/').unwrap_or(&path);
        url.set_path(&format!("{base}/registry.json"));
    }
    url.set_fragment(None);
    let href = url.to_string();
    let id = crate::hex(&crate::crypto::sha256(href.as_bytes()))[..16].to_string();
    Ok((href, id))
}

/// GETs `url` from its one host (status exactly 200, at most `limit` bytes, 60 s), as Stash.
///
/// The request goes through a [`Network`] allowed only that host, as Lokal's does: every redirect hop is checked
/// against it, `https` only, and a name that resolves to a private address is refused.
pub fn fetch(url: &str, limit: u64, user_agent: &str) -> Result<Vec<u8>, String> {
    let parsed = Url::parse(url).map_err(|_| "Invalid URL".to_string())?;
    let host = crate::net::host_of(&parsed).ok_or_else(|| "Invalid URL".to_string())?;
    let network = Network::new(NetworkSetup {
        permissions: Permissions {
            network: vec![host],
            storage: false,
            file: false,
            allow_http: false,
        },
        user_agent: user_agent.to_string(),
        timeout: REQUEST_TIMEOUT,
        saved: Value::Object(Default::default()),
        keep: Arc::new(|_: Value| {}),
    });
    let started = Instant::now();
    let mut answer = network.request(url, "GET", &[], &[], false, Some(READ_TIMEOUT))?;
    if answer.status != 200 {
        return Err(format!("Repository returned HTTP {}", answer.status));
    }
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        if started.elapsed() > READ_TIMEOUT {
            return Err("Repository read timed out".into());
        }
        let read = answer.body.read(&mut chunk).map_err(|e| e.to_string())?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() as u64 > limit {
            return Err("Response is too large".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "c720fccc7f1063606724ab7436d73ee7c6cc86e8028c4d38122fb02c6a7a0f98";

    /// A few entries in the shape of the official registry.
    const REGISTRY: &str = r#"{
      "version": 1,
      "updated_at": "2026-11-16T00:00:00Z",
      "name": "Some\u0007 repository",
      "extensions": [
        {
          "id": "spotify-web", "name": "spotify-web", "display_name": "Spotify Web", "version": "1.10.6",
          "description": "Spotify metadata provider.",
          "download_url": "https://raw.githubusercontent.com/mornyeapp/Mornye-Extension/main/extensions/spotify-web.sflx",
          "sha256": "8ed483f6c451a3af08303b833457de3fa72d8ec8aec1d80bb5f4594c2a303640",
          "category": "integration", "min_app_version": "4.3.0"
        },
        {
          "id": "soundcloud", "name": "soundcloud", "display_name": "SoundCloud", "version": "1.0.15",
          "description": "SoundCloud metadata and download provider.",
          "download_url": "https://raw.githubusercontent.com/mornyeapp/Mornye-Extension/main/extensions/soundcloud.sflx",
          "sha256": "C720FCCC7F1063606724AB7436D73EE7C6CC86E8028C4D38122FB02C6A7A0F98",
          "category": "download", "tags": ["soundcloud"], "downloads": 0, "min_app_version": "4.9.1"
        },
        {
          "id": "qobuz-web", "name": "qobuz-web", "display_name": "Qobuz", "version": "1.2.27",
          "description": "Qobuz metadata and download provider.",
          "download_url": "https://raw.githubusercontent.com/mornyeapp/Mornye-Extension/main/extensions/qobuz-web.sflx",
          "sha256": "0000000000000000000000000000000000000000000000000000000000000001",
          "category": "download", "min_app_version": "5.3.0"
        },
        { "id": "Broken Integration", "category": "integration" },
        { "id": "theme", "category": "theme" },
        "not an entry"
      ]
    }"#;

    #[test]
    fn a_registry_is_parsed() {
        let (name, entries) = parse(REGISTRY, false).unwrap();
        assert_eq!(name.as_deref(), Some("Some repository"));
        assert_eq!(entries.len(), 2);
        let soundcloud = &entries[0];
        assert_eq!(soundcloud.id, "soundcloud");
        assert_eq!(soundcloud.version, "1.0.15");
        assert_eq!(soundcloud.sha256, SHA);
        assert_eq!(soundcloud.display_name, "SoundCloud");
        assert_eq!(soundcloud.min_app_version.as_deref(), Some("4.9.1"));
        assert!(soundcloud.download_url.ends_with("soundcloud.sflx"));
        assert_eq!(entries[1].id, "qobuz-web");
    }

    #[test]
    fn the_official_repository_keeps_its_own_name() {
        let (name, entries) = parse(REGISTRY, true).unwrap();
        assert_eq!(name, None);
        assert_eq!(entries.len(), 2);
    }

    fn one(entry: &str) -> String {
        format!(r#"{{ "version": 1, "extensions": [{entry}] }}"#)
    }

    #[test]
    fn an_invalid_download_entry_fails_the_listing() {
        let bad = [
            format!(r#"{{ "id": "BAD", "version": "1.0.0", "sha256": "{SHA}", "download_url": "https://example.com/a.sflx", "category": "download" }}"#),
            format!(r#"{{ "id": "ok", "version": 1, "sha256": "{SHA}", "download_url": "https://example.com/a.sflx", "category": "download" }}"#),
            r#"{ "id": "ok", "version": "1.0.0", "sha256": "abc", "download_url": "https://example.com/a.sflx", "category": "download" }"#.to_string(),
            format!(r#"{{ "id": "ok", "version": "1.0.0", "sha256": "{SHA}", "download_url": "http://example.com/a.sflx", "category": "download" }}"#),
        ];
        for entry in bad {
            assert_eq!(
                parse(&one(&entry), false).unwrap_err(),
                "Invalid registry package entry",
                "{entry}"
            );
        }
    }

    #[test]
    fn an_entry_may_be_named_by_name_and_shown_by_it() {
        let entry = format!(
            r#"{{ "name": "demo", "version": "1.0.0", "sha256": "{SHA}", "download_url": "https://example.com/a.sflx", "category": "download" }}"#
        );
        let (_, entries) = parse(&one(&entry), false).unwrap();
        assert_eq!(entries[0].id, "demo");
        assert_eq!(entries[0].display_name, "demo");
        assert_eq!(entries[0].description, "");
        assert_eq!(entries[0].min_app_version, None);
    }

    #[test]
    fn duplicates_and_bad_listings_are_refused() {
        let entry = format!(
            r#"{{ "id": "demo", "version": "1.0.0", "sha256": "{SHA}", "download_url": "https://example.com/a.sflx", "category": "download" }}"#
        );
        let twice = format!(r#"{{ "version": 1, "extensions": [{entry}, {entry}] }}"#);
        assert_eq!(
            parse(&twice, false).unwrap_err(),
            "Duplicate registry package IDs"
        );
        for text in [
            r#"{ "version": 2, "extensions": [] }"#,
            r#"{ "version": 1 }"#,
            "[]",
            "nope",
        ] {
            assert_eq!(parse(text, false).unwrap_err(), INVALID, "{text}");
        }
        let many = format!(
            r#"{{ "version": 1, "extensions": [{}] }}"#,
            vec!["1"; 2001].join(",")
        );
        assert_eq!(parse(&many, false).unwrap_err(), INVALID);
    }

    #[test]
    fn addresses_are_normalized() {
        assert_eq!(
            normalize(" https://example.org ").unwrap(),
            (
                "https://example.org/registry.json".to_string(),
                "7739ea2723f73f31".to_string()
            )
        );
        assert_eq!(
            normalize("https://example.org/repo/#top").unwrap(),
            (
                "https://example.org/repo/registry.json".to_string(),
                "18605dd1d4c0f804".to_string()
            )
        );
        assert_eq!(
            normalize("https://example.org/repo").unwrap().0,
            "https://example.org/repo/registry.json"
        );
        assert_eq!(
            normalize("https://example.org/feed.JSON").unwrap().0,
            "https://example.org/feed.JSON"
        );
        assert_eq!(
            normalize(OFFICIAL).unwrap(),
            (OFFICIAL.to_string(), "e57c4bf7df4805ca".to_string())
        );
        for bad in [
            "http://example.org/registry.json",
            "https://user:pw@example.org/r.json",
            "example.org",
            "",
        ] {
            assert_eq!(
                normalize(bad).unwrap_err(),
                "Repository URLs must use HTTPS",
                "{bad}"
            );
        }
    }

    #[test]
    fn fetching_needs_https() {
        // Refused before any connection is made.
        assert_eq!(
            fetch("http://example.org/registry.json", 10, "Stash/test").unwrap_err(),
            "This addon requires HTTPS"
        );
        assert_eq!(
            fetch("not a url", 10, "Stash/test").unwrap_err(),
            "Invalid URL"
        );
    }
}
