//! Where an add-on may reach, and how its requests go: the network permission policy, the add-on's cookie jar and
//! OAuth state, and requests that follow redirects by hand, checking every hop (spec §4; Lokal
//! `morneye_network.dart`).

use std::collections::HashSet;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Map, Value, json};
use ureq::http;
use url::{Host, Url};

/// Redirects followed at most (hops 0 to 5).
const MAX_REDIRECTS: usize = 5;
/// The most an `http.*` answer may hold.
const MAX_RESPONSE: usize = 16 * 1024 * 1024;
/// Statuses that redirect when they carry a `Location`.
const REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];
/// Most install-time network entries a manifest may list.
const MAX_NETWORK_ENTRIES: usize = 100;

/// What a manifest's `permissions` allow (net.dart:48-81).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    /// Hosts: `example.com` (exactly) or `*.example.com` (any host under it, not itself).
    pub network: Vec<String>,
    pub storage: bool,
    pub file: bool,
    /// Plain `http` too (only `true` counts).
    pub allow_http: bool,
}

impl Permissions {
    /// The permissions a manifest's `permissions` value gives (lenient: what is not there is not allowed).
    pub fn of(value: &Value) -> Permissions {
        let network = value
            .get("network")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let flag = |key: &str| value.get(key).and_then(Value::as_bool) == Some(true);
        Permissions {
            network,
            storage: flag("storage"),
            file: flag("file"),
            allow_http: flag("allowHttp"),
        }
    }

    /// Whether a manifest's `permissions` value is valid to install (pkgs.dart:217-230): `network` a list of at most
    /// 100 strings matching `^(?:\*\.)?[a-z0-9.-]+$` (any case). The error is `Invalid addon network permissions`.
    pub fn check(value: &Value) -> Result<(), String> {
        let valid = value
            .get("network")
            .and_then(Value::as_array)
            .is_some_and(|list| {
                list.len() <= MAX_NETWORK_ENTRIES
                    && list
                        .iter()
                        .all(|entry| entry.as_str().is_some_and(valid_network_entry))
            });
        if valid {
            Ok(())
        } else {
            Err("Invalid addon network permissions".to_string())
        }
    }

    /// Whether `host` (lowercased) is allowed (net.dart:41-45).
    pub fn allows_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.network.iter().any(|entry| {
            let entry = entry.to_ascii_lowercase();
            match entry.strip_prefix("*.") {
                // `*.example.com` takes any host ending in `.example.com`, never `example.com` itself.
                Some(base) => host.ends_with(&entry[1..]) && host != base,
                None => host == entry,
            }
        })
    }

    /// The value the engine's config is given (`{"network": [...], "storage", "file", "allowHttp"}`).
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("network".into(), Value::from(self.network.clone()));
        map.insert("storage".into(), Value::Bool(self.storage));
        map.insert("file".into(), Value::Bool(self.file));
        // Lokal writes `allowHttp` only when it is on.
        if self.allow_http {
            map.insert("allowHttp".into(), Value::Bool(true));
        }
        Value::Object(map)
    }
}

/// `^(?:\*\.)?[a-z0-9.-]+$`, any case.
fn valid_network_entry(entry: &str) -> bool {
    let rest = entry.strip_prefix("*.").unwrap_or(entry);
    !rest.is_empty()
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// Checks `raw` against `permissions` (net.dart:84-104): `Invalid URL`, `Network permission denied for <host>`,
/// `This addon requires HTTPS`.
pub fn validate_url(raw: &str, permissions: &Permissions) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "Invalid URL".to_string())?;
    let host = host_of(&url)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "Invalid URL".to_string())?;
    if !url.username().is_empty() || url.password().is_some() || !permissions.allows_host(&host) {
        return Err(format!("Network permission denied for {host}"));
    }
    match url.scheme() {
        "https" => Ok(url),
        "http" if permissions.allow_http => Ok(url),
        _ => Err("This addon requires HTTPS".to_string()),
    }
}

/// The host as Lokal reads it: lowercased, an IPv6 address without its brackets.
pub(crate) fn host_of(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Domain(domain) => Some(domain.to_ascii_lowercase()),
        Host::Ipv4(address) => Some(address.to_string()),
        Host::Ipv6(address) => Some(address.to_string()),
    }
}

/// Whether `ip` is private, loopback, link-local, shared (100.64/10), unspecified or multicast (net.dart:20-38);
/// IPv4-mapped IPv6 is read as IPv4.
pub fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => private_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return private_v4(&v4);
            }
            let first = v6.segments()[0];
            v6.is_unspecified()
                || v6.is_loopback()
                // fc00::/7 (unique local)
                || first & 0xfe00 == 0xfc00
                // fe80::/10 (link-local)
                || first & 0xffc0 == 0xfe80
                // ff00::/8 (multicast)
                || first & 0xff00 == 0xff00
        }
    }
}

fn private_v4(ip: &Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
}

/// Whether a hop to `host` (resolved to `addresses`; `literal` when the URL named an IP) may go ahead
/// (net.dart:349-368): never when nothing resolved, and never to a private address, unless the host is literally
/// listed and is `localhost` or a private IP literal (a listed private *hostname* is still refused).
fn address_allowed(
    permissions: &Permissions,
    host: &str,
    literal: Option<IpAddr>,
    addresses: &[IpAddr],
) -> bool {
    let listed = permissions
        .network
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case(host));
    let explicit_local =
        listed && (host == "localhost" || literal.is_some_and(|ip| is_private(&ip)));
    !addresses.is_empty() && (explicit_local || !addresses.iter().any(is_private))
}

/// Resolves `host`, giving up after `timeout` (the lookup thread is left to finish on its own).
fn resolve(host: &str, port: u16, timeout: Duration) -> Result<Vec<IpAddr>, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let name = host.to_string();
    std::thread::Builder::new()
        .name("mornye-dns".into())
        .spawn(move || {
            let found = (name.as_str(), port)
                .to_socket_addrs()
                .map(|addresses| addresses.map(|address| address.ip()).collect::<Vec<_>>());
            let _ = sender.send(found);
        })
        .map_err(|error| error.to_string())?;
    match receiver.recv_timeout(timeout) {
        Ok(Ok(addresses)) => Ok(addresses),
        Ok(Err(_)) => Err(format!("Failed host lookup: '{host}'")),
        Err(_) => Err(format!("Host lookup timed out: '{host}'")),
    }
}

/// One stored cookie (net.dart:107-169); `expires` in ms.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cookie {
    name: String,
    value: String,
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    expires: Option<i64>,
}

