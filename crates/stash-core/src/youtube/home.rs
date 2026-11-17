//! YouTube Music's home and, for a listener who signed in, their library: the shelves of the home page (each a title
//! and its cards), the cards of a library page (their playlists, their albums, their artists), and who they are.
//!
//! These pages change shape more than search does (a grid one year, a shelf the next), so their cards are taken from
//! wherever in the answer they are, and every field is read in each of the forms YouTube writes it.

use serde_json::Value;

use super::pages::{
    AlbumCard, ArtistCard, Opens, PlaylistCard, episode, opening_card, opening_row,
};
use super::parse::{Song, explicit, kind, page_id, picture, row, second_line, text, video_id};

/// A card of a shelf or of the library: a song plays, the rest open a page.
#[derive(Debug, Clone, PartialEq)]
pub enum Card {
    Song(Song),
    Album(AlbumCard),
    Artist(ArtistCard),
    Playlist(PlaylistCard),
}

impl Card {
    fn key(&self) -> (&'static str, &str) {
        match self {
            Card::Song(song) => ("song", &song.id),
            Card::Album(album) => ("album", &album.id),
            Card::Artist(artist) => ("artist", &artist.id),
            Card::Playlist(playlist) => ("playlist", &playlist.id),
        }
    }
}

/// A shelf of the home page: its title (and the small line above it, "MIXED FOR YOU"), and its cards.
#[derive(Debug, Clone, PartialEq)]
pub struct Shelf {
    pub title: String,
    pub over: Option<String>,
    pub cards: Vec<Card>,
}

/// Who signed in.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub name: String,
    /// "@handle", when the account has a channel.
    pub handle: Option<String>,
    pub photo: Option<String>,
}

/// `{"simpleText": …}` or `{"runs": [{"text": …}, …]}`, as words.
fn words(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(simple) = value.get("simpleText") {
        return text(Some(simple));
    }
    let joined: String = value
        .get("runs")?
        .as_array()?
        .iter()
        .filter_map(|run| run.get("text")?.as_str())
        .collect();
    text(Some(&Value::from(joined)))
}

/// A card of a carousel (`musicTwoRowItemRenderer`) that plays: a song (a video), or a mix that plays at once (a
/// playlist).
fn playing_card(card: &Value) -> Option<Card> {
    let title = text(card.pointer("/title/runs/0/text"))?;
    let line = card.pointer("/subtitle/runs").and_then(Value::as_array);
    let art = picture(card, "thumbnailRenderer");
    if let Some(watch) = card.pointer("/navigationEndpoint/watchEndpoint") {
        let id = video_id(watch.get("videoId")?.as_str()?)?;
        let said = second_line(line);
        return Some(Card::Song(Song {
            id: id.to_string(),
            title,
            artist: said.artist,
            album: said.album,
            duration_ms: said.length,
            art,
            explicit: explicit(card),
            kind: kind(Some(watch)),
            artist_id: said.artist_id,
            album_id: said.album_id,
        }));
    }
    let list = card.pointer("/navigationEndpoint/watchPlaylistEndpoint/playlistId")?;
    let id = page_id(list.as_str()?)?;
    let author = line
        .into_iter()
        .flatten()
        .filter_map(|run| run.get("text")?.as_str())
        .collect::<String>();
    Some(Card::Playlist(PlaylistCard {
        id,
        title,
        author: text(Some(&Value::from(author))).unwrap_or_default(),
        art,
    }))
}

fn opened(opens: Opens) -> Card {
    match opens {
        Opens::Album(album) => Card::Album(album),
        Opens::Artist(artist) => Card::Artist(artist),
        Opens::Playlist(playlist) => Card::Playlist(playlist),
    }
}

/// A card of any kind: a carousel's card, or a row of a list.
fn card(item: &Value) -> Option<Card> {
    if let Some(card) = item.get("musicTwoRowItemRenderer") {
        return opening_card(card)
            .map(opened)
            .or_else(|| playing_card(card));
    }
    let item = item.get("musicResponsiveListItemRenderer")?;
    if let Some(opens) = opening_row(item) {
        return Some(opened(opens));
    }
    if episode(item) {
        return None;
    }
    row(item).map(Card::Song)
}

