//! Reading YouTube Music's pages: everything a search finds (albums, artists and playlists beside the songs), and
//! the pages of an album, an artist and a playlist.
//!
//! As with the rows of a search ([`super::parse`]), a part is read by what it links to (a page's kind) before what
//! it says, and what cannot be read is left out. A shelf's title is never relied on: YouTube renames them.

use serde_json::Value;

use super::parse::{
    GROUPS, Song, YEAR, column, explicit, kind, names_artist, page_id, picture, row, run_page,
    run_text, second_line, shelves, text, video_id,
};

/// An album as a card or a row names it.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumCard {
    /// Its page ("MPREb_…").
    pub id: String,
    pub title: String,
    /// Empty when the card does not say (an artist's own page).
    pub artist: String,
    pub artist_id: Option<String>,
    pub year: Option<u32>,
    /// A single or an EP, as YouTube labels them.
    pub single: bool,
    pub art: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtistCard {
    /// Its page ("UC…").
    pub id: String,
    pub name: String,
    pub art: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistCard {
    /// Its page ("VL…").
    pub id: String,
    pub title: String,
    /// Who made it; empty when the card does not say.
    pub author: String,
    pub art: Option<String>,
}

/// What a search puts first: the artist or the album whose name was typed, or a song.
#[derive(Debug, Clone, PartialEq)]
pub enum Top {
    Artist(ArtistCard),
    Album(AlbumCard),
    Song(Song),
}

/// Everything a search found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub top: Option<Top>,
    pub songs: Vec<Song>,
    pub albums: Vec<AlbumCard>,
    pub artists: Vec<ArtistCard>,
    pub playlists: Vec<PlaylistCard>,
}

/// The kind of page a link opens ("ALBUM", "ARTIST", "PLAYLIST"), and the page.
fn link(holder: &Value) -> Option<(&str, String)> {
    let browse = holder.pointer("/navigationEndpoint/browseEndpoint")?;
    let kind = browse
        .pointer(
            "/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType",
        )?
        .as_str()?
        .strip_prefix("MUSIC_PAGE_TYPE_")?;
    Some((kind, page_id(browse.get("browseId")?.as_str()?)?))
}

/// The groups of a line of runs, split at its " • ": each group's runs.
fn groups(runs: Option<&Vec<Value>>) -> Vec<Vec<&Value>> {
    let mut groups: Vec<Vec<&Value>> = vec![Vec::new()];
    for run in runs.into_iter().flatten() {
        if run_text(run) == GROUPS {
            groups.push(Vec::new());
        } else if let Some(last) = groups.last_mut() {
            last.push(run);
        }
    }
    groups
}

fn group_text(group: &[&Value]) -> String {
    group.iter().map(|run| run_text(run)).collect::<String>()
}

/// What the line under an album's title says: whether it is a single, its artist and its year. The line reads
/// "Album • Daft Punk • 2013" in a search, "Single • 2023" or "2023" on an artist's page.
fn album_line(runs: Option<&Vec<Value>>) -> (bool, String, Option<String>, Option<u32>) {
    let mut groups = groups(runs);
    let label = groups.first().map(|g| group_text(g)).unwrap_or_default();
    let single = matches!(label.as_str(), "Single" | "EP");
    if single || matches!(label.as_str(), "Album" | "Mixtape" | "Compilation") {
        groups.remove(0);
    }
    let year = groups
        .iter()
        .map(|g| group_text(g))
        .find(|said| YEAR.is_match(said))
        .and_then(|year| year.parse().ok());
    // The artists: the group that links to one; else the first group that is not the year.
    let named = groups
        .iter()
        .find(|g| g.iter().any(|run| names_artist(run)))
        .or_else(|| {
            groups
                .iter()
                .find(|g| !g.is_empty() && !YEAR.is_match(&group_text(g)))
        });
    let artist = named
        .and_then(|g| text(Some(&Value::from(group_text(g)))))
        .unwrap_or_default();
    let artist_id = named
        .into_iter()
        .flatten()
        .filter_map(|run| run_page(run))
        .find(|page| page.starts_with("UC") || page.starts_with("MPLAUC"))
        .and_then(page_id);
    (single, artist, artist_id, year)
}

/// Who made a playlist, from the line under its title: "Playlist • NAFA • 11K views", or without the first word.
fn playlist_line(runs: Option<&Vec<Value>>) -> String {
    let mut groups = groups(runs);
    if groups.first().is_some_and(|g| group_text(g) == "Playlist") {
        groups.remove(0);
    }
    groups
        .first()
        .and_then(|g| text(Some(&Value::from(group_text(g)))))
        .unwrap_or_default()
}