impl Cookie {
    fn same_slot(&self, other: &Cookie) -> bool {
        self.domain == other.domain && self.path == other.path && self.name == other.name
    }

    fn expired(&self, now: i64) -> bool {
        self.expires.is_some_and(|expires| expires <= now)
    }

    fn matches(&self, url: &Url, now: i64) -> bool {
        if self.expired(now) {
            return false;
        }
        let Some(host) = host_of(url) else {
            return false;
        };
        let domain_ok = if self.host_only {
            host == self.domain
        } else {
            host == self.domain || host.ends_with(&format!(".{}", self.domain))
        };
        if !domain_ok || (self.secure && url.scheme() != "https") {
            return false;
        }
        let request_path = if url.path().is_empty() {
            "/"
        } else {
            url.path()
        };
        request_path == self.path
            || (request_path.starts_with(&self.path)
                && (self.path.ends_with('/')
                    || request_path.as_bytes().get(self.path.len()) == Some(&b'/')))
    }

    fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".into(), Value::String(self.name.clone()));
        map.insert("value".into(), Value::String(self.value.clone()));
        map.insert("domain".into(), Value::String(self.domain.clone()));
        map.insert("hostOnly".into(), Value::Bool(self.host_only));
        map.insert("path".into(), Value::String(self.path.clone()));
        map.insert("secure".into(), Value::Bool(self.secure));
        if let Some(expires) = self.expires {
            map.insert("expires".into(), Value::from(expires));
        }
        Value::Object(map)
    }

    fn from_json(value: &Value) -> Option<Cookie> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        Some(Cookie {
            name: text("name")?,
            value: text("value")?,
            domain: text("domain")?,
            host_only: value.get("hostOnly") != Some(&Value::Bool(false)),
            path: text("path").unwrap_or_else(|| "/".to_string()),
            secure: value.get("secure") == Some(&Value::Bool(true)),
            expires: value.get("expires").and_then(Value::as_i64),
        })
    }
}

/// A `Set-Cookie` value, read.
struct SetCookie {
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
    expires: Option<i64>,
    max_age: Option<i64>,
    secure: bool,
}

/// Reads one `Set-Cookie` value; `None` when it has no `name=value` or holds control characters.
fn parse_set_cookie(header: &str) -> Option<SetCookie> {
    let mut parts = header.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let (name, value) = (name.trim(), value.trim());
    let control = |text: &str| text.chars().any(|c| c.is_control() && c != '\t');
    if name.is_empty() || control(name) || control(value) {
        return None;
    }
    let mut cookie = SetCookie {
        name: name.to_string(),
        value: value.to_string(),
        domain: None,
        path: None,
        expires: None,
        max_age: None,
        secure: false,
    };
    for attribute in parts {
        let (key, argument) = match attribute.split_once('=') {
            Some((key, argument)) => (key.trim(), argument.trim()),
            None => (attribute.trim(), ""),
        };
        match key.to_ascii_lowercase().as_str() {
            "expires" => cookie.expires = parse_http_date(argument),
            "max-age" => cookie.max_age = argument.parse::<i64>().ok(),
            "domain" => cookie.domain = Some(argument.to_string()),
            "path" => cookie.path = Some(argument.to_string()),
            "secure" => cookie.secure = true,
            _ => {}
        }
    }
    Some(cookie)
}

/// The default cookie path of `url`: its path up to the last `/`, or `/`.
fn default_path(url: &Url) -> String {
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    match path.rfind('/') {
        Some(slash) if slash > 0 => path[..slash].to_string(),
        _ => "/".to_string(),
    }
}

/// A browser-like cookie jar (net.dart:106-252).
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    /// In the order first stored; a cookie set again keeps its place.
    cookies: Vec<Cookie>,
}

impl CookieJar {
    /// Read back from its saved form (`[{name, value, domain, hostOnly, path, secure, expires?}]`, expires in ms).
    pub fn from_json(value: &Value) -> CookieJar {
        let mut jar = CookieJar::default();
        for cookie in value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Cookie::from_json)
        {
            jar.put(cookie);
        }
        jar
    }

    pub fn to_json(&self) -> Value {
        let now = crate::now_ms();
        Value::Array(
            self.cookies
                .iter()
                .filter(|cookie| !cookie.expired(now))
                .map(Cookie::to_json)
                .collect(),
        )
    }

    /// The `Cookie` header for a request to `url`, when any cookie goes to it.
    pub fn header_for(&self, url: &Url) -> Option<String> {
        self.header_at(url, crate::now_ms())
    }

    /// Keeps the `Set-Cookie` values a response from `url` gave.
    pub fn store(&mut self, url: &Url, set_cookies: &[String]) {
        let now = crate::now_ms();
        for header in set_cookies {
            self.store_at(url, header, now);
        }
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    fn put(&mut self, cookie: Cookie) {
        match self.cookies.iter_mut().find(|kept| kept.same_slot(&cookie)) {
            Some(slot) => *slot = cookie,
            None => self.cookies.push(cookie),
        }
    }

    fn header_at(&self, url: &Url, now: i64) -> Option<String> {
        let mut matching: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|cookie| cookie.matches(url, now))
            .collect();
        // Longer paths first, as browsers send them.
        matching.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        let header = matching
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");
        (!header.is_empty()).then_some(header)
    }

    fn store_at(&mut self, url: &Url, header: &str, now: i64) {
        let Some(parsed) = parse_set_cookie(header) else {
            return;
        };
        let host = host_of(url).unwrap_or_default();
        let mut domain = parsed.domain.unwrap_or_default().to_ascii_lowercase();
        if let Some(rest) = domain.strip_prefix('.') {
            domain = rest.to_string();
        }
        let host_only = domain.is_empty();
        if host_only {
            domain = host;
        } else if host != domain && !host.ends_with(&format!(".{domain}")) {
            // A site cannot set cookies for another site.
            return;
        }
        let path = match parsed.path {
            Some(path) if path.starts_with('/') => path,
            _ => default_path(url),
        };
        let expires = match parsed.max_age {
            // Max-Age wins over Expires.
            Some(seconds) => Some(now.saturating_add(seconds.saturating_mul(1000))),
            None => parsed.expires,
        };
        let cookie = Cookie {
            name: parsed.name,
            value: parsed.value,
            domain,
            host_only,
            path,
            secure: parsed.secure,
            expires,
        };
        if cookie.expired(now) {
            self.cookies.retain(|kept| !kept.same_slot(&cookie));
        } else {
            self.put(cookie);
        }
    }
}

