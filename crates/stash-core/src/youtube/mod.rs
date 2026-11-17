//! YouTube Music, built into Stash: the source every listener has before they add any add-on, as in Stash for
//! Android and Stash on the web. It finds songs and says where they play from without anyone signing in, and YouTube
//! is told nothing of the listener but what is searched for and played. A listener who signs in ([`login`]) also has
//! their own: their liked songs, playlists, albums and artists, and the home page YouTube Music makes them; its pages
//! are then asked as them. Searches never are (they would land in the account's history).
//!
//! The rest of the core asks it the way it asks an add-on ([`crate::addons::BuiltIn`]): it is searched, a library
//! song is found in it by the same matching as in an add-on that can only be searched, and its streams go through
//! the core's relay like any other.

pub mod home;
pub mod innertube;
pub mod login;
pub mod pages;
pub mod parse;
mod player;
pub mod ytdlp;

use std::sync::Arc;

use serde_json::json;

use crate::addons::{Addon, AddonError, Artwork, BuiltIn, Code, Manifest, StreamInfo, Tier, Track};
use crate::reach::Reach;
use home::{Account, Card, Shelf};
use innertube::{InnerTube, Signed, WEB_REMIX, trouble};
use login::Login;
use pages::{AlbumPage, ArtistPage, Found, PlaylistPage};
use parse::Song;

/// The id it has among the listener's sources. Songs brought in from Stash for Android carry their YouTube video
/// under this name, so they play from it at once.
pub const ID: &str = "youtube";

/// What a search for songs sends as `params`: YouTube Music's own "Songs" filter.
const SONGS_ONLY: &str = "EgWKAQIIAWoKEAoQAxAJEAQQBQ%3D%3D";
/// The longest search sent.
const QUERY_CAP: usize = 200;
/// How many answers a playlist is read through: a hundred songs each.
const PLAYLIST_PAGES: usize = 10;
/// An answer to "the songs after these" that brings fewer new songs than this is the playlist's last.
const PLAYLIST_FEW_NEW: usize = 5;
/// How many answers the home page is read through (each a few shelves), and a page of the library (each a hundred or
/// so cards).
const HOME_PAGES: usize = 4;
const LIBRARY_PAGES: usize = 10;

/// A page of the listener's library on YouTube Music.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    Playlists,
    Albums,
    Artists,
}

impl Saved {
    fn page(self) -> &'static str {
        match self {
            Saved::Playlists => "FEmusic_liked_playlists",
            Saved::Albums => "FEmusic_liked_albums",
            Saved::Artists => "FEmusic_library_corpus_track_artists",
        }
    }
}

/// YouTube Music as a source.
#[derive(Default)]
pub struct YouTube {
    line: InnerTube,
    players: player::Players,
    /// yt-dlp, for the songs YouTube's apps will not play to Stash (see [`ytdlp`]), when the listener has it.
    fallback: std::sync::RwLock<Option<ytdlp::YtDlp>>,
}

impl YouTube {
    /// YouTube at other addresses than its own: a test's server.
    pub fn at(music: &str, www: &str) -> Self {
        Self {
            line: InnerTube::at(music, www),
            players: player::Players::default(),
            fallback: Default::default(),
        }
    }

    /// Plays what YouTube's apps will not through `fallback` (yt-dlp), or no longer (nothing).
    pub fn set_fallback(&self, fallback: Option<ytdlp::YtDlp>) {
        *self.fallback.write().unwrap() = fallback;
    }

