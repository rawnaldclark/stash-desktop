//! Signing in to a provider: OAuth 2 authorization code with PKCE, as a public client (no secret), the way Stash for
//! Android signs in, with its app ids and its redirects. The provider's page is shown in a window of Stash's own, which
//! stops at the redirect and hands it here: the redirect's scheme is never registered with the system.
//!
//! - Dropbox: the official SDK's own flow (`/1/connect`, back to `db-<app key>://1/connect`), which every app built on
//!   it uses without registering a redirect; the code is exchanged with no `redirect_uri`, as the SDK does.
//! - OneDrive: Microsoft's `common` endpoint (personal and work accounts), scopes `Files.Read offline_access
//!   User.Read`, back to the Android app's `com.stash.app://oauth/onedrive`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{ANSWER_CAP, CloudError, Kind, unreached};

const DROPBOX_CONNECT: &str = "https://www.dropbox.com/1/connect";
const DROPBOX_TOKEN: &str = "https://api.dropboxapi.com/oauth2/token";
const MICROSOFT_AUTHORIZE: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const MICROSOFT_TOKEN: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
/// The Android app's redirect, registered for its app id as a "mobile and desktop" one.
pub const ONEDRIVE_REDIRECT: &str = "com.stash.app://oauth/onedrive";
const ONEDRIVE_SCOPES: &str = "Files.Read offline_access User.Read";

/// A sign-in begun: the page to open, and what the redirect is checked and the code exchanged with.
#[derive(Clone, PartialEq, Eq)]
pub struct Begun {
    pub url: String,
    pub verifier: String,
    pub state: String,
}

impl std::fmt::Debug for Begun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Begun(…)")
    }
}

/// `bytes` random bytes, as URL-safe text.
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the system gives random bytes");
    URL_SAFE_NO_PAD.encode(buf)
}

/// The PKCE challenge of `verifier` (S256).
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn query(pairs: &[(&str, &str)]) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        q.append_pair(name, value);
    }
    q.finish()
}

/// Begins a sign-in to `kind` with the build's `key` for it.
pub fn begin(kind: Kind, key: &str) -> Begun {
    let verifier = random_token(48);
    let state = random_token(24);
    let code_challenge = challenge(&verifier);
    let url = match kind {
        Kind::Dropbox => {
            let extra = query(&[
                ("code_challenge", &code_challenge),
                ("code_challenge_method", "S256"),
                ("token_access_type", "offline"),
                ("response_type", "code"),
            ]);
            format!(
                "{DROPBOX_CONNECT}?{}",
                query(&[
                    ("k", key),
                    ("n", "0"),
                    ("state", &state),
                    ("extra_query_params", &extra),
                ])
            )
        }
        Kind::OneDrive => format!(
            "{MICROSOFT_AUTHORIZE}?{}",
            query(&[
                ("client_id", key),
                ("response_type", "code"),
                ("redirect_uri", ONEDRIVE_REDIRECT),
                ("response_mode", "query"),
                ("scope", ONEDRIVE_SCOPES),
                ("code_challenge", &code_challenge),
                ("code_challenge_method", "S256"),
                ("state", &state),
                ("prompt", "select_account"),
            ])
        ),
    };
    Begun {
        url,
        verifier,
        state,
    }
}

/// Where the provider sends the listener back, without its query.
fn redirect_base(kind: Kind, key: &str) -> String {
    match kind {
        Kind::Dropbox => format!("db-{}://1/connect", key.to_ascii_lowercase()),
        Kind::OneDrive => ONEDRIVE_REDIRECT.to_string(),
    }
}

/// `scheme://host/path` of an address, lower case but for the path.
fn base_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(format!(
        "{}://{}{}",
        u.scheme().to_ascii_lowercase(),
        u.host_str().unwrap_or("").to_ascii_lowercase(),
        u.path()
    ))
}

/// Whether the window is about to open the provider's redirect.
pub fn is_redirect(kind: Kind, key: &str, url: &str) -> bool {
    base_of(url).is_some_and(|base| base == redirect_base(kind, key))
}

/// The code the redirect carries, checked against the sign-in's `state`; else why there is none, in the listener's
/// words.
pub fn code_of(kind: Kind, url: &str, state: &str) -> Result<String, String> {
    let u = url::Url::parse(url).map_err(|_| "the sign-in did not come back".to_string())?;
    let param = |name: &str| {
        u.query_pairs()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.into_owned())
    };
    if param("error").is_some() {
        return Err(format!("{} was not connected", kind.name()));
    }
    if param("state").as_deref() != Some(state) {
        return Err("that sign-in was not started here: try again".to_string());
    }
    let code = match kind {
        Kind::Dropbox => (param("oauth_token").as_deref() == Some("oauth2code:"))
            .then(|| param("oauth_token_secret"))
            .flatten(),
        Kind::OneDrive => param("code"),
    };
    code.filter(|c| !c.is_empty() && c.len() <= 4096)
        .ok_or_else(|| format!("{} sent no sign-in code", kind.name()))
}

/// What a token call gives.
#[derive(Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access: String,
    /// A new refresh token, when the provider gave one (Microsoft gives one with each refresh).
    pub refresh: Option<String>,
    pub expires_in: u64,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tokens(expires in {} s)", self.expires_in)
    }
}