/// An HTTP date (RFC 1123, RFC 850 or asctime), read the lenient way RFC 6265 §5.1.1 reads cookie dates; in ms.
pub(crate) fn parse_http_date(text: &str) -> Option<i64> {
    let delimiter = |c: char| matches!(c, '\t' | ' '..='/' | ';'..='@' | '['..='`' | '{'..='~');
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for token in text.split(delimiter).filter(|token| !token.is_empty()) {
        if time.is_none()
            && let Some(found) = date_time_of(token)
        {
            time = Some(found);
        } else if day.is_none()
            && let Some(found) = leading_number(token, 1, 2)
        {
            day = Some(found);
        } else if month.is_none()
            && let Some(found) = month_of(token)
        {
            month = Some(found);
        } else if year.is_none()
            && let Some(found) = leading_number(token, 2, 4)
        {
            year = Some(found);
        }
    }
    let (hour, minute, second) = time?;
    let (day, month, mut year) = (day?, month?, year?);
    if (70..=99).contains(&year) {
        year += 1900;
    } else if (0..=69).contains(&year) {
        year += 2000;
    }
    if !(1..=31).contains(&day) || year < 1601 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day as u32);
    Some(days * 86_400_000 + ((hour * 60 + minute) * 60 + second) * 1000)
}

/// `hh:mm:ss` at the start of `token` (1 or 2 digits each), followed by nothing or a non-digit.
fn date_time_of(token: &str) -> Option<(i64, i64, i64)> {
    let mut fields = token.splitn(3, ':');
    let hour = whole_number(fields.next()?, 1, 2)?;
    let minute = whole_number(fields.next()?, 1, 2)?;
    let second = leading_number(fields.next()?, 1, 2)?;
    Some((hour, minute, second))
}

/// `token` when it is all digits, `min..=max` of them.
fn whole_number(token: &str, min: usize, max: usize) -> Option<i64> {
    (token.bytes().all(|b| b.is_ascii_digit()) && (min..=max).contains(&token.len()))
        .then(|| token.parse().ok())
        .flatten()
}

/// The `min..=max` digits `token` starts with (anything after them must not be a digit).
fn leading_number(token: &str, min: usize, max: usize) -> Option<i64> {
    let count = token.bytes().take_while(u8::is_ascii_digit).count();
    (min..=max)
        .contains(&count)
        .then(|| token[..count].parse().ok())
        .flatten()
}

fn month_of(token: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let head = token.get(..3)?.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|month| *month == head)
        .map(|index| index as u32 + 1)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// How an add-on's network is made.
pub struct NetworkSetup {
    pub permissions: Permissions,
    /// Sent when the add-on sets none (`Lokal/1.0.0` in Lokal; Stash sends its own).
    pub user_agent: String,
    /// For each of resolving, connecting, waiting for the answer, and each gap in its body (30 s; 75 s for an
    /// add-on whose name holds `tidal`).
    pub timeout: Duration,
    /// The saved `connection` record (`{auth, cookies}`), or `{}`.
    pub saved: Value,
    /// Keeps the `connection` record (`{auth: <auth without "pending">, cookies: <jar>}`) whenever it changed.
    pub keep: Arc<dyn Fn(Value) + Send + Sync>,
}

/// An answer whose body is still to be read: status, the address it came from after redirects, headers (names
/// lowercased, repeated values joined with ", "), and the body (already decompressed).
pub struct Answer {
    pub status: u16,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

impl Answer {
    /// The header `name` (lowercase).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// One add-on's network: its permissions, cookie jar and OAuth state, shared by every runtime of the add-on.
pub struct Network {
    permissions: Permissions,
    user_agent: String,
    timeout: Duration,
    keeper: Arc<dyn Fn(Value) + Send + Sync>,
    agent: ureq::Agent,
    jar: Mutex<CookieJar>,
    auth: Mutex<Map<String, Value>>,
    generation: AtomicU64,
    /// Keeps the connection record one snapshot at a time, so an older one never lands last.
    keeping: Mutex<()>,
}

impl Network {
    pub fn new(setup: NetworkSetup) -> Network {
        let auth = setup
            .saved
            .get("auth")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let jar = setup
            .saved
            .get("cookies")
            .map(CookieJar::from_json)
            .unwrap_or_default();
        let timeout = Some(setup.timeout);
        let config = ureq::Agent::config_builder()
            // Redirects are followed by hand so each hop is checked; a 4xx or 5xx is an answer, not an error.
            .max_redirects(0)
            .http_status_as_error(false)
            // Straight to the host that was checked, as Lokal does (no proxy from the environment).
            .proxy(None)
            .user_agent(setup.user_agent.as_str())
            .timeout_resolve(timeout)
            .timeout_connect(timeout)
            .timeout_send_request(timeout)
            .timeout_send_body(timeout)
            .timeout_recv_response(timeout)
            .timeout_per_read(timeout)
            .build();
        Network {
            permissions: setup.permissions,
            user_agent: setup.user_agent,
            timeout: setup.timeout,
            keeper: setup.keep,
            agent: ureq::Agent::new_with_config(config),
            jar: Mutex::new(jar),
            auth: Mutex::new(auth),
            generation: AtomicU64::new(0),
            keeping: Mutex::new(()),
        }
    }

    pub fn permissions(&self) -> &Permissions {
        &self.permissions
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Bumped when the person disconnects the add-on: an auth call that saw another generation answers
    /// `Disconnected`.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The add-on's OAuth state (`code`, `access_token`, `refresh_token`, `expires_at`, `pkce`, `pending`, …).
    /// Drop the guard before calling [`Network::keep`] or making a request.
    pub fn auth(&self) -> MutexGuard<'_, Map<String, Value>> {
        lock(&self.auth)
    }

    /// Keeps the connection record now (after the auth state or the cookies changed).
    pub fn keep(&self) {
        let _keeping = lock(&self.keeping);
        let mut auth = self.auth().clone();
        auth.remove("pending");
        let cookies = self.jar().to_json();
        (self.keeper)(json!({ "auth": auth, "cookies": cookies }));
    }

    /// `http.clearCookies`: empties the jar and keeps it.
    pub fn clear_cookies(&self) {
        self.jar().clear();
        self.keep();
    }

    /// Logging out: the generation moves on, the auth state and the cookies are emptied, and kept.
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.auth().clear();
        self.jar().clear();
        self.keep();
    }