/// A row of a list (`musicResponsiveListItemRenderer`) that opens a page: an album, an artist or a playlist.
pub(super) enum Opens {
    Album(AlbumCard),
    Artist(ArtistCard),
    Playlist(PlaylistCard),
}

pub(super) fn opening_row(item: &Value) -> Option<Opens> {
    let (kind, id) = link(item)?;
    let title = text(column(item, 0)?.first()?.get("text"))?;
    let art = picture(item, "thumbnail");
    let line = column(item, 1);
    Some(match kind {
        "ALBUM" => {
            let (single, artist, artist_id, year) = album_line(line);
            Opens::Album(AlbumCard {
                id,
                title,
                artist,
                artist_id,
                year,
                single,
                art,
            })
        }
        // An artist of the listener's library opens as the artist (its id, "MPLAUC…", is the artist's with "MPLA" before it).
        "ARTIST" | "LIBRARY_ARTIST" => Opens::Artist(ArtistCard {
            id,
            name: title,
            art,
        }),
        "PLAYLIST" => Opens::Playlist(PlaylistCard {
            id,
            title,
            author: playlist_line(line),
            art,
        }),
        _ => return None,
    })
}

/// A card of a carousel (`musicTwoRowItemRenderer`) that opens a page.
pub(super) fn opening_card(card: &Value) -> Option<Opens> {
    let (kind, id) = link(card)?;
    let title = text(card.pointer("/title/runs/0/text"))?;
    let art = picture(card, "thumbnailRenderer");
    let line = card.pointer("/subtitle/runs").and_then(Value::as_array);
    Some(match kind {
        "ALBUM" => {
            let (single, artist, artist_id, year) = album_line(line);
            Opens::Album(AlbumCard {
                id,
                title,
                artist,
                artist_id,
                year,
                single,
                art,
            })
        }
        // An artist of the listener's library opens as the artist (its id, "MPLAUC…", is the artist's with "MPLA" before it).
        "ARTIST" | "LIBRARY_ARTIST" => Opens::Artist(ArtistCard {
            id,
            name: title,
            art,
        }),
        "PLAYLIST" => Opens::Playlist(PlaylistCard {
            id,
            title,
            author: playlist_line(line),
            art,
        }),
        _ => return None,
    })
}

/// Whether a row is an episode of a podcast: its second line says so, or links to the podcast.
pub(super) fn episode(item: &Value) -> bool {
    let line = column(item, 1);
    let says = line
        .and_then(|runs| runs.first())
        .is_some_and(|first| run_text(first) == "Episode");
    says || line
        .into_iter()
        .flatten()
        .any(|run| run_page(run).is_some_and(|page| page.starts_with("MPSPP")))
}

/// The card a search puts first (`musicCardShelfRenderer`).
fn top(card: &Value) -> Option<Top> {
    let first = card.pointer("/title/runs/0")?;
    let title = text(first.get("text"))?;
    let art = picture(card, "thumbnail");
    let line = card.pointer("/subtitle/runs").and_then(Value::as_array);
    if let Some((kind, id)) = link(first) {
        return match kind {
            "ARTIST" => Some(Top::Artist(ArtistCard {
                id,
                name: title,
                art,
            })),
            "ALBUM" => {
                let (single, artist, artist_id, year) = album_line(line);
                Some(Top::Album(AlbumCard {
                    id,
                    title,
                    artist,
                    artist_id,
                    year,
                    single,
                    art,
                }))
            }
            _ => None,
        };
    }
    let watch = first.pointer("/navigationEndpoint/watchEndpoint")?;
    let id = video_id(watch.get("videoId")?.as_str()?)?;
    let line = second_line(line);
    Some(Top::Song(Song {
        id: id.to_string(),
        title,
        artist: line.artist,
        album: line.album,
        duration_ms: line.length,
        art,
        explicit: explicit(card),
        kind: kind(Some(watch)),
        artist_id: line.artist_id,
        album_id: line.album_id,
    }))
}