/// Whether a card is one Stash shows: a library's "Episodes for later" is podcasts', and a song needs an artist.
fn shown(card: &Card) -> bool {
    match card {
        Card::Song(song) => !song.artist.is_empty(),
        Card::Playlist(playlist) => playlist.id != "SE" && playlist.id != "VLSE",
        _ => true,
    }
}

/// The token for what comes after this answer, wherever it is given: an item at the end of a list, or beside the list.
fn continuation(answer: &Value) -> Option<String> {
    fn find(value: &Value) -> Option<&str> {
        match value {
            Value::Object(map) => {
                if let Some(token) = map
                    .get("nextContinuationData")
                    .and_then(|next| next.get("continuation"))
                    .or_else(|| {
                        map.get("continuationCommand")
                            .and_then(|command| command.get("token"))
                    })
                    .and_then(Value::as_str)
                {
                    return Some(token);
                }
                map.values().find_map(find)
            }
            Value::Array(items) => items.iter().find_map(find),
            _ => None,
        }
    }
    let token = find(answer)?;
    (!token.is_empty()
        && token.len() <= 4_096
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'%' | b'=')))
    .then(|| token.to_string())
}

/// Every card in `value`, in order (the same card once).
fn cards_in(value: &Value, into: &mut Vec<Card>) {
    match value {
        Value::Object(map) => {
            if let Some(found) = card(value) {
                if shown(&found) && !into.iter().any(|known| known.key() == found.key()) {
                    into.push(found);
                }
                return;
            }
            for inner in map.values() {
                cards_in(inner, into);
            }
        }
        Value::Array(items) => {
            for item in items {
                cards_in(item, into);
            }
        }
        _ => {}
    }
}

/// The cards of a library page (their playlists, albums or artists) and the token for the rest.
pub fn cards(answer: &Value) -> (Vec<Card>, Option<String>) {
    let mut found = Vec::new();
    // Not what comes after the answer: the menus and headers hold no cards, and the cards are under "contents" (or
    // "continuationContents", or the actions of an answer to "the rest").
    for part in [
        "contents",
        "continuationContents",
        "onResponseReceivedActions",
    ] {
        if let Some(value) = answer.get(part) {
            cards_in(value, &mut found);
        }
    }
    (found, continuation(answer))
}

/// The shelves of the home page's answer (the first, or one of "the rest"), and the token for the rest.
pub fn shelves(answer: &Value) -> (Vec<Shelf>, Option<String>) {
    let list = answer
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer")
        .or_else(|| answer.pointer("/continuationContents/sectionListContinuation"));
    let Some(list) = list else {
        return (Vec::new(), None);
    };
    let mut shelves = Vec::new();
    for section in list
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(shelf) = section
            .get("musicCarouselShelfRenderer")
            .or_else(|| section.get("musicShelfRenderer"))
        else {
            continue;
        };
        let header = shelf
            .pointer("/header/musicCarouselShelfBasicHeaderRenderer")
            .unwrap_or(shelf);
        let Some(title) = words(header.get("title")) else {
            continue;
        };
        let mut cards = Vec::new();
        cards_in(shelf.get("contents").unwrap_or(&Value::Null), &mut cards);
        if !cards.is_empty() {
            shelves.push(Shelf {
                title,
                over: words(header.get("strapline")),
                cards,
            });
        }
    }
    let more = list
        .get("continuations")
        .and_then(|value| continuation(&serde_json::json!({ "c": value })));
    (shelves, more)
}