    /// One request (net.dart:371-456): every hop checked with [`validate_url`] and its addresses resolved and
    /// refused when private (unless the host is literally listed and is `localhost` or a private IP literal); at
    /// most 5 redirects (301/302/303/307/308 with a Location), followed by hand; on a change of origin the
    /// add-on's `authorization`, `cookie` and `x-…(token|key|signature|session)` headers are dropped; 303, or
    /// 301/302 after a POST, become a GET without body. Dropped from the add-on's headers always: `host`,
    /// `content-length`, `connection`, `transfer-encoding`, `proxy-authorization`. The jar's cookies are sent
    /// (unless the add-on set `Cookie`) and every hop's `Set-Cookie` kept, except for `direct_media`, which also
    /// never follows a redirect (`Direct-media downloads do not follow redirects`). The body is sent only for
    /// methods other than GET and HEAD, and only when not empty. `body_gap` (a download's) replaces the
    /// per-gap timeout of the body when given.
    pub fn request(
        &self,
        url: &str,
        method: &str,
        body: &[u8],
        headers: &[(String, String)],
        direct_media: bool,
        body_gap: Option<Duration>,
    ) -> Result<Answer, String> {
        let mut url = validate_url(url, &self.permissions)?;
        let mut method = method.trim().to_ascii_uppercase();
        if method.is_empty() {
            method = "GET".to_string();
        }
        let mut body = Some(body);
        let mut sent: Vec<(String, String)> = headers
            .iter()
            .filter(|(name, _)| !dropped_header(name))
            .cloned()
            .collect();
        for _hop in 0..=MAX_REDIRECTS {
            self.check_address(&url)?;
            let response = self.send(&url, &method, body, &sent, direct_media, body_gap)?;
            let status = response.status().as_u16();
            if !direct_media {
                let set_cookies: Vec<String> = response
                    .headers()
                    .get_all("set-cookie")
                    .iter()
                    .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
                    .collect();
                if !set_cookies.is_empty() {
                    self.jar().store(&url, &set_cookies);
                    self.keep();
                }
            }
            let location = response
                .headers()
                .get("location")
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
            if REDIRECTS.contains(&status)
                && let Some(location) = location
            {
                // The redirect's own body is not read: dropping it closes that connection.
                drop(response);
                if direct_media {
                    return Err("Direct-media downloads do not follow redirects".to_string());
                }
                let next = url.join(&location).map_err(|_| "Invalid URL".to_string())?;
                let next = validate_url(next.as_str(), &self.permissions)?;
                if next.origin() != url.origin() {
                    sent.retain(|(name, _)| !credential_header(name));
                }
                if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                    method = "GET".to_string();
                    body = None;
                }
                url = next;
                continue;
            }
            let headers = joined_headers(response.headers());
            let reader = response.into_body().into_reader();
            return Ok(Answer {
                status,
                url,
                headers,
                body: Box::new(reader),
            });
        }
        Err("Too many addon redirects".to_string())
    }

    /// `http.*` (net.dart:460-494): the whole answer, at most 16 MiB (`Addon network response exceeds 16 MB`), as
    /// `{statusCode, status, ok, url, headers, body}` (+ `binary`, base64 of the raw bytes, when `as_fetch`). Not
    /// 2xx is not an error.
    pub fn json_response(
        &self,
        url: &str,
        method: &str,
        body: &str,
        headers: &[(String, String)],
        as_fetch: bool,
    ) -> Result<Value, String> {
        let answer = self.request(url, method, body.as_bytes(), headers, false, None)?;
        let data = read_limited(answer.body, MAX_RESPONSE)?;
        let mut header_map = Map::new();
        for (name, value) in &answer.headers {
            header_map.insert(name.clone(), Value::String(value.clone()));
        }
        let status = answer.status;
        let mut out = Map::new();
        out.insert("statusCode".into(), Value::from(status));
        out.insert("status".into(), Value::from(status));
        out.insert("ok".into(), Value::Bool((200..300).contains(&status)));
        out.insert("url".into(), Value::String(answer.url.to_string()));
        out.insert("headers".into(), Value::Object(header_map));
        out.insert(
            "body".into(),
            Value::String(String::from_utf8_lossy(&data).into_owned()),
        );
        if as_fetch {
            out.insert("binary".into(), Value::String(crate::bytes::base64(&data)));
        }
        Ok(Value::Object(out))
    }

    fn jar(&self) -> MutexGuard<'_, CookieJar> {
        lock(&self.jar)
    }

    /// Resolves the hop's host and refuses it when it lands on a private address it may not reach.
    fn check_address(&self, url: &Url) -> Result<(), String> {
        let host = host_of(url).ok_or_else(|| "Invalid URL".to_string())?;
        let literal: Option<IpAddr> = match url.host() {
            Some(Host::Ipv4(address)) => Some(address.into()),
            Some(Host::Ipv6(address)) => Some(address.into()),
            Some(Host::Domain(domain)) => domain.parse().ok(),
            None => None,
        };
        let addresses = match literal {
            Some(address) => vec![address],
            None => resolve(
                &host,
                url.port_or_known_default().unwrap_or(443),
                self.timeout,
            )?,
        };
        if address_allowed(&self.permissions, &host, literal, &addresses) {
            Ok(())
        } else {
            Err(
                "Addon request resolved to a private address without explicit permission"
                    .to_string(),
            )
        }
    }

    /// One hop, as it goes on the wire.
    fn send(
        &self,
        url: &Url,
        method: &str,
        body: Option<&[u8]>,
        sent: &[(String, String)],
        direct_media: bool,
        body_gap: Option<Duration>,
    ) -> Result<http::Response<ureq::Body>, String> {
        let names: HashSet<String> = sent
            .iter()
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect();
        let cookie = if direct_media {
            None
        } else {
            self.jar().header_for(url)
        };
        let mut headers = http::HeaderMap::new();
        if !names.contains("user-agent") && !self.user_agent.is_empty() {
            set_header(&mut headers, "User-Agent", &self.user_agent);
        }
        if let Some(cookie) = cookie
            && !names.contains("cookie")
        {
            set_header(&mut headers, "Cookie", &cookie);
        }
        for (name, value) in sent {
            set_header(&mut headers, name, value);
        }
        let method = http::Method::from_bytes(method.as_bytes())
            .map_err(|_| format!("Invalid HTTP method: {method}"))?;
        let mut target = url.clone();
        target.set_fragment(None);
        let uri: http::Uri = target
            .as_str()
            .parse()
            .map_err(|_| "Invalid URL".to_string())?;
        let bodyless = method == http::Method::GET || method == http::Method::HEAD;
        // An empty POST, PUT or PATCH still says `Content-Length: 0`, as browsers do.
        let says_length = method == http::Method::POST
            || method == http::Method::PUT
            || method == http::Method::PATCH;
        let payload = body.filter(|bytes| !bytes.is_empty() && !bodyless);
        let result = match payload {
            Some(bytes) => self.run(build(method, uri, headers, bytes.to_vec()), body_gap),
            None if says_length => self.run(build(method, uri, headers, Vec::new()), body_gap),
            None => self.run(build(method, uri, headers, ()), body_gap),
        };
        result.map_err(|error| error.to_string())
    }

    fn run<S: ureq::AsSendBody>(
        &self,
        request: http::Request<S>,
        body_gap: Option<Duration>,
    ) -> Result<http::Response<ureq::Body>, ureq::Error> {
        match body_gap {
            Some(gap) => {
                let request = self
                    .agent
                    .configure_request(request)
                    .timeout_per_read(Some(gap))
                    .build();
                self.agent.run(request)
            }
            None => self.agent.run(request),
        }
    }
}

