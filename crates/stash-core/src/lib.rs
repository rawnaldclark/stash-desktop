//! The Stash desktop core: the library database, and later the audio engine, the add-on client and the local API.
//!
//! The core has no UI and no window. Every shell (the Tauri app, a phone remote, a hosted web page) talks to it
//! through the same API, so the shell can be replaced without touching what the listener owns: the library.

pub mod db;
pub mod library;
pub mod synth;

pub use library::{AlbumRow, Counts, Library, NewTrack, Origin, TrackRow};

/// What can go wrong in the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error(
        "the library was written by a newer Stash (schema {found}, this build reads up to {supported})"
    )]
    NewerSchema { found: i64, supported: i64 },
}

pub type Result<T> = std::result::Result<T, Error>;
