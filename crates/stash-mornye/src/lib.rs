//! Mornye extensions (the SpotiFLAC package format, `.sflx`) in Stash: each add-on runs in a QuickJS runtime of its
//! own ([`engine`]) and asks the core for what it may do ([`host`]): reach the addresses its manifest names
//! ([`net`]), keep things ([`store`]), sign requests with a session a person granted ([`session`]), write files in
//! a folder of its own and download into it. [`packages`] installs them from a registry, keeps their settings, and
//! asks them for songs; [`download`] turns one of their songs into a file the player can play.
//!
//! The behaviour follows Lokal (https://github.com/lokal-music/mobile, GPL-3.0), which ships Mornye extensions
//! natively; this is a port of its host to Rust, with Stash's own core around it.

// The hooks the core hands the host are callbacks (`Arc<dyn Fn(…) + Send + Sync>`) wherever they go.
#![allow(clippy::type_complexity)]

pub mod engine;

/// The prelude every add-on runs after (see `prelude.js`).
pub const PRELUDE: &str = include_str!("prelude.js");

/// The Mornye version this host is compatible with: what `utils.appVersion()` says, and the newest `minAppVersion`
/// a package may ask for.
pub const COMPAT_VERSION: &str = "5.3.0";
pub mod auth;
pub mod bytes;
pub mod crypto;
pub mod download;
pub mod ffmpeg;
pub mod host;
pub mod live;
pub mod manifest;
pub mod net;
pub mod packages;
pub mod probe;
pub mod registry;
pub mod session;
pub mod store;
pub mod track;

/// Why asking an add-on failed, in the kinds the core acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trouble {
    /// Words to show: the add-on's own, or the host's.
    Failed(String),
    /// The add-on cannot give this song (the queue moves on).
    Unavailable(String),
    /// Its service is limiting requests: ask again in `seconds`.
    RateLimited { seconds: u64, message: String },
    /// A person must pass its verification page, or sign in, first; `url` is the page when known.
    Verify {
        url: Option<String>,
        message: String,
    },
    /// The add-on is not installed, or is switched off.
    Missing(String),
}

impl Trouble {
    pub fn message(&self) -> &str {
        match self {
            Trouble::Failed(m) | Trouble::Unavailable(m) | Trouble::Missing(m) => m,
            Trouble::RateLimited { message, .. } | Trouble::Verify { message, .. } => message,
        }
    }
}

impl From<String> for Trouble {
    fn from(message: String) -> Self {
        Trouble::Failed(message)
    }
}

/// Milliseconds since 1970.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `n` random bytes.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).expect("the system gives random bytes");
    bytes
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