fn build<S>(
    method: http::Method,
    uri: http::Uri,
    headers: http::HeaderMap,
    body: S,
) -> http::Request<S> {
    let mut request = http::Request::new(body);
    *request.method_mut() = method;
    *request.uri_mut() = uri;
    *request.headers_mut() = headers;
    request
}

/// Sets (replaces) a header; one the HTTP library refuses is left out, as in Lokal.
fn set_header(headers: &mut http::HeaderMap, name: &str, value: &str) {
    if let (Ok(name), Ok(value)) = (
        http::HeaderName::from_bytes(name.as_bytes()),
        http::HeaderValue::from_bytes(value.as_bytes()),
    ) {
        headers.insert(name, value);
    }
}

/// `^(host|content-length|connection|transfer-encoding|proxy-authorization)$`, any case.
fn dropped_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host" | "content-length" | "connection" | "transfer-encoding" | "proxy-authorization"
    )
}

/// `^(authorization|cookie|x-.*(?:token|key|signature|session))$`, any case: not sent to another origin.
fn credential_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "authorization"
        || name == "cookie"
        || (name.starts_with("x-")
            && ["token", "key", "signature", "session"]
                .iter()
                .any(|end| name.ends_with(end)))
}

/// Names lowercased, repeated values joined with ", ".
fn joined_headers(headers: &http::HeaderMap) -> Vec<(String, String)> {
    headers
        .keys()
        .map(|name| {
            let values: Vec<String> = headers
                .get_all(name)
                .iter()
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
                .collect();
            (name.as_str().to_string(), values.join(", "))
        })
        .collect()
}

/// The whole body, failing past `limit` bytes.
fn read_limited(body: Box<dyn Read + Send>, limit: usize) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    body.take(limit as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    if data.len() > limit {
        return Err("Addon network response exceeds 16 MB".to_string());
    }
    Ok(data)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Headers as an add-on gives them (a JSON object of name → value; other values are made strings, nulls skipped).
pub fn headers_of(value: Option<&Value>) -> Vec<(String, String)> {
    let Some(Value::Object(map)) = value else {
        return Vec::new();
    };
    map.iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(name, value)| (name.clone(), text_of(value)))
        .collect()
}

/// A JSON value as Dart's string interpolation writes it: a string as it is, `null` as `null`, others as JSON.
pub(crate) fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A loopback HTTP server for tests, and a network allowed to reach it.
#[cfg(test)]
pub(crate) mod testing {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use serde_json::Value;

    use super::{Network, NetworkSetup, Permissions};

    /// A request the server received.
    #[derive(Debug, Clone)]
    pub(crate) struct Seen {
        pub(crate) method: String,
        /// The path and query.
        pub(crate) target: String,
        pub(crate) headers: Vec<(String, String)>,
        pub(crate) body: Vec<u8>,
    }

    impl Seen {
        pub(crate) fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }

