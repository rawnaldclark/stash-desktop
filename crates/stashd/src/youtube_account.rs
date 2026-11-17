//! The YouTube account the listener signed in with, as Stash for Android has it: in a window of Google's own sign-in
//! (the desktop shell opens it, and hands back the session's cookies once Google set them), or from cookies pasted in
//! (a browser's `Cookie` header, or a `cookies.txt` file). Either way YouTube is asked who signed in before the login
//! is kept, so a login that does not work is never kept.
//!
//! The login is kept in the library's settings and is never given back to the window: the window is told who signed
//! in, and whether YouTube still takes it.
//!
//! `GET /api/youtube/account`: who, or nobody. `POST /api/youtube/account {"cookies"}`: sign in with these.
//! `POST /api/youtube/account/window`: sign in in Google's window. `POST /api/youtube/account/out`: sign out.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::addons::Code;
use stash_core::library::Library;
use stash_core::youtube::YouTube;
use stash_core::youtube::home::Account;
use stash_core::youtube::login::Login;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, offline, refuse, reply_json};

/// The setting the login is kept in (empty: signed out).
pub(crate) const SETTING: &str = "youtube.login";

/// Handed to the shell when the listener signs in in a window: called once, with the cookies of the session Google
/// set (`Cookie` header form), or with nothing when the window was closed first.
pub type SignedIn = Box<dyn FnOnce(Option<String>) + Send>;

/// Opens a window of Google's sign-in for YouTube (or brings forward the one open) and calls the [`SignedIn`] it is
/// given when it closes. True when there is a window.
pub type SignInShell = Arc<dyn Fn(SignedIn) -> bool + Send + Sync>;

/// The account, for the routes and for the window's sign-in, which ends on a thread of the shell's.
pub(crate) struct TubeAccount {
    library: Arc<Mutex<Library>>,
    youtube: Arc<YouTube>,
    /// Who signed in, once YouTube said.
    known: Mutex<Option<Account>>,
    /// A sign-in window is open.
    signing: AtomicBool,
    /// Why the last sign-in was not kept, in the listener's words.
    trouble: Mutex<Option<String>>,
}

impl TubeAccount {
    /// The account of `youtube`, signed in with the login kept from before, if any.
    pub(crate) fn new(library: Arc<Mutex<Library>>, youtube: Arc<YouTube>) -> Self {
        let kept = library.lock().unwrap().setting(SETTING).ok().flatten();
        if let Some(login) = kept.as_deref().and_then(Login::read) {
            youtube.set_login(Some(login));
        }
        Self {
            library,
            youtube,
            known: Mutex::default(),
            signing: AtomicBool::new(false),
            trouble: Mutex::default(),
        }
    }

    /// Signs in with the cookies in `text`, once YouTube says whose they are; the login before stays otherwise.
    fn take(&self, text: &str) -> Result<Account, (u16, String)> {
        let Some(login) = Login::read(text) else {
            return Err((
                400,
                "That is not a YouTube sign-in: it has no SAPISID cookie. Copy the cookies of music.youtube.com \
                 while signed in there."
                    .to_string(),
            ));
        };
        let before = self.youtube.login();
        self.youtube.set_login(Some(login.clone()));
        match self.youtube.account() {
            Ok(account) => {
                self.library
                    .lock()
                    .unwrap()
                    .set_setting(SETTING, &login.header())
                    .map_err(|e| (500, e.to_string()))?;
                *self.known.lock().unwrap() = Some(account.clone());
                *self.trouble.lock().unwrap() = None;
                Ok(account)
            }
            Err(e) => {
                self.youtube.set_login(before);
                Err(match e.code {
                    Code::Unauthorized => (
                        401,
                        "YouTube did not take that sign-in: the session may have signed out, or some of its \
                         cookies are missing"
                            .to_string(),
                    ),
                    Code::RateLimited => (429, e.in_words()),
                    _ => (502, e.in_words()),
                })
            }
        }
    }

    /// What the shell hands back when a sign-in window closes.
    fn signed_in(self: &Arc<Self>) -> crate::youtube_account::SignedIn {
        let account = Arc::clone(self);
        Box::new(move |cookies: Option<String>| {
            if let Some(cookies) = cookies
                && let Err((_, why)) = account.take(&cookies)
            {
                *account.trouble.lock().unwrap() = Some(why);
            }
            account.signing.store(false, Ordering::Relaxed);
        })
    }

    fn out(&self) -> Result<()> {
        self.youtube.set_login(None);
        self.library.lock().unwrap().set_setting(SETTING, "")?;
        *self.known.lock().unwrap() = None;
        *self.trouble.lock().unwrap() = None;
        Ok(())
    }
}

fn account_json(account: &Account) -> Value {
    json!({ "name": account.name, "handle": account.handle, "photo": account.photo })
}

/// The account as the window sees it (never the login).
fn told(state: &State, account: &TubeAccount) -> Value {
    let signed_in = account.youtube.login().is_some();
    // Who it is, asked once (a login YouTube no longer takes is found out here, too).
    if signed_in
        && !account.youtube.login_spent()
        && account.known.lock().unwrap().is_none()
        && !offline(state)
        && let Ok(who) = account.youtube.account()
    {
        *account.known.lock().unwrap() = Some(who);
    }
    let known = account.known.lock().unwrap().clone();
    json!({
        "signed_in": signed_in,
        "spent": signed_in && account.youtube.login_spent(),
        "signing": account.signing.load(Ordering::Relaxed),
        "window": state.sign_in.is_some(),
        "trouble": *account.trouble.lock().unwrap(),
        "account": known.as_ref().filter(|_| signed_in).map(account_json),
    })
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !path.starts_with("/api/youtube/account") {
        return Ok(None);
    }
    let Some(account) = state
        .youtube_account()
        .filter(|_| state.youtube_on().is_some())
    else {
        return Ok(Some(refuse(404, "YouTube Music is switched off")));
    };
    let reply = match (request.method(), path) {
        (Method::Get, "/api/youtube/account") => reply_json(200, told(state, &account)),
        (Method::Post, "/api/youtube/account") => {
            let cookies = body_json(request)?
                .get("cookies")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if offline(state) {
                return Ok(Some(refuse(
                    409,
                    "Stash is offline: YouTube is asked whose sign-in it is before it is kept",
                )));
            }
            match account.take(&cookies) {
                Ok(_) => reply_json(200, told(state, &account)),
                Err((status, why)) => refuse(status, &why),
            }
        }
        (Method::Post, "/api/youtube/account/window") => {
            let Some(shell) = state.sign_in.as_ref() else {
                return Ok(Some(refuse(
                    404,
                    "Signing in in a window needs the Stash app: paste your cookies instead",
                )));
            };
            if offline(state) {
                return Ok(Some(refuse(409, "Stash is offline: go online to sign in")));
            }
            *account.trouble.lock().unwrap() = None;
            account.signing.store(true, Ordering::Relaxed);
            if !shell(account.signed_in()) {
                account.signing.store(false, Ordering::Relaxed);
                return Ok(Some(refuse(500, "The sign-in window did not open")));
            }
            reply_json(202, told(state, &account))
        }
        (Method::Post, "/api/youtube/account/out") => {
            account.out()?;
            reply_json(200, told(state, &account))
        }
        _ => return Ok(None),
    };
    Ok(Some(reply))
}