fn token_call(
    agent: &ureq::Agent,
    kind: Kind,
    pairs: &[(&str, &str)],
) -> Result<Tokens, CloudError> {
    let address = match kind {
        Kind::Dropbox => DROPBOX_TOKEN,
        Kind::OneDrive => MICROSOFT_TOKEN,
    };
    let mut response = agent
        .post(address)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(query(pairs))
        .map_err(|e| unreached(kind.name(), e))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(ANSWER_CAP)
        .read_to_string()
        .unwrap_or_default();
    // invalid_grant (revoked, run out), invalid_client, interaction_required: nothing a retry mends.
    if status == 400 || status == 401 {
        return Err(CloudError::SignedOut);
    }
    if !(200..300).contains(&status) {
        return Err(super::trouble(status, kind.name()));
    }
    let json: Value = serde_json::from_str(&text).map_err(|_| {
        CloudError::Failed(format!(
            "{}'s sign-in answer could not be read",
            kind.name()
        ))
    })?;
    let access = json
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| CloudError::Failed(format!("{} gave no access token", kind.name())))?;
    Ok(Tokens {
        access: access.to_string(),
        refresh: json
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string),
        expires_in: json
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(3600),
    })
}

/// Exchanges the redirect's code for tokens.
pub fn exchange(
    agent: &ureq::Agent,
    kind: Kind,
    key: &str,
    code: &str,
    verifier: &str,
) -> Result<Tokens, CloudError> {
    let tokens = match kind {
        Kind::Dropbox => token_call(
            agent,
            kind,
            &[
                ("code", code),
                ("grant_type", "authorization_code"),
                ("code_verifier", verifier),
                ("client_id", key),
            ],
        ),
        Kind::OneDrive => token_call(
            agent,
            kind,
            &[
                ("client_id", key),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", ONEDRIVE_REDIRECT),
                ("code_verifier", verifier),
                ("scope", ONEDRIVE_SCOPES),
            ],
        ),
    }
    .map_err(|e| match e {
        CloudError::SignedOut => CloudError::Failed(format!(
            "{} did not take the sign-in: try again",
            kind.name()
        )),
        other => other,
    })?;
    if tokens.refresh.is_none() {
        return Err(CloudError::Failed(format!(
            "{} gave no lasting sign-in: try again",
            kind.name()
        )));
    }
    Ok(tokens)
}

/// A new access token from the refresh token.
pub fn refresh(
    agent: &ureq::Agent,
    kind: Kind,
    key: &str,
    refresh_token: &str,
) -> Result<Tokens, CloudError> {
    match kind {
        Kind::Dropbox => token_call(
            agent,
            kind,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", key),
            ],
        ),
        Kind::OneDrive => token_call(
            agent,
            kind,
            &[
                ("client_id", key),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("scope", ONEDRIVE_SCOPES),
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_the_verifiers_sha256_as_the_rfc_shows() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let a = random_token(48);
        assert_eq!(a.len(), 64);
        assert_ne!(a, random_token(48));
    }

    #[test]
    fn a_dropbox_sign_in_goes_to_its_own_page_and_comes_back_by_the_apps_scheme() {
        let begun = begin(Kind::Dropbox, "abc123");
        let page = url::Url::parse(&begun.url).unwrap();
        assert_eq!(page.host_str(), Some("www.dropbox.com"));
        let param = |name: &str| {
            page.query_pairs()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.into_owned())
        };
        assert_eq!(param("k").as_deref(), Some("abc123"));
        assert_eq!(param("state").as_deref(), Some(begun.state.as_str()));
        let extra = param("extra_query_params").unwrap();
        assert!(extra.contains(&format!("code_challenge={}", challenge(&begun.verifier))));
        assert!(extra.contains("token_access_type=offline"));

        let back = format!(
            "db-abc123://1/connect?oauth_token=oauth2code%3A&oauth_token_secret=THE-CODE&uid=1&state={}",
            begun.state
        );
        assert!(is_redirect(Kind::Dropbox, "ABC123", &back));
        assert!(!is_redirect(
            Kind::Dropbox,
            "abc123",
            "https://www.dropbox.com/1/connect?x"
        ));
        assert!(!is_redirect(Kind::OneDrive, "abc123", &back));
        assert_eq!(
            code_of(Kind::Dropbox, &back, &begun.state).as_deref(),
            Ok("THE-CODE")
        );
        // Another sign-in's redirect is not taken.
        assert!(code_of(Kind::Dropbox, &back, "other").is_err());
    }

    #[test]
    fn a_onedrive_sign_in_comes_back_to_the_android_apps_redirect() {
        let begun = begin(Kind::OneDrive, "0123abcd-0123-4567-89ab-0123456789ab");
        assert!(begun.url.starts_with(MICROSOFT_AUTHORIZE));
        assert!(
            begun
                .url
                .contains("redirect_uri=com.stash.app%3A%2F%2Foauth%2Fonedrive")
        );
        let back = format!(
            "com.stash.app://oauth/onedrive?code=M.C5_x!y*z&state={}",
            begun.state
        );
        assert!(is_redirect(Kind::OneDrive, "x", &back));
        assert_eq!(
            code_of(Kind::OneDrive, &back, &begun.state).as_deref(),
            Ok("M.C5_x!y*z")
        );
        let refused = format!(
            "com.stash.app://oauth/onedrive?error=access_denied&state={}",
            begun.state
        );
        assert_eq!(
            code_of(Kind::OneDrive, &refused, &begun.state),
            Err("OneDrive was not connected".to_string())
        );
    }
}
