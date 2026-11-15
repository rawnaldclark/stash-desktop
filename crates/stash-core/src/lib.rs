//! The Stash desktop core: the library database, the audio engine and its player, and the add-on client.
//!
//! The core has no UI and no window. Every shell (the Tauri app, a phone remote, a hosted web page) talks to it
//! through the same API, so the shell can be replaced without touching what the listener owns: the library.

pub mod addons;
pub mod audio;
pub mod covers;
pub mod db;
pub mod eclipse;
pub mod identity;
pub mod import;
pub mod library;
pub mod lists;
pub mod matching;
pub mod player;
pub mod scan;
pub mod synth;

pub use library::{
    AlbumRow, ArtistRow, Counts, Described, FileState, Library, NewTrack, Origin, PlaylistRow,
    ScanChange, StoredAddon, TrackRow,
};

/// What can go wrong in the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("audio engine: {0}")]
    Audio(String),
    #[error("scan: {0}")]
    Scan(String),
    /// Something the listener asked for that cannot be done, in words for the listener.
    #[error("{0}")]
    Refused(String),
    #[error(
        "the library was written by a newer Stash (schema {found}, this build reads up to {supported})"
    )]
    NewerSchema { found: i64, supported: i64 },
}

pub type Result<T> = std::result::Result<T, Error>;