/// Everything a search's answer holds: the card it puts first, and its rows by what each is. Episodes of podcasts,
/// podcasts and people's channels are left out.
pub fn found(answer: &Value) -> Found {
    let mut found = Found::default();
    for shelf in shelves(answer) {
        let in_card = shelf.get("musicCardShelfRenderer").is_some();
        if let Some(card) = shelf.get("musicCardShelfRenderer") {
            found.top = found.top.or_else(|| top(card));
        }
        let rows = [
            "musicCardShelfRenderer",
            "musicShelfRenderer",
            "itemSectionRenderer",
        ]
        .iter()
        .find_map(|name| shelf.get(name)?.get("contents")?.as_array());
        for item in rows
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("musicResponsiveListItemRenderer"))
        {
            match opening_row(item) {
                Some(Opens::Album(album)) => {
                    if !found.albums.iter().any(|known| known.id == album.id) {
                        found.albums.push(album);
                    }
                }
                Some(Opens::Artist(artist)) => {
                    if !found.artists.iter().any(|known| known.id == artist.id) {
                        found.artists.push(artist);
                    }
                }
                Some(Opens::Playlist(playlist)) => {
                    if !found.playlists.iter().any(|known| known.id == playlist.id) {
                        found.playlists.push(playlist);
                    }
                }
                None => {
                    if episode(item) {
                        continue;
                    }
                    if let Some(mut song) = row(item)
                        && !found.songs.iter().any(|known| known.id == song.id)
                    {
                        // The songs listed on an artist's card are that artist's, and do not say so.
                        if song.artist.is_empty()
                            && in_card
                            && let Some(Top::Artist(artist)) = &found.top
                        {
                            song.artist = artist.name.clone();
                            song.artist_id = Some(artist.id.clone());
                        }
                        found.songs.push(song);
                    }
                }
            }
        }
    }
    found
}

/// The header of an album's or a playlist's page.
fn header(answer: &Value) -> Option<&Value> {
    answer
        .pointer("/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicResponsiveHeaderRenderer")
        .or_else(|| answer.pointer("/header/musicDetailHeaderRenderer"))
        .or_else(|| answer.pointer("/header/musicResponsiveHeaderRenderer"))
}

/// The sections under a page's header: beside it (two columns) or below it (one).
fn sections(answer: &Value) -> impl Iterator<Item = &Value> {
    answer
        .pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents")
        .or_else(|| {
            answer.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        })
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// An album's page.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumPage {
    pub title: String,
    pub artist: String,
    pub artist_id: Option<String>,
    pub year: Option<u32>,
    pub single: bool,
    pub art: Option<String>,
    pub songs: Vec<Song>,
}

/// An album's page (`browse` of "MPREb_…"); nothing when the answer is no album. Its rows name no artist and no
/// album of their own when they are the album's: they are given the album's.
pub fn album(answer: &Value, id: &str) -> Option<AlbumPage> {
    let head = header(answer)?;
    let title = text(head.pointer("/title/runs/0/text"))?;
    let line = head.pointer("/subtitle/runs").and_then(Value::as_array);
    let (single, mut artist, mut artist_id, year) = album_line(line);
    // The artist is on a line of its own on the page as it is now.
    if let Some(strap) = head
        .pointer("/straplineTextOne/runs")
        .and_then(Value::as_array)
    {
        let named = second_line(Some(strap));
        if !named.artist.is_empty() {
            (artist, artist_id) = (named.artist, named.artist_id);
        }
    }
    let art = picture(head, "thumbnail");
    let mut songs: Vec<Song> = Vec::new();
    for item in sections(answer)
        .filter_map(|section| section.pointer("/musicShelfRenderer/contents")?.as_array())
        .flatten()
        .filter_map(|item| item.get("musicResponsiveListItemRenderer"))
    {
        let Some(mut song) = row(item) else {
            continue;
        };
        if song.artist.is_empty() {
            song.artist = artist.clone();
            song.artist_id = artist_id.clone();
        }
        song.album = Some(title.clone());
        song.album_id = page_id(id);
        song.art = song.art.or_else(|| art.clone());
        // As YouTube lists them: two tracks of an album may be one video.
        songs.push(song);
    }
    Some(AlbumPage {
        title,
        artist,
        artist_id,
        year,
        single,
        art,
        songs,
    })
}

/// An artist's page.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtistPage {
    pub name: String,
    pub art: Option<String>,
    /// Their most played songs.
    pub songs: Vec<Song>,
    /// Their albums, then their singles and EPs, each in YouTube's order (the newest first).
    pub albums: Vec<AlbumCard>,
    pub singles: Vec<AlbumCard>,
    /// Artists their listeners also play.
    pub related: Vec<ArtistCard>,
    /// What "More" beside their albums, and beside their singles and EPs, opens: all of them (see [`discography`]).
    /// None when the page shows them all.
    pub albums_more: Option<More>,
    pub singles_more: Option<More>,
    /// A playlist of all their songs (the songs' "Show all"): its page ("VL…").
    pub songs_all: Option<String>,
}

