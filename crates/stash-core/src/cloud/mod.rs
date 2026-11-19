//! Music in the listener's own cloud storage: Dropbox and OneDrive, as Stash for Android and the web player have it.
//!
//! The listener connects an account (signing in on the provider's own page, [`auth`]) and picks folders of music.
//! Each folder is scanned ([`scan`]): every audio file in it becomes a song of the library, known by its tags, read
//! from the file's first and last bytes without downloading it ([`tags`]). A song a file brings is a song like any
//! other (liked, put in playlists, saved to this computer); it plays from the file itself, through a link the provider
//! gives for a while. What sources call a song keeps the file: `dropbox` or `onedrive`, with the provider's id for the
//! file (the same ids the other Stash apps keep). A song the library has already gains the file, and plays from it.
//!
//! Read-only: nothing here ever writes to the listener's storage.

pub mod auth;
pub mod dropbox;
pub mod onedrive;
pub mod scan;
pub mod store;
pub mod tags;

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

/// A cloud storage service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Dropbox,
    OneDrive,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Dropbox, Kind::OneDrive];

    /// Its id: what sources call a song from it, and how the library keeps its accounts.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Dropbox => "dropbox",
            Kind::OneDrive => "onedrive",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Dropbox => "Dropbox",
            Kind::OneDrive => "OneDrive",
        }
    }

    pub fn from_id(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.id() == id)
    }
}

/// This build's ids for each provider: a Dropbox app key and a Microsoft app (client) id. Public-client ids, not
/// secrets (PKCE, no secret), the Android app's own; given at build time (`STASH_DROPBOX_KEY`, `STASH_ONEDRIVE_ID`), or
/// by the environment. A provider without one is not offered.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Keys {
    pub dropbox: Option<String>,
    pub onedrive: Option<String>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Keys(dropbox: {}, onedrive: {})",
            self.dropbox.is_some(),
            self.onedrive.is_some()
        )
    }
}

impl Keys {
    /// The environment's ids, else the build's.
    pub fn found() -> Keys {
        let env = |name: &str| std::env::var(name).ok();
        Keys {
            dropbox: dropbox_key(env("STASH_DROPBOX_KEY").as_deref())
                .or_else(|| dropbox_key(option_env!("STASH_DROPBOX_KEY"))),
            onedrive: onedrive_id(env("STASH_ONEDRIVE_ID").as_deref())
                .or_else(|| onedrive_id(option_env!("STASH_ONEDRIVE_ID"))),
        }
    }

    pub fn of(&self, kind: Kind) -> Option<&str> {
        match kind {
            Kind::Dropbox => self.dropbox.as_deref(),
            Kind::OneDrive => self.onedrive.as_deref(),
        }
    }
}

/// A Dropbox app key: letters and digits (it goes into an address).
fn dropbox_key(given: Option<&str>) -> Option<String> {
    let key = given.map(str::trim).filter(|k| !k.is_empty())?;
    (key.len() <= 64 && key.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| key.to_ascii_lowercase())
}

/// A Microsoft app id: a GUID.
fn onedrive_id(given: Option<&str>) -> Option<String> {
    let id = given
        .map(str::trim)
        .filter(|k| !k.is_empty())?
        .to_ascii_lowercase();
    let parts: Vec<&str> = id.split('-').collect();
    let shape = [8, 4, 4, 4, 12];
    (parts.len() == 5
        && parts
            .iter()
            .zip(shape)
            .all(|(part, len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit())))
    .then_some(id)
}

/// A folder or file as the listener browses their storage. `path` is where it is, for showing ("/Music/Rips").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub folder: bool,
    pub path: String,
}

/// A picked folder as a provider is asked for it: its id, and where it was when it was picked (lower case), so a file's
/// folders below it can be read from the file's own path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub id: String,
    pub path_lower: String,
}

/// A file a page of changes names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct File {
    pub id: String,
    pub name: String,
    pub size: u64,
    /// What changes when the file does (Dropbox's rev, OneDrive's cTag).
    pub version: Option<String>,
    /// The folder it is in, by id, when the page says only that (OneDrive).
    pub parent: Option<String>,
    /// The folders from just below the picked one down to the file's own, when the page says (Dropbox).
    pub folders: Option<Vec<String>>,
    /// Its whole path, lower case, for a provider that names what was removed by path (Dropbox).
    pub path_lower: Option<String>,
    /// A link to its bytes the page gave already (OneDrive), saving a request.
    pub link: Option<String>,
}

