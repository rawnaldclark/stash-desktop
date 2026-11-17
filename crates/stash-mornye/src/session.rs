//! Signed sessions (spec §6; Lokal `morneye_session.dart`): the gateway a manifest names (`signedSession.baseUrl`)
//! first asks a person to pass a verification page once; the grant that page returns to the callback is exchanged
//! for a session, and every request after is HMAC-signed with it. No attestation and no secret of an app's: a random
//! install id, values from the manifest, the person's grant, and what the gateway issued.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Map, Value, json};
use url::Url;

use crate::net::{Network, days_from_civil, parse_http_date, text_of, validate_url};
use crate::store::Store;

/// The record holding the install id and the session.
const SESSION: &str = "session";
/// The record holding the gateway's cooldown (`{until: ms}`).
const RATE_LIMIT: &str = "ratelimit";
/// How long a verification page may take before its callback is refused.
const CALLBACK_MAX_AGE_MS: i64 = 3 * 60 * 1000;
/// A session this close to its end is refreshed first, when the gateway has a refresh endpoint.
const REFRESH_WITHIN_MS: i64 = 60 * 60 * 1000;
/// The cooldown when a 429 names none.
const DEFAULT_COOLDOWN_SECONDS: f64 = 60.0;
/// What a usable session holds.
const SESSION_FIELDS: [&str; 3] = ["session_id", "session_secret", "expires_at"];

/// A manifest's `signedSession` (sess.dart:41-59).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub base_url: String,
    pub bootstrap: String,
    pub challenge: String,
    pub exchange: String,
    pub refresh: Option<String>,
    pub header_prefix: String,
    pub scheme_label: String,
    pub app_version: String,
    pub platform: String,
    pub time_window_seconds: u64,
    pub callback_url: String,
    /// Shared by add-ons that name the same one; else the add-on's key.
    pub namespace: Option<String>,
}

impl SessionConfig {
    /// The config a manifest's `signedSession` value gives, with the defaults; `None` when it has no `baseUrl`.
    pub fn of(value: &Value) -> Option<SessionConfig> {
        let base_url = value.get("baseUrl")?.as_str()?.to_string();
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        let endpoints = value.get("endpoints");
        let endpoint = |key: &str| {
            endpoints
                .and_then(|endpoints| endpoints.get(key))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let time_window_seconds = value
            .get("timeWindowSeconds")
            .and_then(Value::as_f64)
            .map(f64::trunc)
            .filter(|seconds| *seconds >= 1.0)
            .map_or(300, |seconds| seconds as u64);
        Some(SessionConfig {
            base_url,
            bootstrap: endpoint("bootstrap").unwrap_or_else(|| "/bootstrap".into()),
            challenge: endpoint("challenge").unwrap_or_else(|| "/challenge".into()),
            exchange: endpoint("exchange").unwrap_or_else(|| "/session/exchange".into()),
            refresh: endpoint("refresh"),
            header_prefix: text("headerPrefix").unwrap_or_else(|| "X-Sig-".into()),
            scheme_label: text("schemeLabel").unwrap_or_else(|| "SPOTIFLAC-HMAC-V1".into()),
            app_version: text("appVersion").unwrap_or_else(|| "ext-1.0".into()),
            platform: text("platform").unwrap_or_else(|| "extension".into()),
            time_window_seconds,
            callback_url: text("callbackUrl").unwrap_or_else(|| "spotiflac://session-grant".into()),
            namespace: text("namespace"),
        })
    }
}

/// The signed headers for one request (sess.dart:31-79), with `now_ms` and `nonce` given so the test vector can be
/// checked.
#[allow(clippy::too_many_arguments)]
pub fn signed_headers(
    config: &SessionConfig,
    session_id: &str,
    session_secret: &str,
    method: &str,
    url_path: &str,
    body: &str,
    now_ms: i64,
    nonce: &str,
) -> Vec<(String, String)> {
    let timestamp = iso_timestamp(now_ms);
    let body_hash = crate::hex(&crate::crypto::sha256(body.as_bytes()));
    let window = now_ms
        .div_euclid(1000)
        .div_euclid(config.time_window_seconds.max(1) as i64);
    // The key rolls with the time window: HMAC(secret, "<window>:<session id>").
    let rolling = crate::bytes::base64url_nopad(&crate::crypto::hmac_sha256(
        session_secret.as_bytes(),
        format!("{window}:{session_id}").as_bytes(),
    ));
    let method = method.to_uppercase();
    // The query is not signed: its line stays empty.
    let lines: [&str; 10] = [
        &config.scheme_label,
        &method,
        url_path,
        "",
        &body_hash,
        &timestamp,
        nonce,
        session_id,
        &config.app_version,
        &config.platform,
    ];
    let signature = crate::bytes::base64url_nopad(&crate::crypto::hmac_sha256(
        rolling.as_bytes(),
        lines.join("\n").as_bytes(),
    ));
    let prefix = &config.header_prefix;
    vec![
        (format!("{prefix}Session"), session_id.to_string()),
        (format!("{prefix}Timestamp"), timestamp),
        (format!("{prefix}Nonce"), nonce.to_string()),
        (format!("{prefix}Body-SHA256"), body_hash),
        (format!("{prefix}Signature"), signature),
        (format!("{prefix}App-Version"), config.app_version.clone()),
        (format!("{prefix}Platform"), config.platform.clone()),
    ]
}

/// A verification page waiting to come back.
#[derive(Debug, Clone)]
struct Pending {
    url: String,
    state: String,
    created_ms: i64,
}

/// What lives in memory only.
#[derive(Debug, Default)]
struct State {
    pending: Option<Pending>,
    /// The grant the callback brought, until it is exchanged.
    grant: Option<String>,
    /// The gateway asked for verification again (428).
    blocked: bool,
    /// Bumped by [`Session::clear`]: a flight that saw another one does not write back.
    generation: u64,
}

/// One piece of work at a time; whoever came while one was running gets its result instead of running it again.
struct Flight<T> {
    last: Mutex<Option<Result<T, String>>>,
    finished: AtomicU64,
}

impl<T: Clone> Flight<T> {
    fn new() -> Flight<T> {
        Flight {
            last: Mutex::new(None),
            finished: AtomicU64::new(0),
        }
    }