/// A list an artist's page opens from a shelf's "More": its page ("MPAD…") and what picks the shelf (`params`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct More {
    pub id: String,
    pub params: String,
}

/// What a shelf's "More" opens, when it is a list of the artist's records.
fn more_of(shelf: &Value) -> Option<More> {
    let link = shelf.pointer(
        "/header/musicCarouselShelfBasicHeaderRenderer/moreContentButton/buttonRenderer/navigationEndpoint/browseEndpoint",
    )?;
    let id = link.get("browseId")?.as_str()?;
    let params = link.get("params")?.as_str()?;
    (id.starts_with("MPAD") && !params.is_empty()).then(|| More {
        id: id.to_string(),
        params: params.to_string(),
    })
}

/// An artist's page (`browse` of "UC…"); nothing when the answer is no artist's.
pub fn artist(answer: &Value) -> Option<ArtistPage> {
    let head = [
        "musicImmersiveHeaderRenderer",
        "musicVisualHeaderRenderer",
        "musicHeaderRenderer",
    ]
    .iter()
    .find_map(|name| answer.get("header")?.get(name))?;
    let name = text(head.pointer("/title/runs/0/text"))?;
    let mut page = ArtistPage {
        art: picture(head, "thumbnail").or_else(|| picture(head, "foregroundThumbnail")),
        name,
        songs: Vec::new(),
        albums: Vec::new(),
        singles: Vec::new(),
        related: Vec::new(),
        albums_more: None,
        singles_more: None,
        songs_all: None,
    };
    for section in sections(answer) {
        // The first list of rows is their songs.
        if let Some(rows) = section
            .pointer("/musicShelfRenderer/contents")
            .and_then(Value::as_array)
            && page.songs.is_empty()
        {
            page.songs_all = section
                .pointer("/musicShelfRenderer/bottomEndpoint/browseEndpoint/browseId")
                .and_then(Value::as_str)
                .filter(|id| id.starts_with("VL"))
                .map(str::to_string);
            for mut song in rows
                .iter()
                .filter_map(|item| row(item.get("musicResponsiveListItemRenderer")?))
            {
                if song.artist.is_empty() {
                    song.artist = page.name.clone();
                }
                page.songs.push(song);
            }
        }
        let cards = section
            .pointer("/musicCarouselShelfRenderer/contents")
            .and_then(Value::as_array);
        // A shelf of records says by its cards whether its "More" opens the albums or the singles.
        let mut records = None;
        for card in cards
            .into_iter()
            .flatten()
            .filter_map(|card| opening_card(card.get("musicTwoRowItemRenderer")?))
        {
            match card {
                Opens::Album(mut album) => {
                    records.get_or_insert(album.single);
                    // A card on the artist's own page does not say whose it is.
                    if album.artist.is_empty() {
                        album.artist = page.name.clone();
                    }
                    let list = if album.single {
                        &mut page.singles
                    } else {
                        &mut page.albums
                    };
                    if !list.iter().any(|known| known.id == album.id) {
                        list.push(album);
                    }
                }
                Opens::Artist(artist) => {
                    if !page.related.iter().any(|known| known.id == artist.id) {
                        page.related.push(artist);
                    }
                }
                Opens::Playlist(_) => {}
            }
        }
        if let (Some(single), Some(more)) = (
            records,
            section.get("musicCarouselShelfRenderer").and_then(more_of),
        ) {
            let place = if single {
                &mut page.singles_more
            } else {
                &mut page.albums_more
            };
            place.get_or_insert(more);
        }
    }
    Some(page)
}

/// One answer of an artist's discography: their name, the records it lists, and what to ask for the ones after.
#[derive(Debug, Clone, PartialEq)]
pub struct Discography {
    pub artist: String,
    pub albums: Vec<AlbumCard>,
    pub more: Option<String>,
    /// Whether these are all of them: nothing named after them, and none left unread.
    pub whole: bool,
}

/// The cards of a grid's items, each as the artist's (a card here does not say whose it is).
fn grid_cards(items: &[Value], artist: &str) -> Vec<AlbumCard> {
    let mut out: Vec<AlbumCard> = Vec::new();
    for card in items
        .iter()
        .filter_map(|item| opening_card(item.get("musicTwoRowItemRenderer")?))
    {
        if let Opens::Album(mut album) = card {
            if album.artist.is_empty() {
                album.artist = artist.to_string();
            }
            if !out.iter().any(|known| known.id == album.id) {
                out.push(album);
            }
        }
    }
    out
}

