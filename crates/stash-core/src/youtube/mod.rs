//! YouTube Music, built into Stash: the source every listener has before they add any add-on, as in Stash for
//! Android and Stash on the web. It finds songs and says where they play from; nobody signs in, and YouTube is told
//! nothing of the listener but what is searched for and played.
//!
//! The rest of the core asks it the way it asks an add-on ([`crate::addons::BuiltIn`]): it is searched, a library
//! song is found in it by the same matching as in an add-on that can only be searched, and its streams go through
//! the core's relay like any other.

pub mod innertube;
pub mod parse;
mod player;

use std::sync::Arc;

use serde_json::json;

use crate::addons::{Addon, AddonError, Artwork, BuiltIn, Manifest, StreamInfo, Tier, Track};
use crate::reach::Reach;
use innertube::{InnerTube, WEB_REMIX};
use parse::Song;

/// The id it has among the listener's sources. Songs brought in from Stash for Android carry their YouTube video
/// under this name, so they play from it at once.
pub const ID: &str = "youtube";

/// What a search for songs sends as `params`: YouTube Music's own "Songs" filter.
const SONGS_ONLY: &str = "EgWKAQIIAWoKEAoQAxAJEAQQBQ%3D%3D";
/// The longest search sent.
const QUERY_CAP: usize = 200;

/// YouTube Music as a source.
#[derive(Default)]
pub struct YouTube {
    line: InnerTube,
    players: player::Players,
}

impl YouTube {
    /// YouTube at other addresses than its own: a test's server.
    pub fn at(music: &str, www: &str) -> Self {
        Self {
            line: InnerTube::at(music, www),
            players: player::Players::default(),
        }
    }

    /// The visitor id YouTube gave, to keep for the next start; and one kept from before, taken up again.
    pub fn visitor(&self) -> Option<String> {
        self.line.visitor()
    }

    pub fn set_visitor(&self, visitor: &str) {
        self.line.set_visitor(visitor);
    }

    /// Has `learned` told the visitor id when YouTube first gives one.
    pub fn on_visitor(&self, learned: innertube::Learned) {
        self.line.on_visitor(learned);
    }

    /// The songs YouTube Music finds for `query`, in its order.
    pub fn songs(&self, query: &str) -> Result<Vec<Song>, AddonError> {
        let query: String = query.chars().take(QUERY_CAP).collect();
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let answer = self.line.call(
            &WEB_REMIX,
            "search",
            json!({ "query": query, "params": SONGS_ONLY }),
        )?;
        Ok(parse::songs(&answer))
    }

    /// The manifest it is listed with among the listener's sources.
    pub fn manifest() -> Manifest {
        Manifest {
            stash: 1,
            id: ID.to_string(),
            name: "YouTube Music".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            kind: "server".to_string(),
            description: Some(
                "Built into Stash. Finds and plays songs from YouTube Music, without signing in."
                    .to_string(),
            ),
            resources: vec!["search".to_string(), "stream".to_string()],
            dialect: Default::default(),
            resolve_by_isrc: false,
            quality: None,
            no_downloads: false,
            reach: Some(Reach::Internet),
        }
    }

    /// As one of the listener's sources. `reach` is the internet, but for a test's server on this computer.
    pub fn source(self: &Arc<Self>, reach: Reach) -> Addon {
        let manifest = Manifest {
            reach: Some(reach),
            ..Self::manifest()
        };
        Addon::built_in(manifest, Arc::clone(self) as Arc<dyn BuiltIn>, reach)
            .expect("the built-in manifest is well-formed")
    }
}

/// A row of YouTube Music as a track of the source; nothing for a row that names no artist (the rest of the core
/// knows a song by its title and its artist).
fn track(song: Song) -> Option<Track> {
    if song.artist.is_empty() {
        return None;
    }
    Some(Track {
        id: song.id,
        title: song.title,
        artist: song.artist,
        album: song.album,
        duration_ms: song.duration_ms,
        isrc: None,
        explicit: song.explicit.then_some(true),
        max_tier: Some(Tier::Low),
        artwork: song.art.into_iter().map(|url| Artwork { url }).collect(),
    })
}

impl BuiltIn for YouTube {
    fn search(&self, query: &str, limit: u32) -> Result<Vec<Track>, AddonError> {
        Ok(self
            .songs(query)?
            .into_iter()
            .filter_map(track)
            .take(limit as usize)
            .collect())
    }

    fn stream(&self, id: &str) -> Result<StreamInfo, AddonError> {
        self.players.stream(&self.line, id)
    }
}
