//! `auth.*`: an add-on's own OAuth login (spec §2.5; Lokal `morneye_host.dart` authCall 949-1074, and the callback
//! in `morneye_packages.dart` 1736-1768). The state lives in the add-on's [`Network`] (`auth`), kept sealed with its
//! cookies.

use serde_json::{Map, Value, json};
use url::Url;

use crate::net::{Network, text_of, validate_url};

/// How long a login page may take before its callback is refused.
const CALLBACK_MAX_AGE_MS: f64 = 180_000.0;
/// A token's life when the server names none.
const DEFAULT_LIFETIME_SECONDS: i64 = 3600;
/// Actions that only read: the connection record is not kept after them.
const READS: [&str; 4] = ["getAuthCode", "getTokens", "isAuthenticated", "getPKCE"];

/// One `auth.<action>` call. A call during which the person disconnected the add-on (the network's generation
/// moved) answers `{success: false, error: "Disconnected"}`. Unknown actions fail with `Unknown auth API`.
pub fn call(network: &Network, action: &str, args: &[Value]) -> Result<Value, String> {
    let generation = network.generation();
    let result = run(network, action, args)?;
    if network.generation() != generation {
        return Ok(disconnected());
    }
    if !READS.contains(&action) {
        network.keep();
    }
    Ok(result)
}