/// What a grid or a continuation names for the items after these.
fn grid_more(grid: &Value, items: &[Value]) -> Option<String> {
    grid.pointer("/continuations/0/nextContinuationData/continuation")
        .or_else(|| {
            items
                .last()?
                .pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
        })
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The list an artist page's "More" opens (`browse` of "MPAD…" with its `params`): all their albums, or all their
/// singles and EPs. Nothing when the answer holds no such list.
pub fn discography(answer: &Value) -> Option<Discography> {
    let artist =
        text(answer.pointer("/header/musicHeaderRenderer/title/runs/0/text")).unwrap_or_default();
    let grid = sections(answer).find_map(|section| section.get("gridRenderer"))?;
    let items = grid.get("items").and_then(Value::as_array)?;
    let more = grid_more(grid, items);
    Some(Discography {
        albums: grid_cards(items, &artist),
        whole: more.is_none(),
        more,
        artist,
    })
}

/// The records after these (`browse` of a [`Discography::more`]), and what to ask for the ones after them.
pub fn discography_more(answer: &Value, artist: &str) -> (Vec<AlbumCard>, Option<String>) {
    let appended = answer
        .get("onResponseReceivedActions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|action| {
            action
                .pointer("/appendContinuationItemsAction/continuationItems")?
                .as_array()
        });
    if let Some(items) = appended {
        return (grid_cards(items, artist), grid_more(&Value::Null, items));
    }
    let Some(grid) = answer.pointer("/continuationContents/gridContinuation") else {
        return (Vec::new(), None);
    };
    let items = grid
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    (grid_cards(items, artist), grid_more(grid, items))
}

/// A playlist's page, or the part of it one answer holds.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistPage {
    pub title: String,
    pub author: String,
    pub art: Option<String>,
    pub songs: Vec<Song>,
    /// What to ask for the songs after these, when there are more.
    pub more: Option<String>,
}

/// The songs among `items`, and the word that asks for the ones after them.
fn playlist_rows(items: &[Value]) -> (Vec<Song>, Option<String>) {
    let songs = items
        .iter()
        .filter_map(|item| row(item.get("musicResponsiveListItemRenderer")?))
        .collect();
    let more = items
        .iter()
        .find_map(|item| {
            item.pointer(
                "/continuationItemRenderer/continuationEndpoint/continuationCommand/token",
            )?
            .as_str()
        })
        .filter(|token| token.len() <= 4_096)
        .map(str::to_string);
    (songs, more)
}

/// A playlist's page (`browse` of "VL…"); nothing when the answer is no playlist.
pub fn playlist(answer: &Value) -> Option<PlaylistPage> {
    let head = header(answer)?;
    let title = text(head.pointer("/title/runs/0/text"))?;
    let author = text(head.pointer("/facepile/avatarStackViewModel/text/content"))
        .or_else(|| {
            let strap = head
                .pointer("/straplineTextOne/runs")
                .and_then(Value::as_array);
            Some(second_line(strap).artist).filter(|named| !named.is_empty())
        })
        .unwrap_or_default();
    let mut page = PlaylistPage {
        title,
        author,
        art: picture(head, "thumbnail"),
        songs: Vec::new(),
        more: None,
    };
    for section in sections(answer) {
        let Some(shelf) = section
            .get("musicPlaylistShelfRenderer")
            .or_else(|| section.get("musicShelfRenderer"))
        else {
            continue;
        };
        let Some(items) = shelf.get("contents").and_then(Value::as_array) else {
            continue;
        };
        let (songs, more) = playlist_rows(items);
        page.songs.extend(songs);
        // The older page keeps the word beside the rows.
        page.more = more.or_else(|| {
            shelf
                .pointer("/continuations/0/nextContinuationData/continuation")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    }
    Some(page)
}

/// The songs of an answer to "the songs after these", and the word for the ones after those.
pub fn playlist_more(answer: &Value) -> (Vec<Song>, Option<String>) {
    let appended = answer
        .get("onResponseReceivedActions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|action| {
            action
                .pointer("/appendContinuationItemsAction/continuationItems")?
                .as_array()
        });
    if let Some(items) = appended {
        return playlist_rows(items);
    }
    // The older answer.
    let shelf = ["musicPlaylistShelfContinuation", "musicShelfContinuation"]
        .iter()
        .find_map(|name| answer.get("continuationContents")?.get(name));
    let Some(shelf) = shelf else {
        return (Vec::new(), None);
    };
    let (songs, more) = shelf
        .get("contents")
        .and_then(Value::as_array)
        .map(|items| playlist_rows(items))
        .unwrap_or_default();
    let more = more.or_else(|| {
        shelf
            .pointer("/continuations/0/nextContinuationData/continuation")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    (songs, more)
}