/// Who signed in, from the answer to `account/accounts_list` (or the account menu): the account picked, else the
/// first.
pub fn account(answer: &Value) -> Option<Account> {
    fn collect<'a>(value: &'a Value, into: &mut Vec<&'a Value>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    if matches!(
                        key.as_str(),
                        "accountItem" | "accountItemRenderer" | "activeAccountHeaderRenderer"
                    ) && inner.is_object()
                    {
                        into.push(inner);
                    } else {
                        collect(inner, into);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| collect(item, into)),
            _ => {}
        }
    }
    let mut items = Vec::new();
    collect(answer, &mut items);
    let item = items
        .iter()
        .find(|item| item.get("isSelected").and_then(Value::as_bool) == Some(true))
        .or_else(|| items.first())?;
    let photo = item
        .pointer("/accountPhoto/thumbnails")
        .and_then(Value::as_array)
        .and_then(|all| all.last())
        .and_then(|last| last.get("url")?.as_str())
        .filter(|url| url.starts_with("https://") && url.len() <= 2_048)
        .map(str::to_string);
    Some(Account {
        name: words(item.get("accountName"))?,
        handle: words(item.get("channelHandle")).filter(|handle| handle.starts_with('@')),
        photo,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn two_row(title: &str, endpoint: Value, line: &str) -> Value {
        json!({ "musicTwoRowItemRenderer": {
            "title": { "runs": [{ "text": title }] },
            "subtitle": { "runs": [{ "text": line }] },
            "navigationEndpoint": endpoint,
        } })
    }

    #[test]
    fn cards_of_every_kind_are_read() {
        let album = two_row(
            "Discovery",
            json!({ "browseEndpoint": { "browseId": "MPREb_abc", "browseEndpointContextSupportedConfigs": { "browseEndpointContextMusicConfig": { "pageType": "MUSIC_PAGE_TYPE_ALBUM" } } } }),
            "Album",
        );
        let song = two_row(
            "One More Time",
            json!({ "watchEndpoint": { "videoId": "FGBhQbmPwH8" } }),
            "Daft Punk",
        );
        let mix = two_row(
            "My Mix",
            json!({ "watchPlaylistEndpoint": { "playlistId": "RDTMAK5uy_abc" } }),
            "Daft Punk, Justice",
        );
        let later = two_row(
            "Episodes for later",
            json!({ "browseEndpoint": { "browseId": "VLSE", "browseEndpointContextSupportedConfigs": { "browseEndpointContextMusicConfig": { "pageType": "MUSIC_PAGE_TYPE_PLAYLIST" } } } }),
            "Auto playlist",
        );
        let answer = json!({ "contents": { "gridRenderer": { "items": [album.clone(), song, mix, later, album] } } });
        let (cards, more) = cards(&answer);
        assert_eq!(more, None);
        assert_eq!(
            cards.iter().map(|card| card.key()).collect::<Vec<_>>(),
            vec![
                ("album", "MPREb_abc"),
                ("song", "FGBhQbmPwH8"),
                ("playlist", "RDTMAK5uy_abc"),
            ]
        );
        let Card::Song(song) = &cards[1] else {
            panic!()
        };
        assert_eq!(song.artist, "Daft Punk");
        let Card::Playlist(mix) = &cards[2] else {
            panic!()
        };
        assert_eq!(mix.author, "Daft Punk, Justice");

        // An artist of the listener's library: a row that opens their page in the library, read as the artist.
        let artist = json!({ "musicResponsiveListItemRenderer": {
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "Justice" }] } } },
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "12 songs" }] } } },
            ],
            "navigationEndpoint": { "browseEndpoint": { "browseId": "MPLAUCabcdef", "browseEndpointContextSupportedConfigs": { "browseEndpointContextMusicConfig": { "pageType": "MUSIC_PAGE_TYPE_LIBRARY_ARTIST" } } } },
        } });
        let (cards, _) = super::cards(
            &json!({ "contents": { "musicShelfRenderer": { "contents": [artist] } } }),
        );
        let [Card::Artist(artist)] = cards.as_slice() else {
            panic!("{cards:?}")
        };
        assert_eq!(
            (artist.id.as_str(), artist.name.as_str()),
            ("UCabcdef", "Justice")
        );
    }

    #[test]
    fn who_signed_in_is_read_from_the_account_list() {
        let answer = json!({ "contents": [{ "accountSectionListRenderer": { "contents": [{ "accountItemSectionRenderer": { "contents": [
            { "accountItem": { "accountName": { "simpleText": "A brand" }, "isSelected": false } },
            { "accountItem": {
                "accountName": { "runs": [{ "text": "Some " }, { "text": "One" }] },
                "channelHandle": { "simpleText": "@someone" },
                "accountPhoto": { "thumbnails": [{ "url": "https://yt3.example/s48" }, { "url": "https://yt3.example/s88" }] },
                "isSelected": true,
            } },
        ] } }] } }] });
        assert_eq!(
            account(&answer),
            Some(Account {
                name: "Some One".to_string(),
                handle: Some("@someone".to_string()),
                photo: Some("https://yt3.example/s88".to_string()),
            })
        );
        assert_eq!(account(&json!({ "contents": [] })), None);
    }
}