/// The login page to open, while one is pending.
pub fn pending_url(network: &Network) -> Option<String> {
    network
        .auth()
        .get("pending")
        .and_then(|pending| pending.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Whether the login window is about to open the pending login's callback (same scheme, authority and path).
pub fn is_callback(network: &Network, url: &str) -> bool {
    let callback = network
        .auth()
        .get("pending")
        .filter(|pending| pending.is_object())
        .and_then(|pending| pending.get("callback"))
        .filter(|callback| !callback.is_null())
        .map(text_of)
        .unwrap_or_default();
    if callback.is_empty() {
        return false;
    }
    let (Ok(expected), Ok(actual)) = (Url::parse(&callback), Url::parse(url)) else {
        return false;
    };
    actual.scheme() == expected.scheme()
        && actual.authority() == expected.authority()
        && actual.path() == expected.path()
}

/// The login page came back to its callback: its `code` is kept for the add-on (`Invalid or expired OAuth
/// callback` when nothing was pending, it is older than 3 minutes, or its `state` differs).
pub fn callback(network: &Network, url: &str) -> Result<Value, String> {
    const INVALID: &str = "Invalid or expired OAuth callback";
    let url = Url::parse(url).map_err(|_| INVALID.to_string())?;
    let state = param(&url, "state");
    {
        let mut auth = network.auth();
        let Some(Value::Object(pending)) = auth.get("pending") else {
            return Err(INVALID.to_string());
        };
        let stale = pending
            .get("createdAt")
            .and_then(Value::as_f64)
            .is_none_or(|created| crate::now_ms() as f64 - created > CALLBACK_MAX_AGE_MS);
        let expected = pending.get("state").and_then(Value::as_str);
        if stale || expected.is_none() || state.as_deref() != expected {
            return Err(INVALID.to_string());
        }
        // Nothing calls the add-on now: it reads the code (`getAuthCode`) or exchanges it on its next call.
        auth.insert(
            "code".into(),
            Value::String(param(&url, "code").unwrap_or_default()),
        );
        auth.insert("pending".into(), Value::Null);
    }
    network.keep();
    Ok(json!({ "success": true }))
}

/// Whether the add-on holds a login (an `access_token`, `refresh_token` or `code`).
pub fn has_login(network: &Network) -> bool {
    let auth = network.auth();
    ["access_token", "refresh_token", "code"]
        .iter()
        .any(|key| auth.get(*key).is_some_and(|value| !value.is_null()))
}

/// One action (host.dart:949-1074).
fn run(network: &Network, action: &str, args: &[Value]) -> Result<Value, String> {
    let value = args.first().unwrap_or(&Value::Null);
    match action {
        "getAuthCode" => Ok(network.auth().get("code").cloned().unwrap_or(Value::Null)),
        "getTokens" => {
            let auth = network.auth();
            Ok(json!({
                "access_token": or_empty(&auth, "access_token"),
                "refresh_token": or_empty(&auth, "refresh_token"),
                "expires_at": auth.get("expires_at").cloned().unwrap_or(Value::Null),
            }))
        }
        "isAuthenticated" => {
            let auth = network.auth();
            let token = auth
                .get("access_token")
                .filter(|token| !token.is_null())
                .map(text_of)
                .unwrap_or_default();
            let fresh = match auth.get("expires_at") {
                None | Some(Value::Null) => true,
                Some(Value::Number(at)) => {
                    at.as_f64().is_some_and(|at| at > crate::now_ms() as f64)
                }
                Some(_) => false,
            };
            Ok(Value::Bool(!token.is_empty() && fresh))
        }
        "clearAuth" => {
            network.auth().clear();
            Ok(Value::Bool(true))
        }
        "setAuthCode" => {
            let mut auth = network.auth();
            match value {
                Value::String(code) => {
                    auth.insert("code".into(), Value::String(code.clone()));
                }
                // An add-on may keep its own tokens this way.
                Value::Object(fields) => {
                    for (key, field) in fields {
                        auth.insert(key.clone(), field.clone());
                    }
                }
                _ => {}
            }
            Ok(Value::Bool(true))
        }
        "generatePKCE" => Ok(generate_pkce(network, value)),
        "getPKCE" => Ok(network
            .auth()
            .get("pkce")
            .filter(|pkce| !pkce.is_null())
            .cloned()
            .unwrap_or_else(|| json!({}))),
        "openAuthUrl" => open_auth_url(network, args, false),
        "startOAuthWithPKCE" => open_auth_url(network, args, true),
        "exchangeCodeWithPKCE" => exchange(network, value),
        _ => Err("Unknown auth API".to_string()),
    }
}

/// A PKCE pair (`{verifier, challenge, method: 'S256'}`), kept as `auth.pkce`: the verifier is the first `length`
/// (43 to 128, else 64) characters of base64url(96 random bytes).
fn generate_pkce(network: &Network, length: &Value) -> Value {
    let length = length
        .as_f64()
        .filter(|length| length.is_finite())
        .map_or(64, |length| length.trunc().clamp(43.0, 128.0) as usize);
    let verifier: String = crate::bytes::base64url_nopad(&crate::random_bytes(96))
        .chars()
        .take(length)
        .collect();
    let challenge = crate::bytes::base64url_nopad(&crate::crypto::sha256(verifier.as_bytes()));
    let pkce = json!({ "verifier": verifier, "challenge": challenge, "method": "S256" });
    network.auth().insert("pkce".into(), pkce.clone());
    pkce
}

/// `openAuthUrl(authUrl, redirectUri)` / `openAuthUrl({authUrl, redirectUri})`, and `startOAuthWithPKCE({authUrl,
/// clientId, redirectUri, scope | scopes, extraParams})`, which adds a PKCE challenge and the OAuth parameters.
fn open_auth_url(network: &Network, args: &[Value], with_pkce: bool) -> Result<Value, String> {
    let first = args.first().unwrap_or(&Value::Null);
    let config = match first {
        Value::Object(map) => map.clone(),
        other => {
            let mut map = Map::new();
            map.insert("authUrl".into(), other.clone());
            map.insert(
                "redirectUri".into(),
                args.get(1).cloned().unwrap_or(Value::Null),
            );
            map
        }
    };
    let auth_url = text_of(config.get("authUrl").unwrap_or(&Value::Null));
    let mut url = validate_url(&auth_url, network.permissions())?;
    let state = crate::bytes::base64url_nopad(&crate::random_bytes(24));
    let mut query: Vec<(String, String)> = Vec::new();
    for (key, value) in url.query_pairs() {
        set_pair(&mut query, &key, value.into_owned());
    }
    set_pair(&mut query, "state", state.clone());
    if with_pkce {
        let pkce = generate_pkce(network, &json!(64));
        let scopes = config
            .get("scopes")
            .and_then(Value::as_array)
            .map(|scopes| scopes.iter().map(text_of).collect::<Vec<_>>().join(" "));
        let scope = config
            .get("scope")
            .filter(|scope| !scope.is_null())
            .cloned()
            .or(scopes.map(Value::String));
        let mut extra: Vec<(String, Option<Value>)> = vec![
            ("client_id".into(), config.get("clientId").cloned()),
            ("redirect_uri".into(), config.get("redirectUri").cloned()),
            ("response_type".into(), Some(json!("code"))),
            ("scope".into(), scope),
            ("code_challenge".into(), pkce.get("challenge").cloned()),
            ("code_challenge_method".into(), Some(json!("S256"))),
        ];
        if let Some(Value::Object(params)) = config.get("extraParams") {
            for (key, value) in params {
                set_pair(&mut extra, key, Some(value.clone()));
            }
        }
        // Parameters left null are skipped.
        for (key, value) in extra {
            if let Some(value) = value.filter(|value| !value.is_null()) {
                set_pair(&mut query, &key, text_of(&value));
            }
        }
    }
    {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (key, value) in &query {
            pairs.append_pair(key, value);
        }
    }
    let open = url.to_string();
    let callback = config
        .get("redirectUri")
        .filter(|callback| !callback.is_null())
        .cloned()
        .unwrap_or_else(|| json!(""));
    network.auth().insert(
        "pending".into(),
        json!({ "url": open, "state": state, "createdAt": crate::now_ms(), "callback": callback }),
    );
    Ok(json!({
        "success": true,
        "open_auth_url": open,
        "message": "Log in on the page Stash opens; access is checked when it closes.",
    }))
}

/// `exchangeCodeWithPKCE({tokenUrl, clientId, code?, redirectUri?, extraParams?})`: the code (the config's, else the
/// one kept) and the PKCE verifier are posted form-encoded to `tokenUrl`; the tokens are kept with `expires_at`.
fn exchange(network: &Network, config: &Value) -> Result<Value, String> {
    let empty = Map::new();
    let config = config.as_object().unwrap_or(&empty);
    let (code, verifier) = {
        let auth = network.auth();
        let code = [config.get("code"), auth.get("code")]
            .into_iter()
            .flatten()
            .find(|code| !code.is_null())
            .map(text_of)
            .unwrap_or_default();
        let verifier = auth
            .get("pkce")
            .filter(|pkce| pkce.is_object())
            .and_then(|pkce| pkce.get("verifier"))
            .filter(|verifier| !verifier.is_null())
            .map(text_of)
            .unwrap_or_default();
        (code, verifier)
    };
    let client_id = config
        .get("clientId")
        .filter(|client| !client.is_null())
        .map(text_of)
        .unwrap_or_default();
    let mut form: Vec<(String, String)> = vec![
        ("grant_type".into(), "authorization_code".into()),
        ("client_id".into(), client_id),
        ("code".into(), code),
        ("code_verifier".into(), verifier),
    ];
    if let Some(redirect) = config
        .get("redirectUri")
        .filter(|redirect| !redirect.is_null())
    {
        set_pair(&mut form, "redirect_uri", text_of(redirect));
    }
    if let Some(Value::Object(params)) = config.get("extraParams") {
        for (key, value) in params {
            set_pair(&mut form, key, text_of(value));
        }
    }
    // URLSearchParams encoding: `*-._` and alphanumerics as they are, space as `+`, the rest %XX.
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in &form {
        serializer.append_pair(key, value);
    }
    let body = serializer.finish();
    let generation = network.generation();
    let token_url = text_of(config.get("tokenUrl").unwrap_or(&Value::Null));
    let headers = [(
        "Content-Type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    )];
    let answer = network.json_response(&token_url, "POST", &body, &headers, false)?;
    if answer.get("ok") != Some(&Value::Bool(true)) {
        return Err(format!(
            "OAuth exchange returned HTTP {}",
            text_of(answer.get("status").unwrap_or(&Value::Null))
        ));
    }
    // Disconnected while it ran: the tokens are not written back.
    if network.generation() != generation {
        return Ok(disconnected());
    }
    let tokens = match answer
        .get("body")
        .and_then(Value::as_str)
        .map(serde_json::from_str::<Value>)
    {
        Some(Ok(Value::Object(tokens))) => tokens,
        _ => return Err("OAuth exchange returned an invalid answer".to_string()),
    };
    let lifetime = tokens
        .get("expires_in")
        .and_then(|value| match value {
            Value::Number(seconds) => seconds.as_f64(),
            Value::String(text) => text.trim().parse::<f64>().ok(),
            _ => None,
        })
        .filter(|seconds| seconds.is_finite())
        .map_or(DEFAULT_LIFETIME_SECONDS, |seconds| seconds.trunc() as i64);
    {
        let mut auth = network.auth();
        for (key, value) in &tokens {
            auth.insert(key.clone(), value.clone());
        }
        let expires_at = crate::now_ms().saturating_add(lifetime.saturating_mul(1000));
        auth.insert("expires_at".into(), Value::from(expires_at));
    }
    let mut out = Map::new();
    out.insert("success".into(), Value::Bool(true));
    out.extend(tokens);
    Ok(Value::Object(out))
}

fn disconnected() -> Value {
    json!({ "success": false, "error": "Disconnected" })
}

/// `auth[key] ?? ''`.
fn or_empty(auth: &Map<String, Value>, key: &str) -> Value {
    auth.get(key)
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| json!(""))
}

/// Sets `key` in an ordered list of pairs, as a Dart map does: a key set again keeps its place.
fn set_pair<V>(pairs: &mut Vec<(String, V)>, key: &str, value: V) {
    match pairs.iter_mut().find(|(name, _)| name == key) {
        Some(slot) => slot.1 = value,
        None => pairs.push((key.to_string(), value)),
    }
}

/// The query parameter `name` (the last when repeated).
fn param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .filter(|(key, _)| key == name)
        .last()
        .map(|(_, value)| value.into_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::net::testing::{Reply, Server, network};

    fn kept_count(kept: &Arc<Mutex<Vec<Value>>>) -> usize {
        kept.lock().unwrap().len()
    }

    #[test]
    fn codes_and_tokens() {
        let (network, kept) = network(json!({}));
        assert_eq!(call(&network, "getAuthCode", &[]), Ok(Value::Null));
        assert_eq!(
            call(&network, "setAuthCode", &[json!("abc")]),
            Ok(json!(true))
        );
        assert_eq!(call(&network, "getAuthCode", &[]), Ok(json!("abc")));
        assert!(has_login(&network));
        call(
            &network,
            "setAuthCode",
            &[json!({"access_token": "t", "refresh_token": "r"})],
        )
        .unwrap();
        assert_eq!(
            call(&network, "getTokens", &[]),
            Ok(json!({"access_token": "t", "refresh_token": "r", "expires_at": null}))
        );
        assert_eq!(call(&network, "isAuthenticated", &[]), Ok(json!(true)));
        let past = crate::now_ms() - 1000;
        call(&network, "setAuthCode", &[json!({ "expires_at": past })]).unwrap();
        assert_eq!(call(&network, "isAuthenticated", &[]), Ok(json!(false)));
        call(
            &network,
            "setAuthCode",
            &[json!({ "expires_at": crate::now_ms() + 60_000 })],
        )
        .unwrap();
        assert_eq!(call(&network, "isAuthenticated", &[]), Ok(json!(true)));
        call(
            &network,
            "setAuthCode",
            &[json!({ "expires_at": "tomorrow" })],
        )
        .unwrap();
        assert_eq!(call(&network, "isAuthenticated", &[]), Ok(json!(false)));
        // Reads are not kept; changes are.
        let before = kept_count(&kept);
        call(&network, "getTokens", &[]).unwrap();
        call(&network, "getAuthCode", &[]).unwrap();
        assert_eq!(kept_count(&kept), before);
        assert_eq!(call(&network, "clearAuth", &[]), Ok(json!(true)));
        assert_eq!(kept_count(&kept), before + 1);
        assert_eq!(
            call(&network, "getTokens", &[]),
            Ok(json!({"access_token": "", "refresh_token": "", "expires_at": null}))
        );
        assert!(!has_login(&network));
        assert_eq!(
            call(&network, "nope", &[]),
            Err("Unknown auth API".to_string())
        );
    }

    #[test]
    fn pkce_pairs() {
        let (network, _) = network(json!({}));
        for (asked, length) in [
            (json!(50), 50),
            (json!(10), 43),
            (json!(500), 128),
            (json!(60.9), 60),
            (json!("x"), 64),
            (Value::Null, 64),
        ] {
            let pkce = call(&network, "generatePKCE", &[asked]).unwrap();
            let verifier = pkce["verifier"].as_str().unwrap();
            assert_eq!(verifier.len(), length);
            assert!(
                verifier
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            );
            assert_eq!(
                pkce["challenge"],
                crate::bytes::base64url_nopad(&crate::crypto::sha256(verifier.as_bytes()))
            );
            assert_eq!(pkce["method"], "S256");
            assert_eq!(call(&network, "getPKCE", &[]), Ok(pkce));
        }
        network.auth().clear();
        assert_eq!(call(&network, "getPKCE", &[]), Ok(json!({})));
    }

    #[test]
    fn a_login_page_comes_back_with_its_code() {
        let (network, kept) = network(json!({}));
        let opened = call(
            &network,
            "openAuthUrl",
            &[
                json!("https://login.example.com/authorize?client=1"),
                json!("https://app.example.com/callback"),
            ],
        )
        .unwrap();
        assert_eq!(opened["success"], true);
        assert_eq!(
            opened["message"],
            "Log in on the page Stash opens; access is checked when it closes."
        );
        let page = Url::parse(opened["open_auth_url"].as_str().unwrap()).unwrap();
        assert_eq!(param(&page, "client").as_deref(), Some("1"));
        let state = param(&page, "state").unwrap();
        assert_eq!(state.len(), 32);
        assert_eq!(pending_url(&network).as_deref(), Some(page.as_str()));
        // The pending login is never kept on disk.
        assert!(
            kept.lock().unwrap().last().unwrap()["auth"]
                .get("pending")
                .is_none()
        );
        assert!(is_callback(
            &network,
            "https://app.example.com/callback?code=c&state=s"
        ));
        assert!(!is_callback(
            &network,
            "https://app.example.com/other?code=c"
        ));
        assert!(!is_callback(&network, "https://login.example.com/callback"));
        let invalid = Err("Invalid or expired OAuth callback".to_string());
        assert_eq!(
            callback(
                &network,
                "https://app.example.com/callback?code=c&state=wrong"
            ),
            invalid
        );
        let back = format!("https://app.example.com/callback?code=the-code&state={state}");
        assert_eq!(callback(&network, &back), Ok(json!({"success": true})));
        assert_eq!(call(&network, "getAuthCode", &[]), Ok(json!("the-code")));
        assert_eq!(pending_url(&network), None);
        assert!(!is_callback(&network, &back));
        assert!(has_login(&network));
        assert_eq!(
            kept.lock().unwrap().last().unwrap()["auth"]["code"],
            "the-code"
        );
        // Nothing pending now.
        assert_eq!(callback(&network, &back), invalid);
    }

    #[test]
    fn old_or_foreign_login_pages_are_refused() {
        let (network, _) = network(json!({}));
        assert_eq!(
            call(
                &network,
                "openAuthUrl",
                &[json!("https://evil.example.org/login")]
            ),
            Err("Network permission denied for evil.example.org".to_string())
        );
        assert_eq!(
            call(
                &network,
                "openAuthUrl",
                &[json!({"authUrl": "http://login.example.com/"})]
            )
            .map(|_| ()),
            Ok(())
        );
        let state = network.auth()["pending"]["state"]
            .as_str()
            .unwrap()
            .to_string();
        // Without a redirect URI nothing is taken for the callback.
        assert!(!is_callback(&network, "https://app.example.com/callback"));
        network.auth()["pending"]["createdAt"] = json!(crate::now_ms() - 4 * 60 * 1000);
        assert_eq!(
            callback(
                &network,
                &format!("https://app.example.com/cb?state={state}")
            ),
            Err("Invalid or expired OAuth callback".to_string())
        );
    }

    #[test]
    fn an_oauth_start_names_the_pkce_challenge_and_parameters() {
        let (network, _) = network(json!({}));
        let opened = call(
            &network,
            "startOAuthWithPKCE",
            &[json!({
                "authUrl": "https://login.example.com/authorize?audience=music",
                "clientId": "client",
                "redirectUri": "https://app.example.com/cb",
                "scopes": ["read", "write"],
                "extraParams": {"prompt": "consent", "client_id": "override", "skipped": null},
            })],
        )
        .unwrap();
        let page = Url::parse(opened["open_auth_url"].as_str().unwrap()).unwrap();
        let pairs: Vec<(String, String)> = page
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "audience",
                "state",
                "client_id",
                "redirect_uri",
                "response_type",
                "scope",
                "code_challenge",
                "code_challenge_method",
                "prompt"
            ]
        );
        let value = |key: &str| param(&page, key).unwrap();
        assert_eq!(value("client_id"), "override");
        assert_eq!(value("redirect_uri"), "https://app.example.com/cb");
        assert_eq!(value("response_type"), "code");
        assert_eq!(value("scope"), "read write");
        assert_eq!(value("code_challenge_method"), "S256");
        assert_eq!(value("prompt"), "consent");
        let pkce = call(&network, "getPKCE", &[]).unwrap();
        assert_eq!(pkce["verifier"].as_str().unwrap().len(), 64);
        assert_eq!(value("code_challenge"), pkce["challenge"].as_str().unwrap());
        assert!(is_callback(&network, "https://app.example.com/cb?code=x"));
    }

    #[test]
    fn a_code_is_exchanged_for_tokens() {
        let server = Server::start(|seen| match seen.path() {
            "/token" => Reply::json(
                200,
                &json!({"access_token": "at", "refresh_token": "rt", "expires_in": 120}),
            ),
            _ => Reply::new(400, "{}"),
        });
        let (network, _) = network(json!({}));
        let pkce = call(&network, "generatePKCE", &[json!(43)]).unwrap();
        call(&network, "setAuthCode", &[json!("the code")]).unwrap();
        let tokens = call(
            &network,
            "exchangeCodeWithPKCE",
            &[json!({
                "tokenUrl": format!("{}/token", server.base()),
                "clientId": "client",
                "redirectUri": "https://app.example.com/cb",
                "extraParams": {"device": "desk*top"},
            })],
        )
        .unwrap();
        assert_eq!(
            tokens,
            json!({"success": true, "access_token": "at", "refresh_token": "rt", "expires_in": 120})
        );
        let seen = &server.seen()[0];
        assert_eq!(seen.method, "POST");
        assert_eq!(
            seen.header("content-type"),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(
            String::from_utf8(seen.body.clone()).unwrap(),
            format!(
                "grant_type=authorization_code&client_id=client&code=the+code&code_verifier={}\
                 &redirect_uri=https%3A%2F%2Fapp.example.com%2Fcb&device=desk*top",
                pkce["verifier"].as_str().unwrap()
            )
        );
        let expires = network.auth()["expires_at"].as_i64().unwrap();
        assert!((expires - crate::now_ms() - 120_000).abs() < 5_000);
        assert_eq!(call(&network, "isAuthenticated", &[]), Ok(json!(true)));
        // A refused exchange is an error.
        let refused = call(
            &network,
            "exchangeCodeWithPKCE",
            &[json!({ "tokenUrl": format!("{}/other", server.base()) })],
        );
        assert_eq!(refused, Err("OAuth exchange returned HTTP 400".to_string()));
    }

    #[test]
    fn a_disconnect_during_an_exchange_writes_nothing_back() {
        let (network, kept) = network(json!({}));
        let network = Arc::new(network);
        let during = Arc::clone(&network);
        let server = Server::start(move |_| {
            during.reset();
            Reply::json(200, &json!({"access_token": "late"}))
        });
        let answer = call(
            &network,
            "exchangeCodeWithPKCE",
            &[json!({ "tokenUrl": format!("{}/token", server.base()) })],
        );
        assert_eq!(
            answer,
            Ok(json!({"success": false, "error": "Disconnected"}))
        );
        assert!(network.auth().is_empty());
        assert_eq!(
            kept.lock().unwrap().last().cloned().unwrap(),
            json!({"auth": {}, "cookies": []})
        );
    }
}