    fn run(&self, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let seen = self.finished.load(Ordering::SeqCst);
        let mut last = lock(&self.last);
        if self.finished.load(Ordering::SeqCst) != seen
            && let Some(result) = &*last
        {
            return result.clone();
        }
        let result = work();
        *last = Some(result.clone());
        self.finished.fetch_add(1, Ordering::SeqCst);
        result
    }
}

/// Why a signed request stopped: the cooldown (thrown to the add-on) or anything else (answered as `{ok: false}`).
enum Fail {
    RateLimited(String),
    Other(String),
}

impl From<String> for Fail {
    fn from(message: String) -> Fail {
        Fail::Other(message)
    }
}

/// One add-on's (or one shared namespace's) signed session.
pub struct Session {
    config: SessionConfig,
    /// The store namespace: sha256hex of namespace (or add-on key), base URL, app version and platform.
    namespace: String,
    network: Arc<Network>,
    store: Arc<Store>,
    state: Mutex<State>,
    /// Read-change-write of the stored records, one at a time.
    records: Mutex<()>,
    bootstrapping: Flight<bool>,
    exchanging: Flight<Value>,
    refreshing: Flight<()>,
}

impl Session {
    pub fn new(
        config: SessionConfig,
        addon_key: &str,
        network: Arc<Network>,
        store: Arc<Store>,
    ) -> Session {
        let owner = config.namespace.as_deref().unwrap_or(addon_key);
        let parts: [&str; 4] = [
            owner,
            &config.base_url,
            &config.app_version,
            &config.platform,
        ];
        let namespace = crate::hex(&crate::crypto::sha256(parts.join("\n").as_bytes()));
        Session {
            config,
            namespace,
            network,
            store,
            state: Mutex::new(State::default()),
            records: Mutex::new(()),
            bootstrapping: Flight::new(),
            exchanging: Flight::new(),
            refreshing: Flight::new(),
        }
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// `{authenticated, verification_required, expires_at, session_id, install_id, app_version, platform, auth_url}`.
    pub fn status(&self) -> Value {
        let record = self.record();
        let state = self.state();
        json!({
            "authenticated": usable(&record) && !state.blocked,
            "verification_required": state.pending.is_some(),
            "expires_at": field_or_empty(&record, "expires_at"),
            "session_id": field_or_empty(&record, "session_id"),
            "install_id": record.get("install_id").cloned().unwrap_or(Value::Null),
            "app_version": self.config.app_version,
            "platform": self.config.platform,
            "auth_url": state.pending.as_ref().map(|pending| pending.url.clone()).unwrap_or_default(),
        })
    }

    /// Forgets the session (keeps the install id): `{success: true}`.
    pub fn clear(&self) -> Value {
        let record = self.record();
        {
            let mut state = self.state();
            state.generation += 1;
            state.pending = None;
            state.blocked = false;
        }
        let mut kept = Map::new();
        if let Some(install_id) = record.get("install_id") {
            kept.insert("install_id".into(), install_id.clone());
        }
        let _ = self.save(&kept);
        json!({ "success": true })
    }

    /// Asks the gateway for a session; false when a person must pass the verification page first (its address is
    /// then [`Session::pending_url`]).
    pub fn bootstrap(&self) -> Result<bool, String> {
        self.bootstrapping.run(|| self.run_bootstrap())
    }

    /// The verification page to open, while one is pending.
    pub fn pending_url(&self) -> Option<String> {
        self.state()
            .pending
            .as_ref()
            .map(|pending| pending.url.clone())
    }

    /// Whether the page the verification window is about to open is the callback (to be stopped and handed to
    /// [`Session::callback`]): the callback itself, or a wrapper page whose `cb` is one and carries a grant.
    pub fn is_callback(&self, url: &str) -> bool {
        self.callback_target(url).is_some()
    }

    /// The verification page came back to the callback: its grant is exchanged for a session (`Invalid or expired
    /// verification callback`, `The verification page returned no grant. Please try verification again.`).
    pub fn callback(&self, url: &str) -> Result<Value, String> {
        const INVALID: &str = "Invalid or expired verification callback";
        let target = self.callback_target(url).ok_or(INVALID)?;
        let returned_state = param(&target, "state");
        {
            let state = self.state();
            let pending = state.pending.as_ref().ok_or(INVALID)?;
            if crate::now_ms() - pending.created_ms > CALLBACK_MAX_AGE_MS
                || returned_state.as_deref() != Some(pending.state.as_str())
            {
                return Err(INVALID.to_string());
            }
        }
        let grant = returned_grant(&target)
            .ok_or("The verification page returned no grant. Please try verification again.")?;
        self.state().grant = Some(grant);
        self.complete_grant(None)
    }

    /// Exchanges `grant` (else the one pending) for a session: `{success: true}`.
    pub fn complete_grant(&self, grant: Option<String>) -> Result<Value, String> {
        let grant = match grant {
            Some(grant) => grant,
            None => self
                .state()
                .grant
                .clone()
                .ok_or("No pending verification grant")?,
        };
        self.exchanging.run(|| self.run_exchange(&grant))
    }

    /// `session.signedFetch` (sess.dart:578-660). `sleep` waits (and fails when the call was cancelled).
    pub fn signed_fetch(
        &self,
        method: &str,
        path: &str,
        body: &str,
        headers: &[(String, String)],
        sleep: &dyn Fn(Duration) -> Result<(), String>,
    ) -> Result<Value, String> {
        match self.fetch(method, path, body, headers, sleep) {
            Ok(value) => Ok(value),
            Err(Fail::RateLimited(message)) => Err(message),
            Err(Fail::Other(message)) => Ok(json!({ "ok": false, "error": message })),
        }
    }

    /// Fails with `The add-on service is rate limiting requests (HTTP 429). Try again in N seconds.` while the
    /// gateway's cooldown lasts.
    pub fn check_rate(&self) -> Result<(), String> {
        match self.rate_limited_for() {
            Some(seconds) => Err(rate_limit_message(seconds)),
            None => Ok(()),
        }
    }

    /// The seconds left of the gateway's cooldown, if one lasts.
    pub fn rate_limited_for(&self) -> Option<u64> {
        let until = self
            .store
            .read(&self.namespace, RATE_LIMIT)
            .get("until")?
            .as_i64()?;
        let remaining = until - crate::now_ms();
        (remaining > 0).then(|| ((remaining + 999) / 1000) as u64)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// The stored record; an install id is made the first time.
    fn record(&self) -> Map<String, Value> {
        let _records = lock(&self.records);
        let mut record = self.store.read(&self.namespace, SESSION);
        if record.get("install_id").is_none_or(Value::is_null) {
            record.insert(
                "install_id".into(),
                Value::String(crate::hex(&crate::random_bytes(16))),
            );
            let _ = self.store.write(&self.namespace, SESSION, &record);
        }
        record
    }

    fn save(&self, record: &Map<String, Value>) -> Result<(), String> {
        let _records = lock(&self.records);
        self.store.write(&self.namespace, SESSION, record)
    }

    /// `path` under the base URL, never outside it (sess.dart:272-281).
    fn endpoint(&self, path: &str) -> Result<Url, String> {
        const ESCAPED: &str = "Signed-session path escaped its base URL";
        let base = Url::parse(&self.config.base_url).map_err(|_| "Invalid URL".to_string())?;
        let text = base.as_str();
        let trimmed = text.strip_suffix('/').unwrap_or(text);
        let url = Url::parse(&format!("{trimmed}/{}", path.trim_start_matches('/')))
            .map_err(|_| ESCAPED.to_string())?;
        let base_path = base.path().strip_suffix('/').unwrap_or(base.path());
        if url.origin() != base.origin() || !url.path().starts_with(&format!("{base_path}/")) {
            return Err(ESCAPED.to_string());
        }
        Ok(url)
    }

    /// sess.dart:319-395.
    fn run_bootstrap(&self) -> Result<bool, String> {
        let generation = self.state().generation;
        let record = self.record();
        let mut url = self.endpoint(&self.config.bootstrap)?;
        let install_id = text_field(&record, "install_id");
        set_query(
            &mut url,
            &[
                ("app_version", self.config.app_version.as_str()),
                ("install_id", install_id.as_str()),
            ],
        );
        let response = self
            .network
            .json_response(url.as_str(), "GET", "", &[], false)?;
        if response.get("ok") != Some(&Value::Bool(true)) {
            return Err(format!(
                "Signed-session bootstrap returned HTTP {}",
                text_field_of(&response, "status")
            ));
        }
        let body =
            body_object(&response).ok_or("Signed-session bootstrap returned an invalid answer")?;
        if self.state().generation != generation {
            return Err("Session changed during authentication".to_string());
        }
        if SESSION_FIELDS.iter().all(|key| present(&body, key)) {
            let mut saved = record;
            saved.extend(body);
            self.save(&saved)?;
            let mut state = self.state();
            state.pending = None;
            state.blocked = false;
            return Ok(true);
        }
        let mut state_token = random_token();
        let mut auth_url = ["auth_url", "challenge_url"]
            .iter()
            .find_map(|key| body.get(*key).filter(|value| !value.is_null()))
            .and_then(Value::as_str)
            .map(str::to_string);
        if auth_url.is_none()
            && let Some(challenge_id) = body.get("challenge_id").filter(|value| !value.is_null())
        {
            // A v2 grant challenge: the page hands a grant back to our callback, with our state.
            let mut callback =
                Url::parse(&self.config.callback_url).map_err(|_| "Invalid URL".to_string())?;
            set_query(
                &mut callback,
                &[("cb_version", "v2grant"), ("state", state_token.as_str())],
            );
            let mut challenge = self.endpoint(&self.config.challenge)?;
            let id = text_of(challenge_id);
            set_query(
                &mut challenge,
                &[("id", id.as_str()), ("cb", callback.as_str())],
            );
            auth_url = Some(challenge.to_string());
        }
        let auth_url = auth_url.ok_or("Signed-session bootstrap gave no verification page")?;
        let mut page = validate_url(&auth_url, self.network.permissions())?;
        let own_state = param(&page, "state");
        let nested_state = ["cb", "callback_url", "redirect_uri"]
            .iter()
            .find_map(|key| param(&page, key))
            .and_then(|nested| query_value(&nested, "state"));
        if own_state.is_none() && nested_state.is_none() {
            page.query_pairs_mut().append_pair("state", &state_token);
        } else if let Some(found) = own_state.or(nested_state) {
            state_token = found;
        }
        self.state().pending = Some(Pending {
            url: page.to_string(),
            state: state_token,
            created_ms: crate::now_ms(),
        });
        Ok(false)
    }

    /// sess.dart:407-445.
    fn run_exchange(&self, grant: &str) -> Result<Value, String> {
        let generation = self.state().generation;
        let record = self.record();
        let url = self.endpoint(&self.config.exchange)?;
        let body = ordered_json(&[
            ("grant", Value::String(grant.to_string())),
            (
                "install_id",
                record.get("install_id").cloned().unwrap_or(Value::Null),
            ),
            (
                "app_version",
                Value::String(self.config.app_version.clone()),
            ),
            ("platform", Value::String(self.config.platform.clone())),
        ]);
        let headers = [("Content-Type".to_string(), "application/json".to_string())];
        let response = self
            .network
            .json_response(url.as_str(), "POST", &body, &headers, false)?;
        if response.get("ok") != Some(&Value::Bool(true)) {
            return Err(format!(
                "Session exchange returned HTTP {}",
                text_field_of(&response, "status")
            ));
        }
        let issued = body_object(&response).ok_or("Session exchange returned an invalid answer")?;
        if !SESSION_FIELDS.iter().all(|key| present(&issued, key)) {
            return Err("Session exchange omitted its session fields".to_string());
        }
        if self.state().generation != generation {
            return Err("Session changed during authentication".to_string());
        }
        let mut saved = record;
        for key in SESSION_FIELDS {
            if let Some(value) = issued.get(key) {
                saved.insert(key.into(), value.clone());
            }
        }
        self.save(&saved)?;
        let mut state = self.state();
        state.pending = None;
        state.grant = None;
        state.blocked = false;
        Ok(json!({ "success": true }))
    }

    /// `{ok: false, needsVerification: true, error: 'VERIFY_REQUIRED', auth_url, open_auth_url}`.
    fn verification(&self) -> Value {
        let url = self.pending_url().unwrap_or_default();
        json!({
            "ok": false,
            "needsVerification": true,
            "error": "VERIFY_REQUIRED",
            "auth_url": url,
            "open_auth_url": url,
        })
    }

    fn rate_check(&self) -> Result<(), Fail> {
        self.check_rate().map_err(Fail::RateLimited)
    }

    /// sess.dart:584-660.
    fn fetch(
        &self,
        method: &str,
        path: &str,
        body: &str,
        headers: &[(String, String)],
        sleep: &dyn Fn(Duration) -> Result<(), String>,
    ) -> Result<Value, Fail> {
        self.rate_check()?;
        let mut record = self.record();
        let blocked = self.state().blocked;
        if !usable(&record) || blocked {
            if !self.bootstrap()? {
                return Ok(self.verification());
            }
            record = self.record();
        }
        self.refresh(&record);
        record = self.record();
        for attempt in 0..3 {
            let response = self.request(&record, method, path, body, headers)?;
            let contract = body_object(&response).unwrap_or_default();
            let code = contract_code(&contract);
            let status = response.get("status").and_then(Value::as_i64).unwrap_or(0);
            let origin = contract.get("origin").and_then(Value::as_str);
            let action = contract.get("action").and_then(Value::as_str);
            let retryable = contract.get("retryable") == Some(&Value::Bool(true));
            if status == 503
                && origin == Some("provider")
                && code == "PROVIDER_UNAVAILABLE"
                && retryable
                && contract.get("retry_mode").and_then(Value::as_str) == Some("same_operation")
                && attempt < 2
            {
                let seconds = contract
                    .get("retry_after_seconds")
                    .and_then(Value::as_f64)
                    .map_or(1.0, f64::trunc)
                    .clamp(1.0, 120.0);
                sleep(Duration::from_secs(seconds as u64))?;
                continue;
            }
            let reset = status == 401
                && origin == Some("gateway")
                && code == "SESSION_INVALID"
                && action == Some("bootstrap_session");
            let verify = status == 428
                && origin == Some("gateway")
                && code == "VERIFY_REQUIRED"
                && action == Some("verify");
            if (reset || verify) && attempt < 1 {
                if reset {
                    self.clear();
                } else {
                    self.state().blocked = true;
                }
                if !self.bootstrap()? {
                    return Ok(self.verification());
                }
                record = self.record();
                continue;
            }
            return Ok(signed_addon_response(&response, &contract));
        }
        Ok(json!({ "ok": false, "error": "Signed request failed" }))
    }

    /// One signed request (sess.dart:551-575); a 429 starts the cooldown.
    fn request(
        &self,
        record: &Map<String, Value>,
        method: &str,
        path: &str,
        body: &str,
        headers: &[(String, String)],
    ) -> Result<Value, Fail> {
        self.rate_check()?;
        let url = self.endpoint(path)?;
        let nonce = crate::hex(&crate::random_bytes(16));
        let signed = signed_headers(
            &self.config,
            &text_field(record, "session_id"),
            &text_field(record, "session_secret"),
            method,
            url.path(),
            body,
            crate::now_ms(),
            &nonce,
        );
        // Later headers win: the add-on's over the content type, the signature's over the add-on's.
        let mut sent = vec![("Content-Type".to_string(), "application/json".to_string())];
        sent.extend(headers.iter().cloned());
        sent.extend(signed);
        let response =
            self.network
                .json_response(url.as_str(), &method.to_uppercase(), body, &sent, false)?;
        if response.get("status").and_then(Value::as_i64) == Some(429) {
            return Err(Fail::RateLimited(self.remember_rate_limit(&response)));
        }
        Ok(response)
    }

    /// Refreshes a session close to its end (best effort, sess.dart:516-549).
    fn refresh(&self, record: &Map<String, Value>) {
        let Some(path) = self.config.refresh.as_deref() else {
            return;
        };
        let Some(expires) = record.get("expires_at").and_then(expiry_ms) else {
            return;
        };
        if expires - crate::now_ms() > REFRESH_WITHIN_MS {
            return;
        }
        let _ = self.refreshing.run(|| {
            self.run_refresh(record, path);
            Ok(())
        });
    }

    fn run_refresh(&self, record: &Map<String, Value>, path: &str) {
        let generation = self.state().generation;
        let body = ordered_json(&[(
            "install_id",
            record.get("install_id").cloned().unwrap_or(Value::Null),
        )]);
        let Ok(response) = self.request(record, "POST", path, &body, &[]) else {
            return;
        };
        if response.get("ok") != Some(&Value::Bool(true)) {
            return;
        }
        let Some(next) = body_object(&response) else {
            return;
        };
        if generation != self.state().generation
            || self.record().get("session_id") != record.get("session_id")
        {
            return;
        }
        let mut saved = record.clone();
        for key in SESSION_FIELDS {
            if let Some(value) = next.get(key).filter(|value| !value.is_null()) {
                saved.insert(key.into(), value.clone());
            }
        }
        let _ = self.save(&saved);
    }

    /// Keeps the cooldown a 429 asked for (sess.dart:185-220): the longer of `Retry-After` (seconds or an HTTP
    /// date) and the body's `retry_after_seconds`, else 60 s; never shortened. The error to throw.
    fn remember_rate_limit(&self, response: &Value) -> String {
        let now = crate::now_ms();
        let contract = body_object(response).unwrap_or_default();
        let header = response
            .get("headers")
            .and_then(Value::as_object)
            .and_then(|headers| {
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
                    .map(|(_, value)| text_of(value))
            });
        let header_seconds = header.as_deref().and_then(|raw| {
            raw.trim()
                .parse::<f64>()
                .ok()
                .or_else(|| parse_http_date(raw).map(|at| (at - now) as f64 / 1000.0))
        });
        let body_seconds = contract
            .get("retry_after_seconds")
            .and_then(|value| match value {
                Value::Number(number) => number.as_f64(),
                Value::String(text) => text.trim().parse::<f64>().ok(),
                _ => None,
            });
        let seconds = [header_seconds, body_seconds]
            .into_iter()
            .flatten()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .fold(None, |longest: Option<f64>, seconds| {
                Some(longest.map_or(seconds, |longest| longest.max(seconds)))
            })
            .unwrap_or(DEFAULT_COOLDOWN_SECONDS)
            .ceil();
        // Kept within range: a gateway's absurd delay must not overflow the clock.
        let until = now.saturating_add((seconds.min(1e12) as i64).saturating_mul(1000));
        let _records = lock(&self.records);
        let previous = self
            .store
            .read(&self.namespace, RATE_LIMIT)
            .get("until")
            .and_then(Value::as_i64);
        let until = previous
            .filter(|previous| *previous > until)
            .unwrap_or(until);
        let mut kept = Map::new();
        kept.insert("until".into(), Value::from(until));
        let _ = self.store.write(&self.namespace, RATE_LIMIT, &kept);
        rate_limit_message((until - now + 999).max(1000) as u64 / 1000)
    }

    /// The callback a page is about to open (sess.dart:493-511): the callback itself, or a wrapper whose `cb` is the
    /// callback carrying an issued grant (the challenge page's own `cb` template carries none and must open).
    fn callback_target(&self, raw: &str) -> Option<Url> {
        let expected = Url::parse(&self.config.callback_url).ok()?;
        let matches = |url: &Url| {
            url.scheme() == expected.scheme()
                && url.authority() == expected.authority()
                && url.path() == expected.path()
        };
        let url = Url::parse(raw).ok()?;
        if matches(&url) {
            return Some(url);
        }
        let target = Url::parse(&param(&url, "cb")?).ok()?;
        (matches(&target) && returned_grant(&target).is_some()).then_some(target)
    }
}

/// The text of the error [`Session::check_rate`] fails with, for `seconds`.
pub fn rate_limit_message(seconds: u64) -> String {
    format!(
        "The add-on service is rate limiting requests (HTTP 429). Try again in {seconds} seconds."
    )
}

/// The gateway's answer as a Mornye add-on reads it (sess.dart:95-131): the `http` answer plus `code`, `retryable`,
/// `retryMode`, `retryAfterSeconds`, `needsVerification`, and the snake_case `error_*` fields when there is a code.
pub(crate) fn signed_addon_response(response: &Value, contract: &Map<String, Value>) -> Value {
    let status = response.get("status").and_then(Value::as_i64);
    let code = contract_code(contract);
    let explicit = contract
        .get("retry_mode")
        .filter(|value| !value.is_null())
        .map(text_of)
        .unwrap_or_default()
        .trim()
        .to_string();
    // A 4xx other than 408 and 429 does not get better by asking again, unless the gateway says otherwise.
    let terminal =
        status.is_some_and(|status| (400..500).contains(&status) && status != 408 && status != 429);
    let retry_mode = if !explicit.is_empty() {
        explicit
    } else if terminal {
        "none".to_string()
    } else {
        String::new()
    };
    let retry_seconds = contract
        .get("retry_after_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let retryable = contract.get("retryable") == Some(&Value::Bool(true)) && retry_mode != "none";
    let mut out = response.as_object().cloned().unwrap_or_default();
    out.insert("code".into(), Value::String(code.clone()));
    out.insert("retryable".into(), Value::Bool(retryable));
    out.insert("retryMode".into(), Value::String(retry_mode));
    out.insert("retryAfterSeconds".into(), Value::from(retry_seconds));
    out.insert(
        "needsVerification".into(),
        Value::Bool(status == Some(428) || code == "VERIFY_REQUIRED"),
    );
    if !code.is_empty() {
        out.insert("error_code".into(), Value::String(code));
        out.insert(
            "error_origin".into(),
            contract.get("origin").cloned().unwrap_or(Value::Null),
        );
        out.insert(
            "error_action".into(),
            contract.get("action").cloned().unwrap_or(Value::Null),
        );
        out.insert("retry_after_seconds".into(), Value::from(retry_seconds));
    }
    Value::Object(out)
}

/// The contract's `code`, uppercased (`''` when there is none).
fn contract_code(contract: &Map<String, Value>) -> String {
    contract
        .get("code")
        .filter(|value| !value.is_null())
        .map(text_of)
        .unwrap_or_default()
        .to_uppercase()
}

/// A session that can sign: an id, a secret, and an end still to come (sess.dart:263-269).
fn usable(record: &Map<String, Value>) -> bool {
    present(record, "session_id")
        && present(record, "session_secret")
        && record
            .get("expires_at")
            .and_then(expiry_ms)
            .is_some_and(|expires| expires > crate::now_ms())
}

fn present(record: &Map<String, Value>, key: &str) -> bool {
    record.get(key).is_some_and(|value| !value.is_null())
}

/// `'${record[key]}'` (`null` when it is missing).
fn text_field(record: &Map<String, Value>, key: &str) -> String {
    text_of(record.get(key).unwrap_or(&Value::Null))
}

fn text_field_of(value: &Value, key: &str) -> String {
    text_of(value.get(key).unwrap_or(&Value::Null))
}

/// `record[key] ?? ''`.
fn field_or_empty(record: &Map<String, Value>, key: &str) -> Value {
    record
        .get(key)
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()))
}

/// When a session ends, in ms: an ISO-8601 string; a number is read as seconds (or ms when it is that large).
fn expiry_ms(value: &Value) -> Option<i64> {
    match value {
        Value::String(text) => parse_iso_ms(text),
        Value::Number(number) => number.as_f64().filter(|n| n.is_finite()).map(|n| {
            if n.abs() < 1e11 {
                (n * 1000.0) as i64
            } else {
                n as i64
            }
        }),
        _ => None,
    }
}

/// An `http` answer's body as a JSON object.
fn body_object(response: &Value) -> Option<Map<String, Value>> {
    match serde_json::from_str::<Value>(response.get("body")?.as_str()?) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// A JSON object with its keys in the order given.
fn ordered_json(pairs: &[(&str, Value)]) -> String {
    let fields: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{}:{}", Value::from(*key), value))
        .collect();
    format!("{{{}}}", fields.join(","))
}

/// The issued grant a callback carries: `grant`, else `code` (legacy), not blank.
fn returned_grant(url: &Url) -> Option<String> {
    ["grant", "code"]
        .iter()
        .find_map(|key| param(url, key).filter(|value| !value.trim().is_empty()))
}

/// The query parameter `name` of `url` (the last when repeated).
fn param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .filter(|(key, _)| key == name)
        .last()
        .map(|(_, value)| value.into_owned())
}

/// The query parameter `name` of any URL-like text (relative ones too).
fn query_value(raw: &str, name: &str) -> Option<String> {
    let before_fragment = raw.split('#').next().unwrap_or_default();
    let (_, query) = before_fragment.split_once('?')?;
    url::form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key == name)
        .last()
        .map(|(_, value)| value.into_owned())
}

/// Sets query parameters, replacing any of the same name and keeping the rest.
fn set_query(url: &mut Url, set: &[(&str, &str)]) {
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !set.iter().any(|(name, _)| key == name))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let mut pairs = url.query_pairs_mut();
    pairs.clear();
    for (key, value) in &kept {
        pairs.append_pair(key, value);
    }
    for (key, value) in set {
        pairs.append_pair(key, value);
    }
}