/// One change a page names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    File(File),
    Folder {
        id: String,
        name: String,
        parent: Option<String>,
    },
    /// Something removed: by id (OneDrive), or by its lower-case path, maybe a whole folder (Dropbox).
    Removed {
        id: Option<String>,
        path_lower: Option<String>,
    },
}

/// A page of changes, and where the next one starts: the next page while `more`, else the mark to ask what changed
/// since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub changes: Vec<Change>,
    pub cursor: String,
    pub more: bool,
}

/// A link to a file's bytes that takes a range, good for `good_for`. Its address is the credential: it is never
/// shown or written down.
#[derive(Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub good_for: Duration,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Link(good for {:?})", self.good_for)
    }
}

/// Why a provider gave no answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudError {
    /// The sign-in is no longer taken (revoked, run out): the listener signs in again.
    #[error("sign in again")]
    SignedOut,
    /// The provider no longer knows the mark to ask what changed since: the folder is read whole again.
    #[error("the folder has to be read again")]
    CursorGone,
    /// The file or folder is not there (any more).
    #[error("it is not there any more")]
    Gone,
    /// The provider is busy, or could not be reached: asking later may help.
    #[error("{0}")]
    Busy(String),
    #[error("{0}")]
    Failed(String),
}

/// An access token for the account, fresh (`true`) when the one given before was refused.
pub type TokenFn = Arc<dyn Fn(bool) -> Result<String, CloudError> + Send + Sync>;

/// Who signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
}

/// One provider, as the rest of Stash asks it.
pub trait Drive: Send + Sync {
    fn kind(&self) -> Kind;
    fn me(&self) -> Result<Me, CloudError>;
    /// The folders and files directly inside `folder` (none: the top of the storage).
    fn browse(&self, folder: Option<&str>) -> Result<Vec<Entry>, CloudError>;
    /// Where `folder` is ("/Music"), lower case, for [`Root`].
    fn path_of(&self, folder: &str) -> Result<String, CloudError>;
    /// A page of the changes under `root`: from the start (every file, all the way down) with no `cursor`.
    fn page(&self, root: &Root, cursor: Option<&str>) -> Result<Page, CloudError>;
    fn link(&self, item: &str) -> Result<Link, CloudError>;
}

/// The agent the providers are asked through: the internet only, https only, answers of any status read.
pub fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .https_only(true)
        .max_redirects(5)
        .proxy(None)
        .build();
    crate::reach::Guarded::agent(crate::reach::Reach::Internet, config)
}

/// The largest answer read from a provider's API.
pub(crate) const ANSWER_CAP: u64 = 16 * 1024 * 1024;

/// How a status a provider answered reads, when it is not success: busy, signed out, or not there.
pub(crate) fn trouble(status: u16, what: &str) -> CloudError {
    match status {
        401 => CloudError::SignedOut,
        404 => CloudError::Gone,
        408 | 429 | 500..=599 => CloudError::Busy(format!("{what} is busy (it answered {status})")),
        _ => CloudError::Failed(format!("{what} answered {status}")),
    }
}

/// A request that did not get an answer: the connection is the trouble.
pub(crate) fn unreached(what: &str, e: impl std::fmt::Display) -> CloudError {
    CloudError::Busy(format!("{what} could not be reached ({e})"))
}

/// The file name endings a scan takes as music.
pub fn is_audio(name: &str) -> bool {
    crate::scan::is_audio(std::path::Path::new(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_well_formed_ids_are_taken() {
        assert_eq!(dropbox_key(Some(" AbC123 ")).as_deref(), Some("abc123"));
        assert_eq!(dropbox_key(Some("a b")), None);
        assert_eq!(dropbox_key(Some("")), None);
        assert_eq!(
            onedrive_id(Some("0123ABCD-0123-4567-89ab-0123456789AB")).as_deref(),
            Some("0123abcd-0123-4567-89ab-0123456789ab")
        );
        assert_eq!(onedrive_id(Some("0123abcd-0123-4567-89ab")), None);
        assert_eq!(Kind::from_id("onedrive"), Some(Kind::OneDrive));
        assert_eq!(Kind::from_id("gdrive"), None);
    }

    #[test]
    fn music_files_are_known_by_their_ending() {
        assert!(is_audio("01 Song.FLAC"));
        assert!(is_audio("a.m4a"));
        assert!(!is_audio("cover.jpg"));
        assert!(!is_audio("notes"));
    }
}