        pub(crate) fn path(&self) -> &str {
            self.target.split('?').next().unwrap_or("")
        }
    }

    /// What the server answers.
    pub(crate) struct Reply {
        pub(crate) status: u16,
        pub(crate) headers: Vec<(String, String)>,
        pub(crate) body: Vec<u8>,
    }

    impl Reply {
        pub(crate) fn new(status: u16, body: impl Into<Vec<u8>>) -> Reply {
            Reply {
                status,
                headers: Vec::new(),
                body: body.into(),
            }
        }

        pub(crate) fn json(status: u16, body: &Value) -> Reply {
            Reply::new(status, body.to_string()).with("Content-Type", "application/json")
        }

        pub(crate) fn with(mut self, name: &str, value: &str) -> Reply {
            self.headers.push((name.to_string(), value.to_string()));
            self
        }
    }

    type Handler = Arc<dyn Fn(&Seen) -> Reply + Send + Sync>;

    pub(crate) struct Server {
        pub(crate) port: u16,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Server {
        /// Serves each connection one request, answered by `handler`, on 127.0.0.1.
        pub(crate) fn start(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            let port = listener.local_addr().expect("its address").port();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let handler: Handler = Arc::new(handler);
            let log = Arc::clone(&seen);
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let handler = Arc::clone(&handler);
                    let log = Arc::clone(&log);
                    std::thread::spawn(move || answer(stream, &handler, &log));
                }
            });
            Server { port, seen }
        }

        pub(crate) fn base(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        pub(crate) fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    fn answer(stream: TcpStream, handler: &Handler, log: &Mutex<Vec<Seen>>) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let mut headers = Vec::new();
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                break;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                headers.push((name.trim().to_string(), value.trim().to_string()));
            }
        }
        let length = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0; length];
        let _ = reader.read_exact(&mut body);
        let seen = Seen {
            method,
            target,
            headers,
            body,
        };
        log.lock().unwrap().push(seen.clone());
        let reply = handler(&seen);
        let mut head = format!("HTTP/1.1 {} Test\r\n", reply.status);
        for (name, value) in &reply.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            reply.body.len()
        ));
        let _ = writer.write_all(head.as_bytes());
        let _ = writer.write_all(&reply.body);
        let _ = writer.flush();
    }

    /// A network that may reach 127.0.0.1 over plain HTTP (and `example.com`, `*.example.com`), with what it kept.
    pub(crate) fn network(saved: Value) -> (Network, Arc<Mutex<Vec<Value>>>) {
        let kept = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&kept);
        let network = Network::new(NetworkSetup {
            permissions: Permissions {
                network: vec![
                    "127.0.0.1".into(),
                    "example.com".into(),
                    "*.example.com".into(),
                ],
                storage: true,
                file: false,
                allow_http: true,
            },
            user_agent: "Stash-Test/1".into(),
            timeout: Duration::from_secs(5),
            saved,
            keep: Arc::new(move |value| log.lock().unwrap().push(value)),
        });
        (network, kept)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Reply, Server, network};
    use super::*;

    fn permissions(network: &[&str]) -> Permissions {
        Permissions {
            network: network.iter().map(|s| s.to_string()).collect(),
            ..Permissions::default()
        }
    }

    #[test]
    fn permissions_read_leniently() {
        let read = Permissions::of(&json!({
            "network": ["api.example.com", 5, "*.cdn.example.com"],
            "storage": true,
            "file": "yes",
            "allowHttp": true,
        }));
        assert_eq!(read.network, vec!["api.example.com", "*.cdn.example.com"]);
        assert!(read.storage && !read.file && read.allow_http);
        assert_eq!(Permissions::of(&json!(null)), Permissions::default());
        assert_eq!(
            read.to_json(),
            json!({"network": ["api.example.com", "*.cdn.example.com"], "storage": true, "file": false, "allowHttp": true})
        );
        assert!(Permissions::default().to_json().get("allowHttp").is_none());
    }

    #[test]
    fn install_check_follows_the_pattern() {
        assert!(
            Permissions::check(&json!({"network": ["api.example.com", "*.Example.COM", "a-b.c"]}))
                .is_ok()
        );
        assert!(Permissions::check(&json!({"network": []})).is_ok());
        for bad in [
            json!({}),
            json!({"network": "example.com"}),
            json!({"network": ["*."]}),
            json!({"network": ["*.*.example.com"]}),
            json!({"network": ["example.com/path"]}),
            json!({"network": ["exa mple.com"]}),
            json!({"network": [1]}),
        ] {
            assert_eq!(
                Permissions::check(&bad),
                Err("Invalid addon network permissions".to_string()),
                "{bad}"
            );
        }
        let many: Vec<String> = (0..101).map(|i| format!("h{i}.example.com")).collect();
        assert!(Permissions::check(&json!({ "network": many })).is_err());
    }

    #[test]
    fn wildcards_take_subdomains_only() {
        let allowed = permissions(&["*.example.com", "API.example.org"]);
        assert!(allowed.allows_host("a.example.com"));
        assert!(allowed.allows_host("a.b.example.com"));
        assert!(!allowed.allows_host("example.com"));
        assert!(!allowed.allows_host("badexample.com"));
        assert!(allowed.allows_host("api.example.org"));
        assert!(!allowed.allows_host("www.api.example.org"));
    }

    #[test]
    fn urls_are_checked() {
        let allowed = permissions(&["example.com", "*.example.net"]);
        assert!(validate_url("https://example.com/x", &allowed).is_ok());
        assert!(validate_url("https://EXAMPLE.com/x", &allowed).is_ok());
        assert_eq!(
            validate_url("not a url", &allowed).unwrap_err(),
            "Invalid URL"
        );
        assert_eq!(
            validate_url("/relative", &allowed).unwrap_err(),
            "Invalid URL"
        );
        assert_eq!(
            validate_url("https://other.example.org/", &allowed).unwrap_err(),
            "Network permission denied for other.example.org"
        );
        assert_eq!(
            validate_url("https://user:pass@example.com/", &allowed).unwrap_err(),
            "Network permission denied for example.com"
        );
        assert_eq!(
            validate_url("http://example.com/", &allowed).unwrap_err(),
            "This addon requires HTTPS"
        );
        let with_http = Permissions {
            allow_http: true,
            ..allowed.clone()
        };
        assert!(validate_url("http://a.example.net/", &with_http).is_ok());
        assert_eq!(
            validate_url("ftp://example.com/", &with_http).unwrap_err(),
            "This addon requires HTTPS"
        );
    }

    #[test]
    fn private_addresses() {
        for private in [
            "0.1.2.3",
            "10.0.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "100.64.0.1",
            "100.127.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "febf::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
        ] {
            assert!(is_private(&private.parse().unwrap()), "{private}");
        }
        for public in [
            "8.8.8.8",
            "172.15.0.1",
            "172.32.0.1",
            "100.63.0.1",
            "100.128.0.1",
            "169.253.1.1",
            "2001:db8::1",
            "fec0::1",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_private(&public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn private_hops_need_a_listed_literal_or_localhost() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let public: IpAddr = "93.184.216.34".parse().unwrap();
        let listed = permissions(&[
            "127.0.0.1",
            "localhost",
            "intranet.example.com",
            "*.example.com",
        ]);
        assert!(address_allowed(
            &listed,
            "127.0.0.1",
            Some(loopback),
            &[loopback]
        ));
        assert!(address_allowed(&listed, "localhost", None, &[loopback]));
        // A listed private hostname is still refused.
        let inner: IpAddr = "10.0.0.5".parse().unwrap();
        assert!(!address_allowed(
            &listed,
            "intranet.example.com",
            None,
            &[inner]
        ));
        assert!(!address_allowed(
            &listed,
            "cdn.example.com",
            None,
            &[public, inner]
        ));
        assert!(address_allowed(&listed, "cdn.example.com", None, &[public]));
        assert!(!address_allowed(&listed, "cdn.example.com", None, &[]));
        // Not listed literally: a private literal is refused.
        assert!(!address_allowed(
            &permissions(&[]),
            "127.0.0.1",
            Some(loopback),
            &[loopback]
        ));
    }

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn cookies_keep_to_their_host_and_path() {
        let mut jar = CookieJar::default();
        let now = 1_000_000;
        let page = url("https://www.example.com/app/page");
        jar.store_at(&page, "host=1", now);
        jar.store_at(&page, "wide=2; Domain=.Example.com; Path=/", now);
        jar.store_at(&page, "deep=3; Path=/app/inner", now);
        jar.store_at(&page, "safe=4; Secure; Path=/", now);
        jar.store_at(&page, "other=5; Domain=example.org", now);
        // `host` takes the default path `/app`.
        assert_eq!(
            jar.header_at(&url("https://www.example.com/app/x"), now)
                .as_deref(),
            Some("host=1; wide=2; safe=4")
        );
        assert_eq!(
            jar.header_at(&url("https://www.example.com/app/inner/x"), now)
                .as_deref(),
            Some("deep=3; host=1; wide=2; safe=4")
        );
        // Host-only stays on its host; a domain cookie goes to subdomains.
        assert_eq!(
            jar.header_at(&url("https://api.example.com/"), now)
                .as_deref(),
            Some("wide=2")
        );
        // `/application` is not under `/app`; plain http gets no secure cookie.
        assert_eq!(
            jar.header_at(&url("http://www.example.com/application"), now)
                .as_deref(),
            Some("wide=2")
        );
        assert_eq!(jar.header_at(&url("https://example.org/"), now), None);
    }

    #[test]
    fn cookies_expire_and_max_age_wins() {
        let mut jar = CookieJar::default();
        let now = 1_700_000_000_000;
        let site = url("https://example.com/");
        jar.store_at(
            &site,
            "a=1; Max-Age=60; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
            now,
        );
        jar.store_at(&site, "b=2; Expires=Wed, 21 Oct 2099 07:28:00 GMT", now);
        assert_eq!(jar.header_at(&site, now).as_deref(), Some("a=1; b=2"));
        assert_eq!(jar.header_at(&site, now + 61_000).as_deref(), Some("b=2"));
        // Set again with a value: it keeps its place.
        jar.store_at(&site, "a=9; Max-Age=60", now);
        assert_eq!(jar.header_at(&site, now).as_deref(), Some("a=9; b=2"));
        // Expired: removed.
        jar.store_at(&site, "a=; Max-Age=0", now);
        jar.store_at(&site, "b=; Expires=Thu, 01 Jan 1970 00:00:00 GMT", now);
        assert_eq!(jar.header_at(&site, now), None);
        assert!(jar.cookies.is_empty());
        // Malformed values are ignored.
        jar.store_at(&site, "no-equals-sign", now);
        jar.store_at(&site, "=nameless", now);
        assert!(jar.cookies.is_empty());
    }

    #[test]
    fn cookies_round_trip_their_saved_form() {
        let mut jar = CookieJar::default();
        let now = crate::now_ms();
        let site = url("https://www.example.com/a/b");
        jar.store_at(
            &site,
            "s=1; Domain=example.com; Path=/; Secure; Max-Age=3600",
            now,
        );
        jar.store_at(&site, "h=2", now);
        let saved = jar.to_json();
        assert_eq!(saved[0]["domain"], "example.com");
        assert_eq!(saved[0]["hostOnly"], false);
        assert_eq!(saved[0]["secure"], true);
        assert!(saved[0]["expires"].as_i64().unwrap() > now);
        assert_eq!(saved[1]["path"], "/a");
        assert_eq!(saved[1]["hostOnly"], true);
        assert!(saved[1].get("expires").is_none());
        let back = CookieJar::from_json(&saved);
        assert_eq!(back.cookies, jar.cookies);
        // Lenient read: missing fields take defaults, broken entries are skipped.
        let read = CookieJar::from_json(
            &json!([{"name": "n", "value": "v", "domain": "example.com"}, {"name": 1}]),
        );
        assert_eq!(read.cookies.len(), 1);
        assert!(read.cookies[0].host_only);
        assert_eq!(read.cookies[0].path, "/");
    }

    #[test]
    fn http_dates() {
        let expected = Some(784_111_777_000);
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), expected);
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), expected);
        assert_eq!(parse_http_date("Sun Nov  6 08:49:37 1994"), expected);
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            parse_http_date("Wed, 29 Feb 2024 12:00:00 GMT"),
            Some(1_709_208_000_000)
        );
        assert_eq!(parse_http_date("not a date"), None);
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 25:49:37 GMT"), None);
        assert_eq!(parse_http_date("Sun, 06 Xyz 1994 08:49:37 GMT"), None);
    }

    #[test]
    fn headers_from_the_add_on() {
        let headers = headers_of(Some(&json!({"A": "1", "B": 2, "C": null, "D": true})));
        assert_eq!(
            headers,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string()),
                ("D".to_string(), "true".to_string()),
            ]
        );
        assert!(headers_of(Some(&json!([1]))).is_empty());
        assert!(headers_of(None).is_empty());
        assert!(credential_header("Authorization"));
        assert!(credential_header("X-Api-Key"));
        assert!(credential_header("x-session"));
        assert!(credential_header("X-Auth-Token"));
        assert!(!credential_header("X-Other"));
        assert!(!credential_header("Accept"));
        assert!(dropped_header("Content-Length"));
        assert!(!dropped_header("Content-Type"));
    }

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn answers_come_back_whole() {
        let server = Server::start(|_| {
            Reply::new(200, "hello é")
                .with("X-Multi", "a")
                .with("X-Multi", "b")
        });
        let (network, _) = network(json!({}));
        let answer = network
            .json_response(&format!("{}/path?q=1", server.base()), "GET", "", &[], true)
            .unwrap();
        assert_eq!(answer["statusCode"], 200);
        assert_eq!(answer["status"], 200);
        assert_eq!(answer["ok"], true);
        assert_eq!(answer["url"], format!("{}/path?q=1", server.base()));
        assert_eq!(answer["headers"]["x-multi"], "a, b");
        assert_eq!(answer["body"], "hello é");
        assert_eq!(answer["binary"], crate::bytes::base64("hello é".as_bytes()));
        let seen = server.seen();
        assert_eq!(seen[0].method, "GET");
        assert_eq!(seen[0].target, "/path?q=1");
        assert_eq!(seen[0].header("user-agent"), Some("Stash-Test/1"));
        assert!(seen[0].body.is_empty());
        // Without `as_fetch` there is no `binary`.
        let plain = network
            .json_response(&server.base(), "GET", "", &[], false)
            .unwrap();
        assert!(plain.get("binary").is_none());
    }

    #[test]
    fn errors_are_answers_and_bodies_go_only_with_their_methods() {
        let server =
            Server::start(|seen| Reply::new(404, format!("{} {}", seen.method, seen.body.len())));
        let (network, _) = network(json!({}));
        let base = server.base();
        let sent = headers(&[
            ("Content-Type", "text/plain"),
            ("Host", "evil.example.com"),
            ("User-Agent", "Mine"),
        ]);
        let posted = network
            .json_response(&base, "post", "abc", &sent, false)
            .unwrap();
        assert_eq!(posted["ok"], false);
        assert_eq!(posted["status"], 404);
        assert_eq!(posted["body"], "POST 3");
        let got = network
            .json_response(&base, "GET", "abc", &[], false)
            .unwrap();
        assert_eq!(got["body"], "GET 0");
        let seen = server.seen();
        assert_eq!(seen[0].header("content-type"), Some("text/plain"));
        assert_eq!(seen[0].header("user-agent"), Some("Mine"));
        assert_eq!(
            seen[0].header("host"),
            Some(format!("127.0.0.1:{}", server.port).as_str())
        );
        assert_eq!(seen[1].header("content-length"), None);
    }

    #[test]
    fn redirects_drop_credentials_across_origins() {
        let landing = Server::start(|_| Reply::new(200, "landed"));
        let target = format!("{}/landed", landing.base());
        let start = Server::start(move |seen| match seen.path() {
            "/away" => Reply::new(302, "").with("Location", &target),
            "/near" => Reply::new(307, "").with("Location", "/here"),
            _ => Reply::new(200, "here"),
        });
        let (network, _) = network(json!({}));
        let sent = headers(&[
            ("Authorization", "Bearer t"),
            ("X-Api-Key", "k"),
            ("Cookie", "c=1"),
            ("X-Other", "kept"),
        ]);
        let away = network
            .json_response(&format!("{}/away", start.base()), "GET", "", &sent, false)
            .unwrap();
        assert_eq!(away["url"], format!("{}/landed", landing.base()));
        assert_eq!(away["body"], "landed");
        let far = &landing.seen()[0];
        assert_eq!(far.header("x-other"), Some("kept"));
        assert_eq!(far.header("authorization"), None);
        assert_eq!(far.header("x-api-key"), None);
        assert_eq!(far.header("cookie"), None);
        // Same origin: the credentials stay, and a 307 keeps the POST and its body.
        let near = network
            .json_response(
                &format!("{}/near", start.base()),
                "POST",
                "data",
                &sent,
                false,
            )
            .unwrap();
        assert_eq!(near["body"], "here");
        let here = start
            .seen()
            .into_iter()
            .find(|seen| seen.path() == "/here")
            .unwrap();
        assert_eq!(here.method, "POST");
        assert_eq!(here.body, b"data");
        assert_eq!(here.header("authorization"), Some("Bearer t"));
    }

    #[test]
    fn see_other_becomes_a_get() {
        let server = Server::start(|seen| match seen.path() {
            "/form" => Reply::new(303, "").with("Location", "/done"),
            "/moved" => Reply::new(301, "").with("Location", "/done"),
            _ => Reply::new(200, format!("{} {}", seen.method, seen.body.len())),
        });
        let (network, _) = network(json!({}));
        let form = network
            .json_response(&format!("{}/form", server.base()), "PUT", "x=1", &[], false)
            .unwrap();
        assert_eq!(form["body"], "GET 0");
        let moved = network
            .json_response(
                &format!("{}/moved", server.base()),
                "POST",
                "x=1",
                &[],
                false,
            )
            .unwrap();
        assert_eq!(moved["body"], "GET 0");
    }

    #[test]
    fn redirect_limits() {
        let server = Server::start(|seen| match seen.path() {
            "/loop" => Reply::new(302, "").with("Location", "/loop"),
            "/out" => Reply::new(302, "").with("Location", "https://elsewhere.example.org/"),
            "/media" => Reply::new(302, "").with("Location", "/file"),
            // A redirect status without a Location is the answer.
            _ => Reply::new(302, "plain"),
        });
        let (network, _) = network(json!({}));
        let base = server.base();
        let looped = network.json_response(&format!("{base}/loop"), "GET", "", &[], false);
        assert_eq!(looped.unwrap_err(), "Too many addon redirects");
        assert_eq!(server.seen().len(), 6);
        let out = network.json_response(&format!("{base}/out"), "GET", "", &[], false);
        assert_eq!(
            out.unwrap_err(),
            "Network permission denied for elsewhere.example.org"
        );
        let media = network.request(&format!("{base}/media"), "GET", b"", &[], true, None);
        assert_eq!(
            media.err().unwrap(),
            "Direct-media downloads do not follow redirects"
        );
        let plain = network
            .json_response(&format!("{base}/x"), "GET", "", &[], false)
            .unwrap();
        assert_eq!(plain["status"], 302);
        assert_eq!(plain["body"], "plain");
    }

    #[test]
    fn cookies_come_and_go_with_requests() {
        let server = Server::start(|seen| match seen.path() {
            "/login" => Reply::new(200, "")
                .with("Set-Cookie", "sid=abc; Path=/")
                .with("Set-Cookie", "lang=en; Path=/"),
            _ => Reply::new(200, seen.header("cookie").unwrap_or("").to_string()),
        });
        let (network, kept) = network(json!({}));
        let base = server.base();
        network
            .json_response(&format!("{base}/login"), "GET", "", &[], false)
            .unwrap();
        let echoed = network
            .json_response(&format!("{base}/me"), "GET", "", &[], false)
            .unwrap();
        assert_eq!(echoed["body"], "sid=abc; lang=en");
        assert_eq!(
            kept.lock().unwrap().last().unwrap()["cookies"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // The add-on's own Cookie wins over the jar's.
        let own = network
            .json_response(
                &format!("{base}/me"),
                "GET",
                "",
                &headers(&[("cookie", "mine=1")]),
                false,
            )
            .unwrap();
        assert_eq!(own["body"], "mine=1");
        // Direct media sends none.
        let mut media = network
            .request(&format!("{base}/me"), "GET", b"", &[], true, None)
            .unwrap();
        let mut text = String::new();
        media.body.read_to_string(&mut text).unwrap();
        assert_eq!(text, "");
        network.clear_cookies();
        let cleared = network
            .json_response(&format!("{base}/me"), "GET", "", &[], false)
            .unwrap();
        assert_eq!(cleared["body"], "");
        assert_eq!(kept.lock().unwrap().last().unwrap()["cookies"], json!([]));
    }

    #[test]
    fn answers_past_16_mib_fail() {
        let server = Server::start(|_| Reply::new(200, vec![b'a'; MAX_RESPONSE + 1]));
        let (network, _) = network(json!({}));
        let result = network.json_response(&server.base(), "GET", "", &[], false);
        assert_eq!(result.unwrap_err(), "Addon network response exceeds 16 MB");
    }

    #[test]
    fn the_connection_record_is_restored_and_kept_without_pending() {
        let saved = json!({
            "auth": {"access_token": "t", "pending": {"url": "https://example.com"}},
            "cookies": [{"name": "n", "value": "v", "domain": "127.0.0.1", "hostOnly": true, "path": "/", "secure": false}],
        });
        let (network, kept) = network(saved);
        assert_eq!(network.auth()["access_token"], "t");
        network.keep();
        let record = kept.lock().unwrap().last().cloned().unwrap();
        assert_eq!(record["auth"], json!({"access_token": "t"}));
        assert_eq!(record["cookies"][0]["name"], "n");
        let before = network.generation();
        network.reset();
        assert_eq!(network.generation(), before + 1);
        assert!(network.auth().is_empty());
        assert_eq!(
            kept.lock().unwrap().last().cloned().unwrap(),
            json!({"auth": {}, "cookies": []})
        );
    }
}