    pub fn fallback(&self) -> Option<ytdlp::YtDlp> {
        self.fallback.read().unwrap().clone()
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

    /// Signs in with `login`, or out (nothing).
    pub fn set_login(&self, login: Option<Login>) {
        self.line.set_login(login);
    }

    /// The login signed in with.
    pub fn login(&self) -> Option<Login> {
        self.line.login()
    }

    /// Whether YouTube stopped taking the login (the listener signed out on the web, or it ran out).
    pub fn login_spent(&self) -> bool {
        self.line.login_spent()
    }

    /// Who signed in: asks YouTube, so it also tells whether it still takes the login.
    pub fn account(&self) -> Result<Account, AddonError> {
        let answer =
            self.line
                .call_as(&WEB_REMIX, "account/accounts_list", json!({}), Signed::Must)?;
        // YouTube answers a session that is not signed in (one signed out on the web, or cookies that are not all
        // there) as it answers anyone: with no account in it.
        home::account(&answer).ok_or_else(|| {
            self.line.spend();
            trouble(
                Code::Unauthorized,
                "YouTube no longer takes your sign-in: sign in again in Settings",
            )
        })
    }

    /// The home page YouTube Music makes: the listener's own when they signed in, else the one it shows anyone.
    pub fn home(&self) -> Result<Vec<Shelf>, AddonError> {
        let answer = self.line.call_as(
            &WEB_REMIX,
            "browse",
            json!({ "browseId": "FEmusic_home" }),
            Signed::IfAny,
        )?;
        let (mut shelves, mut more) = home::shelves(&answer);
        for _ in 1..HOME_PAGES {
            let Some(token) = more.take() else { break };
            // What was read so far is given when the rest cannot be.
            let Ok(answer) = self.line.call_as(
                &WEB_REMIX,
                "browse",
                json!({ "continuation": token }),
                Signed::IfAny,
            ) else {
                break;
            };
            let (next, after) = home::shelves(&answer);
            shelves.extend(next);
            more = after;
        }
        Ok(shelves)
    }

    /// A page of the listener's library: their playlists, the albums they saved, or the artists of their songs.
    pub fn library(&self, which: Saved) -> Result<Vec<Card>, AddonError> {
        let answer = self.line.call_as(
            &WEB_REMIX,
            "browse",
            json!({ "browseId": which.page() }),
            Signed::Must,
        )?;
        let (mut cards, mut more) = home::cards(&answer);
        for _ in 1..LIBRARY_PAGES {
            let Some(token) = more.take() else { break };
            let Ok(answer) = self.line.call_as(
                &WEB_REMIX,
                "browse",
                json!({ "continuation": token }),
                Signed::Must,
            ) else {
                break;
            };
            let (next, after) = home::cards(&answer);
            let before = cards.len();
            for card in next {
                if !cards.contains(&card) {
                    cards.push(card);
                }
            }
            if cards.len() == before {
                break;
            }
            more = after;
        }
        Ok(cards)
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

    /// Everything YouTube Music finds for `query`: the albums, artists and playlists beside the songs, and what it
    /// puts first. (Its songs come without their album and length here: [`YouTube::songs`] has those.)
    pub fn found(&self, query: &str) -> Result<Found, AddonError> {
        let query: String = query.chars().take(QUERY_CAP).collect();
        if query.trim().is_empty() {
            return Ok(Found::default());
        }
        let answer = self
            .line
            .call(&WEB_REMIX, "search", json!({ "query": query }))?;
        Ok(pages::found(&answer))
    }

    /// The page `id` of the kind whose ids start with one of `starts`, as YouTube answers it.
    fn page(&self, id: &str, starts: &[&str]) -> Result<serde_json::Value, AddonError> {
        let known =
            parse::page_id(id).filter(|id| starts.iter().any(|start| id.starts_with(start)));
        let Some(id) = known else {
            return Err(trouble(
                Code::NotFound,
                "that is not a page of YouTube Music",
            ));
        };
        self.line.call_as(
            &WEB_REMIX,
            "browse",
            json!({ "browseId": id }),
            Signed::IfAny,
        )
    }

    /// An album's page ("MPREb_…").
    pub fn album(&self, id: &str) -> Result<AlbumPage, AddonError> {
        let answer = self.page(id, &["MPREb_"])?;
        pages::album(&answer, id)
            .filter(|album| !album.songs.is_empty())
            .ok_or_else(|| trouble(Code::NotFound, "YouTube Music has no such album"))
    }

    /// An artist's page ("UC…").
    pub fn artist(&self, id: &str) -> Result<ArtistPage, AddonError> {
        let answer = self.page(id, &["UC"])?;
        pages::artist(&answer)
            .ok_or_else(|| trouble(Code::NotFound, "YouTube Music has no such artist"))
    }

    /// A playlist's page, by its page ("VL…") or by the playlist's own id ("PL…", "RD…", "OLAK5uy_…"): its songs,
    /// read on through at most [`PLAYLIST_PAGES`] answers. A station (an id that starts with "RD" and is no list
    /// YouTube keeps) never ends: its first answer is all that is read.
    pub fn playlist(&self, id: &str) -> Result<PlaylistPage, AddonError> {
        let page = if id.starts_with("VL") {
            id.to_string()
        } else {
            format!("VL{id}")
        };
        let answer = self.page(&page, &["VLPL", "VLRD", "VLOLAK5uy_", "VLLM"])?;
        let mut playlist = pages::playlist(&answer)
            .ok_or_else(|| trouble(Code::NotFound, "YouTube Music has no such playlist"))?;
        let endless = page.starts_with("VLRD") && !page.starts_with("VLRDCLAK5uy_");
        let mut read = 1;
        while let Some(more) = playlist
            .more
            .take()
            .filter(|_| !endless && read < PLAYLIST_PAGES)
        {
            // What was read so far is given when the rest cannot be.
            let Ok(answer) = self.line.call_as(
                &WEB_REMIX,
                "browse",
                json!({ "continuation": more }),
                Signed::IfAny,
            ) else {
                break;
            };
            let (songs, next) = pages::playlist_more(&answer);
            // Only what was not read yet: a list of YouTube Music's own answers "the songs after these" with the
            // same songs again, without end. An answer that brings nearly nothing new is the last one read.
            let before = playlist.songs.len();
            for song in songs {
                if !playlist.songs.iter().any(|known| known.id == song.id) {
                    playlist.songs.push(song);
                }
            }
            if playlist.songs.len() - before < PLAYLIST_FEW_NEW {
                break;
            }
            playlist.more = next;
            read += 1;
        }
        Ok(playlist)
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
                "Built into Stash. Finds and plays songs from YouTube Music, signed in or not."
                    .to_string(),
            ),
            resources: vec!["search".to_string(), "stream".to_string()],
            dialect: Default::default(),
            resolve_by_isrc: false,
            quality: None,
            no_downloads: false,
            sent: Vec::new(),
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

    /// Where the video plays from: as one of YouTube's apps; else, when that is something yt-dlp could get past (an
    /// age to confirm, no plain address, every app asking whether the listener is a bot), through yt-dlp, signed in as
    /// the listener when they are.
    fn stream(&self, id: &str) -> Result<StreamInfo, AddonError> {
        let failed = match self.players.stream(&self.line, id) {
            Err(e) if ytdlp::worth_trying(&e) => e,
            answered => return answered,
        };
        let Some(fallback) = self.fallback() else {
            return Err(failed);
        };
        let login = self.line.login().filter(|_| !self.line.login_spent());
        fallback.stream(id, login.as_ref())
    }
}