/// base64url (no padding) of 24 random bytes.
fn random_token() -> String {
    crate::bytes::base64url_nopad(&crate::random_bytes(24))
}

/// UTC ISO-8601 with exactly three fractional digits and `Z`, as JavaScript's `toISOString`.
pub(crate) fn iso_timestamp(ms: i64) -> String {
    let (year, month, day) = civil_from_days(ms.div_euclid(86_400_000));
    let rest = ms.rem_euclid(86_400_000);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
}

/// An ISO-8601 date-time as Dart's `DateTime.tryParse` reads it (`YYYY-MM-DD`, then `T` or a space and
/// `hh[:mm[:ss[.fff]]]`, then `Z` or `±hh[:mm]`; separators optional), in ms. Without a zone it is read as UTC.
pub(crate) fn parse_iso_ms(text: &str) -> Option<i64> {
    let s = text.trim().as_bytes();
    let mut i = 0;
    let year = digits(s, &mut i, 4)?;
    skip(s, &mut i, b'-');
    let month = digits(s, &mut i, 2)?;
    skip(s, &mut i, b'-');
    let day = digits(s, &mut i, 2)?;
    let mut clock_ms = 0;
    let mut offset_ms = 0;
    if i < s.len() {
        if !matches!(s[i], b'T' | b't' | b' ') {
            return None;
        }
        i += 1;
        let hour = digits(s, &mut i, 2)?;
        let (mut minute, mut second, mut millis) = (0, 0, 0);
        if let Some(found) = clock_field(s, &mut i) {
            minute = found;
            if let Some(found) = clock_field(s, &mut i) {
                second = found;
                if i < s.len() && matches!(s[i], b'.' | b',') {
                    i += 1;
                    let start = i;
                    while i < s.len() && s[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == start {
                        return None;
                    }
                    millis = (0..3).fold(0, |value, k| {
                        value * 10
                            + s.get(start + k)
                                .filter(|_| start + k < i)
                                .map_or(0, |d| i64::from(d - b'0'))
                    });
                }
            }
        }
        if hour > 24 || minute > 59 || second > 59 {
            return None;
        }
        clock_ms = ((hour * 60 + minute) * 60 + second) * 1000 + millis;
        if i < s.len() && s[i] == b' ' {
            i += 1;
        }
        if i < s.len() {
            match s[i] {
                b'Z' | b'z' => i += 1,
                sign @ (b'+' | b'-') => {
                    i += 1;
                    let hours = digits(s, &mut i, 2)?;
                    let minutes = clock_field(s, &mut i).unwrap_or(0);
                    let offset = (hours * 60 + minutes) * 60_000;
                    offset_ms = if sign == b'-' { -offset } else { offset };
                }
                _ => return None,
            }
        }
        if i != s.len() {
            return None;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month as u32, day as u32) * 86_400_000 + clock_ms - offset_ms)
}

/// Exactly `n` digits at `i`.
fn digits(s: &[u8], i: &mut usize, n: usize) -> Option<i64> {
    let field = s.get(*i..*i + n)?;
    if !field.iter().all(u8::is_ascii_digit) {
        return None;
    }
    *i += n;
    Some(
        field
            .iter()
            .fold(0, |value, d| value * 10 + i64::from(d - b'0')),
    )
}

/// An optional `:` and two digits; nothing taken when they are not there.
fn clock_field(s: &[u8], i: &mut usize) -> Option<i64> {
    let start = *i;
    skip(s, i, b':');
    let found = digits(s, i, 2);
    if found.is_none() {
        *i = start;
    }
    found
}

fn skip(s: &[u8], i: &mut usize, byte: u8) {
    if s.get(*i) == Some(&byte) {
        *i += 1;
    }
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted + 2) / 5 + 1) as u32;
    let month = (if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    }) as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::net::testing::{Reply, Seen, Server, network};

    const DAY_MS: i64 = 86_400_000;

    #[test]
    fn signed_headers_match_the_reference_vector() {
        // Made-up fixture values; the expected strings come from the reference implementation.
        let config = SessionConfig::of(&json!({
            "baseUrl": "https://gateway.example.com/v2",
            "headerPrefix": "X-Zarz-",
            "schemeLabel": "ZARZ-HMAC-V1",
            "appVersion": "tidal-web@1.2.5",
            "platform": "extension",
            "timeWindowSeconds": 300,
        }))
        .unwrap();
        let headers = signed_headers(
            &config,
            "sid1",
            "sec",
            "post",
            "/v2/tickets",
            "{\"a\":\"é\"}",
            1_760_937_314_949,
            "abcd",
        );
        let names: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "X-Zarz-Session",
                "X-Zarz-Timestamp",
                "X-Zarz-Nonce",
                "X-Zarz-Body-SHA256",
                "X-Zarz-Signature",
                "X-Zarz-App-Version",
                "X-Zarz-Platform"
            ]
        );
        let value = |name: &str| headers.iter().find(|(n, _)| n == name).unwrap().1.clone();
        assert_eq!(value("X-Zarz-Session"), "sid1");
        assert_eq!(value("X-Zarz-Timestamp"), "2025-10-20T05:15:14.949Z");
        assert_eq!(value("X-Zarz-Nonce"), "abcd");
        assert_eq!(
            value("X-Zarz-Body-SHA256"),
            "b3a092a6af48807fa9482b2ee140105575daa26d5b24b3c0e60a7e2dee6683b1"
        );
        assert_eq!(
            value("X-Zarz-Signature"),
            "d_UznKcHXh1P8UujgutVoKaevKApl08RhCSwPeee7fE"
        );
        assert_eq!(value("X-Zarz-App-Version"), "tidal-web@1.2.5");
        assert_eq!(value("X-Zarz-Platform"), "extension");
    }

    #[test]
    fn config_defaults() {
        assert_eq!(SessionConfig::of(&json!({})), None);
        assert_eq!(SessionConfig::of(&json!({"baseUrl": 5})), None);
        let config = SessionConfig::of(&json!({
            "baseUrl": "https://gateway.example.com",
            "endpoints": {"refresh": "/session/refresh"},
            "timeWindowSeconds": 0,
        }))
        .unwrap();
        assert_eq!(config.bootstrap, "/bootstrap");
        assert_eq!(config.challenge, "/challenge");
        assert_eq!(config.exchange, "/session/exchange");
        assert_eq!(config.refresh.as_deref(), Some("/session/refresh"));
        assert_eq!(config.header_prefix, "X-Sig-");
        assert_eq!(config.scheme_label, "SPOTIFLAC-HMAC-V1");
        assert_eq!(config.app_version, "ext-1.0");
        assert_eq!(config.platform, "extension");
        assert_eq!(config.time_window_seconds, 300);
        assert_eq!(config.callback_url, "spotiflac://session-grant");
        assert_eq!(config.namespace, None);
    }

    #[test]
    fn iso_times() {
        assert_eq!(iso_timestamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_timestamp(1_760_937_314_949), "2025-10-20T05:15:14.949Z");
        assert_eq!(iso_timestamp(951_782_400_000), "2000-02-29T00:00:00.000Z");
        for ms in [0, 1_760_937_314_949, 951_782_400_000, 4_102_444_800_000] {
            assert_eq!(parse_iso_ms(&iso_timestamp(ms)), Some(ms));
        }
        let at = Some(1_760_937_314_949);
        assert_eq!(parse_iso_ms("2025-10-20T07:15:14.949+02:00"), at);
        assert_eq!(parse_iso_ms("2025-10-20T05:15:14.949123Z"), at);
        assert_eq!(parse_iso_ms("2025-10-20 05:15:14,949 Z"), at);
        assert_eq!(parse_iso_ms("20251020T051514Z"), Some(1_760_937_314_000));
        assert_eq!(parse_iso_ms("2025-10-20"), Some(1_760_918_400_000));
        assert_eq!(parse_iso_ms("2025-10-20T05:15"), Some(1_760_937_300_000));
        for bad in [
            "",
            "soon",
            "2025-13-01",
            "2025-10-20X",
            "2025-10-20T05:15:14Q",
            "2025-10-20T25:00",
        ] {
            assert_eq!(parse_iso_ms(bad), None, "{bad}");
        }
        assert_eq!(expiry_ms(&json!(1_760_937_314)), Some(1_760_937_314_000));
        assert_eq!(
            expiry_ms(&json!(1_760_937_314_949i64)),
            Some(1_760_937_314_949)
        );
        assert_eq!(expiry_ms(&json!(true)), None);
    }

    #[test]
    fn the_gateway_contract_reaches_the_add_on() {
        // A rejected ticket is final, not retried across every tier.
        let denied = signed_addon_response(
            &json!({"ok": false, "status": 403, "body": "{\"code\":\"TICKET_DENIED\"}"}),
            json!({"code": "TICKET_DENIED", "origin": "gateway", "retryable": false, "retry_mode": "none"})
                .as_object()
                .unwrap(),
        );
        assert_eq!(denied["status"], 403);
        assert_eq!(denied["code"], "TICKET_DENIED");
        assert_eq!(denied["retryable"], false);
        assert_eq!(denied["retryMode"], "none");
        assert_eq!(denied["needsVerification"], false);
        assert_eq!(denied["error_code"], "TICKET_DENIED");
        assert_eq!(denied["error_origin"], "gateway");
        assert_eq!(denied["error_action"], Value::Null);
        // Without retry fields a 403 is still final.
        let bare = signed_addon_response(&json!({"status": 403, "body": ""}), &Map::new());
        assert_eq!(bare["retryMode"], "none");
        assert_eq!(bare["retryable"], false);
        assert_eq!(bare["code"], "");
        assert!(bare.get("error_code").is_none());
        // Retry-after, poll and a passing provider error come through.
        let busy = signed_addon_response(
            &json!({"status": 503, "headers": {"retry-after": "3"}}),
            json!({"code": "provider_unavailable", "retryable": true, "retry_mode": "poll_existing", "retry_after_seconds": 3})
                .as_object()
                .unwrap(),
        );
        assert_eq!(busy["retryMode"], "poll_existing");
        assert_eq!(busy["retryable"], true);
        assert_eq!(busy["retryAfterSeconds"].as_f64(), Some(3.0));
        assert_eq!(busy["code"], "PROVIDER_UNAVAILABLE");
        assert_eq!(busy["headers"]["retry-after"], "3");
        // Verification required is said plainly.
        let verify = signed_addon_response(
            &json!({"status": 428}),
            json!({"code": "VERIFY_REQUIRED", "action": "verify"})
                .as_object()
                .unwrap(),
        );
        assert_eq!(verify["needsVerification"], true);
        assert_eq!(verify["retryMode"], "none");
        // A 5xx without a mode is not final.
        let down = signed_addon_response(&json!({"status": 500}), &Map::new());
        assert_eq!(down["retryMode"], "");
    }

    /// A folder of its own, removed when the test ends.
    struct Folder(PathBuf);

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Rig {
        session: Arc<Session>,
        server: Server,
        store: Arc<Store>,
        network: Arc<Network>,
        _folder: Folder,
    }

    impl Rig {
        /// A session whose gateway is a loopback server answering with `handler`.
        fn new(config: Value, handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Rig {
            let server = Server::start(handler);
            let mut config = config;
            config["baseUrl"] = json!(server.base());
            let folder = Folder(std::env::temp_dir().join(format!(
                "stash-mornye-session-{}",
                crate::hex(&crate::random_bytes(8))
            )));
            let store = Arc::new(Store::new(folder.0.clone(), [5; 32]));
            let network = Arc::new(network(json!({})).0);
            let session = Arc::new(Session::new(
                SessionConfig::of(&config).unwrap(),
                "0123456789",
                Arc::clone(&network),
                Arc::clone(&store),
            ));
            Rig {
                session,
                server,
                store,
                network,
                _folder: folder,
            }
        }

        /// A session the gateway issued earlier, ending `ends_in` ms from now.
        fn signed_in(&self, ends_in: i64) {
            let record = json!({
                "install_id": "install",
                "session_id": "session",
                "session_secret": "secret",
                "expires_at": iso_timestamp(crate::now_ms() + ends_in),
            });
            self.store
                .write(
                    &self.session.namespace,
                    SESSION,
                    record.as_object().unwrap(),
                )
                .unwrap();
        }

        fn requests_to(&self, path: &str) -> Vec<Seen> {
            self.server
                .seen()
                .into_iter()
                .filter(|seen| seen.path() == path)
                .collect()
        }

        fn grants(&self) -> Vec<String> {
            self.requests_to("/session/exchange")
                .iter()
                .map(|seen| {
                    let body: Value = serde_json::from_slice(&seen.body).unwrap();
                    body["grant"].as_str().unwrap().to_string()
                })
                .collect()
        }

        fn pending_state(&self) -> String {
            self.session.state().pending.as_ref().unwrap().state.clone()
        }
    }

    fn no_sleep(_: Duration) -> Result<(), String> {
        Ok(())
    }

    fn issued() -> Reply {
        Reply::json(
            200,
            &json!({
                "session_id": "test-session",
                "session_secret": "test-secret",
                "expires_at": iso_timestamp(crate::now_ms() + DAY_MS),
            }),
        )
    }

    /// A gateway that asks for verification, then exchanges grants.
    fn challenging(seen: &Seen) -> Reply {
        match (seen.method.as_str(), seen.path()) {
            ("GET", "/bootstrap") => Reply::json(200, &json!({"challenge_id": "challenge"})),
            ("POST", "/session/exchange") => issued(),
            _ => Reply::new(404, ""),
        }
    }

    fn callback(state: &str, grant: Option<(&str, &str)>) -> String {
        let mut url = Url::parse("spotiflac://session-grant").unwrap();
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("state", state);
            if let Some((key, value)) = grant {
                query.append_pair(key, value);
            }
        }
        url.to_string()
    }

    fn wrapper(target: &str) -> String {
        let mut url = Url::parse("https://gateway.example.com/return").unwrap();
        url.query_pairs_mut().append_pair("cb", target);
        url.to_string()
    }

    fn verified_rig() -> Rig {
        let rig = Rig::new(json!({}), challenging);
        assert_eq!(rig.session.bootstrap(), Ok(false));
        rig
    }

    #[test]
    fn the_challenge_page_opens_and_is_not_taken_for_the_callback() {
        let rig = verified_rig();
        let page = rig.session.pending_url().unwrap();
        let cb = param(&Url::parse(&page).unwrap(), "cb").unwrap();
        assert!(cb.starts_with("spotiflac://session-grant?cb_version=v2grant&state="));
        assert_eq!(query_value(&cb, "state"), Some(rig.pending_state()));
        assert!(!rig.session.is_callback(&page));
        assert!(rig.grants().is_empty());
        // The bootstrap named the app version and the install id.
        let bootstrap = &rig.requests_to("/bootstrap")[0];
        assert!(bootstrap.target.contains("app_version=ext-1.0"));
        let install = rig.session.status()["install_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(install.len(), 32);
        assert!(bootstrap.target.contains(&format!("install_id={install}")));
        let status = rig.session.status();
        assert_eq!(status["verification_required"], true);
        assert_eq!(status["authenticated"], false);
        assert_eq!(status["auth_url"], page);
    }

    #[test]
    fn a_callback_template_inside_another_page_is_not_a_grant() {
        let rig = verified_rig();
        let state = rig.pending_state();
        assert!(!rig.session.is_callback(&wrapper(&callback(&state, None))));
        assert!(
            !rig.session
                .is_callback(&wrapper(&callback(&state, Some(("grant", "")))))
        );
        assert!(
            !rig.session
                .is_callback("https://gateway.example.com/challenge")
        );
    }

    #[test]
    fn direct_and_wrapped_grants_are_exchanged() {
        for wrapped in [false, true] {
            let rig = verified_rig();
            let target = callback(&rig.pending_state(), Some(("grant", "issued-grant")));
            let raw = if wrapped { wrapper(&target) } else { target };
            assert!(rig.session.is_callback(&raw));
            assert_eq!(rig.session.callback(&raw), Ok(json!({"success": true})));
            assert_eq!(rig.grants(), vec!["issued-grant".to_string()]);
            let status = rig.session.status();
            assert_eq!(status["authenticated"], true);
            assert_eq!(status["session_id"], "test-session");
            assert_eq!(rig.session.pending_url(), None);
            // The exchange named the install id, app version and platform, in that order.
            let body =
                String::from_utf8(rig.requests_to("/session/exchange")[0].body.clone()).unwrap();
            assert!(body.starts_with("{\"grant\":\"issued-grant\",\"install_id\":\""));
            assert!(body.ends_with("\"app_version\":\"ext-1.0\",\"platform\":\"extension\"}"));
        }
    }

    #[test]
    fn a_legacy_code_is_exchanged_too() {
        let rig = verified_rig();
        let raw = callback(&rig.pending_state(), Some(("code", "legacy-code")));
        assert!(rig.session.is_callback(&raw));
        rig.session.callback(&raw).unwrap();
        assert_eq!(rig.grants(), vec!["legacy-code".to_string()]);
    }

    #[test]
    fn a_callback_without_a_grant_is_refused() {
        let rig = verified_rig();
        let state = rig.pending_state();
        let raw = callback(&state, None);
        assert!(rig.session.is_callback(&raw));
        assert_eq!(
            rig.session.callback(&raw),
            Err(
                "The verification page returned no grant. Please try verification again."
                    .to_string()
            )
        );
        assert!(
            rig.session
                .callback(&callback(&state, Some(("grant", "  "))))
                .is_err()
        );
        assert!(rig.grants().is_empty());
        assert!(rig.session.pending_url().is_some());
        assert_eq!(rig.session.status()["authenticated"], false);
        assert_eq!(
            rig.session.complete_grant(None),
            Err("No pending verification grant".to_string())
        );
    }

    #[test]
    fn wrong_state_foreign_targets_and_old_pages_are_refused() {
        let rig = verified_rig();
        let invalid = Err("Invalid or expired verification callback".to_string());
        assert_eq!(
            rig.session
                .callback(&callback("wrong", Some(("grant", "g")))),
            invalid
        );
        let foreign = callback(&rig.pending_state(), Some(("grant", "g")))
            .replace("session-grant", "other-target");
        assert!(!rig.session.is_callback(&foreign));
        assert_eq!(rig.session.callback(&foreign), invalid);
        let raw = callback(&rig.pending_state(), Some(("grant", "g")));
        rig.session.state().pending.as_mut().unwrap().created_ms -= 4 * 60 * 1000;
        assert_eq!(rig.session.callback(&raw), invalid);
        assert!(rig.grants().is_empty());
    }

    #[test]
    fn a_session_is_issued_straight_away_when_the_gateway_needs_no_page() {
        let rig = Rig::new(json!({}), |_| issued());
        assert_eq!(rig.session.bootstrap(), Ok(true));
        assert_eq!(rig.session.status()["authenticated"], true);
        let install = rig.session.status()["install_id"].clone();
        assert_eq!(rig.session.clear(), json!({"success": true}));
        let status = rig.session.status();
        assert_eq!(status["authenticated"], false);
        assert_eq!(status["session_id"], "");
        assert_eq!(status["install_id"], install);
    }

    #[test]
    fn a_bootstrap_runs_once_for_callers_who_ask_together() {
        let rig = Rig::new(json!({}), |seen| {
            std::thread::sleep(Duration::from_millis(300));
            challenging(seen)
        });
        std::thread::scope(|scope| {
            let first = scope.spawn(|| rig.session.bootstrap());
            let second = scope.spawn(|| rig.session.bootstrap());
            assert_eq!(first.join().unwrap(), Ok(false));
            assert_eq!(second.join().unwrap(), Ok(false));
        });
        assert_eq!(rig.requests_to("/bootstrap").len(), 1);
    }

    #[test]
    fn requests_are_signed() {
        let rig = Rig::new(json!({"appVersion": "test@1"}), |seen| match seen.path() {
            "/tickets" => Reply::json(200, &json!({"ticket": "t"})),
            _ => Reply::new(404, ""),
        });
        rig.signed_in(DAY_MS);
        let headers = vec![
            ("X-Extra".to_string(), "yes".to_string()),
            ("X-Sig-Session".to_string(), "forged".to_string()),
        ];
        let answer = rig
            .session
            .signed_fetch("post", "/tickets", "{\"id\":1}", &headers, &no_sleep)
            .unwrap();
        assert_eq!(answer["ok"], true);
        assert_eq!(answer["body"], "{\"ticket\":\"t\"}");
        assert_eq!(answer["code"], "");
        assert_eq!(answer["retryable"], false);
        assert_eq!(answer["retryMode"], "");
        assert_eq!(answer["needsVerification"], false);
        let seen = &rig.requests_to("/tickets")[0];
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.header("content-type"), Some("application/json"));
        assert_eq!(seen.header("x-extra"), Some("yes"));
        assert_eq!(seen.header("x-sig-session"), Some("session"));
        assert_eq!(seen.header("x-sig-app-version"), Some("test@1"));
        // The signature checks out against what was sent.
        let expected = signed_headers(
            rig.session.config(),
            "session",
            "secret",
            "POST",
            "/tickets",
            "{\"id\":1}",
            parse_iso_ms(seen.header("x-sig-timestamp").unwrap()).unwrap(),
            seen.header("x-sig-nonce").unwrap(),
        );
        assert_eq!(seen.header("x-sig-signature"), Some(expected[4].1.as_str()));
        assert_eq!(seen.header("x-sig-nonce").unwrap().len(), 32);
    }

    #[test]
    fn without_a_session_the_add_on_is_told_to_verify() {
        let rig = Rig::new(json!({}), challenging);
        let answer = rig
            .session
            .signed_fetch("GET", "/search", "", &[], &no_sleep)
            .unwrap();
        let page = rig.session.pending_url().unwrap();
        assert_eq!(
            answer,
            json!({"ok": false, "needsVerification": true, "error": "VERIFY_REQUIRED", "auth_url": page, "open_auth_url": page})
        );
        assert!(rig.requests_to("/search").is_empty());
    }

    #[test]
    fn a_gateway_asking_for_verification_again_blocks_the_session() {
        let rig = Rig::new(json!({}), |seen| match seen.path() {
            "/search" => Reply::json(
                428,
                &json!({"origin": "gateway", "code": "VERIFY_REQUIRED", "action": "verify"}),
            ),
            _ => challenging(seen),
        });
        rig.signed_in(DAY_MS);
        let answer = rig
            .session
            .signed_fetch("GET", "/search", "", &[], &no_sleep)
            .unwrap();
        assert_eq!(answer["error"], "VERIFY_REQUIRED");
        assert_eq!(rig.session.status()["authenticated"], false);
        assert!(rig.session.pending_url().is_some());
    }

    #[test]
    fn an_invalid_session_is_replaced() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let rig = Rig::new(json!({}), move |seen| match seen.path() {
            "/search" if counted.fetch_add(1, Ordering::SeqCst) == 0 => Reply::json(
                401,
                &json!({"origin": "gateway", "code": "SESSION_INVALID", "action": "bootstrap_session"}),
            ),
            "/search" => Reply::json(200, &json!({})),
            _ => issued(),
        });
        rig.signed_in(DAY_MS);
        let answer = rig
            .session
            .signed_fetch("GET", "/search", "", &[], &no_sleep)
            .unwrap();
        assert_eq!(answer["ok"], true);
        assert_eq!(rig.session.status()["session_id"], "test-session");
        let searches = rig.requests_to("/search");
        assert_eq!(searches[1].header("x-sig-session"), Some("test-session"));
    }

    #[test]
    fn a_passing_provider_outage_is_waited_out() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let rig = Rig::new(json!({}), move |_| {
            if counted.fetch_add(1, Ordering::SeqCst) < 2 {
                Reply::json(
                    503,
                    &json!({"origin": "provider", "code": "provider_unavailable", "retryable": true,
                            "retry_mode": "same_operation", "retry_after_seconds": 2.7}),
                )
            } else {
                Reply::json(200, &json!({}))
            }
        });
        rig.signed_in(DAY_MS);
        let waits = Mutex::new(Vec::new());
        let sleep = |wait: Duration| -> Result<(), String> {
            waits.lock().unwrap().push(wait);
            Ok(())
        };
        let answer = rig
            .session
            .signed_fetch("GET", "/x", "", &[], &sleep)
            .unwrap();
        assert_eq!(answer["ok"], true);
        assert_eq!(*waits.lock().unwrap(), vec![Duration::from_secs(2); 2]);
        // A cancelled wait ends the call with an error answer.
        calls.store(0, Ordering::SeqCst);
        let cancelled = |_: Duration| -> Result<(), String> {
            Err("Extension operation cancelled".to_string())
        };
        let answer = rig
            .session
            .signed_fetch("GET", "/x", "", &[], &cancelled)
            .unwrap();
        assert_eq!(
            answer,
            json!({"ok": false, "error": "Extension operation cancelled"})
        );
    }

    #[test]
    fn a_session_near_its_end_is_refreshed() {
        let rig = Rig::new(
            json!({"endpoints": {"refresh": "/session/refresh"}}),
            |seen| match seen.path() {
                "/session/refresh" => Reply::json(
                    200,
                    &json!({"session_secret": "fresh", "expires_at": iso_timestamp(crate::now_ms() + DAY_MS)}),
                ),
                _ => Reply::json(200, &json!({})),
            },
        );
        rig.signed_in(10 * 60 * 1000);
        rig.session
            .signed_fetch("GET", "/x", "", &[], &no_sleep)
            .unwrap();
        let refresh = &rig.requests_to("/session/refresh")[0];
        assert_eq!(refresh.body, b"{\"install_id\":\"install\"}");
        let record = rig.store.read(&rig.session.namespace, SESSION);
        assert_eq!(record["session_secret"], "fresh");
        assert_eq!(record["session_id"], "session");
        assert_eq!(rig.requests_to("/x").len(), 1);
        // Far from its end: no refresh.
        rig.signed_in(DAY_MS);
        rig.session
            .signed_fetch("GET", "/x", "", &[], &no_sleep)
            .unwrap();
        assert_eq!(rig.requests_to("/session/refresh").len(), 1);
    }

    #[test]
    fn a_429_holds_every_request_until_the_cooldown_ends() {
        let rig = Rig::new(json!({}), |_| {
            Reply::json(429, &json!({})).with("Retry-After", "120")
        });
        rig.signed_in(DAY_MS);
        for _ in 0..25 {
            let failed = rig
                .session
                .signed_fetch("POST", "/tickets", "", &[], &no_sleep);
            assert_eq!(failed, Err(rate_limit_message(120)));
        }
        assert_eq!(rig.server.seen().len(), 1);
        // A new session object over the same store keeps the cooldown.
        let again = Session::new(
            rig.session.config().clone(),
            "0123456789",
            Arc::clone(&rig.network),
            Arc::clone(&rig.store),
        );
        assert!(
            again
                .signed_fetch("POST", "/tickets", "", &[], &no_sleep)
                .is_err()
        );
        assert_eq!(again.rate_limited_for(), Some(120));
        assert_eq!(again.check_rate(), Err(rate_limit_message(120)));
        assert_eq!(again.status()["authenticated"], true);
        assert_eq!(rig.server.seen().len(), 1);
        // Once it has passed, requests go again.
        let past = json!({"until": crate::now_ms() - 1});
        rig.store
            .write(
                &rig.session.namespace,
                RATE_LIMIT,
                past.as_object().unwrap(),
            )
            .unwrap();
        assert_eq!(rig.session.rate_limited_for(), None);
    }

    #[test]
    fn the_longer_cooldown_wins_and_dates_count() {
        for (body, header, expected) in [
            (json!({"retry_after_seconds": 180}), "60".to_string(), 180),
            (json!({"retry_after_seconds": "30"}), "90.2".to_string(), 91),
            (json!({}), "soon".to_string(), 60),
            (json!({}), http_date(crate::now_ms() + 300_000), 300),
        ] {
            let rig = Rig::new(json!({}), move |_| {
                Reply::json(429, &body).with("Retry-After", &header)
            });
            rig.signed_in(DAY_MS);
            assert!(
                rig.session
                    .signed_fetch("GET", "/x", "", &[], &no_sleep)
                    .is_err()
            );
            let left = rig.session.rate_limited_for().unwrap();
            assert!(
                (expected - 2..=expected).contains(&left),
                "{left} for {expected}"
            );
        }
    }

    fn http_date(ms: i64) -> String {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let (year, month, day) = civil_from_days(ms.div_euclid(DAY_MS));
        let rest = ms.rem_euclid(DAY_MS) / 1000;
        format!(
            "Mon, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
            MONTHS[month as usize - 1],
            rest / 3600,
            rest / 60 % 60,
            rest % 60
        )
    }

    #[test]
    fn endpoints_stay_under_the_base() {
        let store = Arc::new(Store::new(
            std::env::temp_dir().join("stash-mornye-unused"),
            [0; 32],
        ));
        let network = Arc::new(network(json!({})).0);
        let session = |base: &str| {
            Session::new(
                SessionConfig::of(&json!({ "baseUrl": base })).unwrap(),
                "0123456789",
                Arc::clone(&network),
                Arc::clone(&store),
            )
        };
        let nested = session("https://gateway.example.com/v2/");
        assert_eq!(
            nested.endpoint("/tickets").unwrap().as_str(),
            "https://gateway.example.com/v2/tickets"
        );
        assert_eq!(
            nested.endpoint("//tickets?x=1").unwrap().as_str(),
            "https://gateway.example.com/v2/tickets?x=1"
        );
        let escaped = Err("Signed-session path escaped its base URL".to_string());
        assert_eq!(
            nested.endpoint("/../admin").map(|url| url.to_string()),
            escaped
        );
        assert_eq!(
            nested.endpoint("/%2e%2e/admin").map(|url| url.to_string()),
            escaped
        );
        // A backslash is a slash in an https address: `..` then only climbs back to the base.
        assert_eq!(
            nested.endpoint("\\..\\admin").map(|url| url.to_string()),
            Ok("https://gateway.example.com/v2/admin".to_string())
        );
        let root = session("https://gateway.example.com");
        assert_eq!(
            root.endpoint("bootstrap").unwrap().as_str(),
            "https://gateway.example.com/bootstrap"
        );
        // Sessions that name one namespace share it; others take the add-on's key.
        let shared = |version: &str| {
            Session::new(
                SessionConfig::of(&json!({"baseUrl": "https://gateway.example.com", "namespace": "shared", "appVersion": version}))
                    .unwrap(),
                "0123456789",
                Arc::clone(&network),
                Arc::clone(&store),
            )
            .namespace
        };
        assert_eq!(shared("a@1"), shared("a@1"));
        assert_ne!(shared("a@1"), shared("b@1"));
        assert_eq!(shared("a@1").len(), 64);
    }
}
