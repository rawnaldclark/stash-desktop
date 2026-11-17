//! YouTube Music, built in: recorded answers read as songs, and a server of the test's own asked as YouTube is.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use stash_core::addons::{Code, StreamRequest, Tier};
use stash_core::reach::Reach;
use stash_core::youtube::home::{self, Card};
use stash_core::youtube::login::Login;
use stash_core::youtube::parse::{self, Kind};
use stash_core::youtube::{self, Saved, YouTube};

fn recorded(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/youtube")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A search's answer holding these shelves.
fn answer(shelves: Value) -> Value {
    json!({ "contents": { "tabbedSearchResultsRenderer": { "tabs": [
        { "tabRenderer": { "content": { "sectionListRenderer": { "contents": shelves } } } }
    ] } } })
}

#[test]
fn a_search_for_songs_is_read_row_by_row() {
    // The "Songs" shelf, as YouTube gave it: artist • album • length, and a count that is no album.
    let shelf = answer(json!([{ "musicShelfRenderer": recorded("search_songs_shelf_live.json") }]));
    let songs = parse::songs(&shelf);
    assert_eq!(songs.len(), 5);
    let first = &songs[0];
    assert_eq!(first.id, "Ba1mzFtR_Zk");
    assert_eq!(
        first.title,
        "Mii Channel Theme (feat. Gabe Nekrutman & Chris Allison)"
    );
    assert_eq!(first.artist, "Insaneintherainmusic");
    assert_eq!(first.album.as_deref(), Some("Mii Channel Theme"));
    assert_eq!(first.duration_ms, Some(313_000));
    assert!(
        songs.iter().all(
            |s| !s.artist.is_empty() && s.album.as_deref().is_none_or(|a| !a.contains("plays"))
        )
    );

    // A song and its clean twin: told apart by the badge.
    let twins = parse::songs(&recorded("search_songs_explicit_twins.json"));
    assert_eq!(twins.len(), 4);
    assert_eq!(
        (twins[0].id.as_str(), twins[0].explicit, twins[0].kind),
        ("wuO4_P_8p-Q", true, Some(Kind::Song))
    );
    assert_eq!(
        (twins[1].id.as_str(), twins[1].explicit),
        ("_MIsk8VoNhM", false)
    );
    assert_eq!(twins[0].title, twins[1].title);
    assert_eq!(twins[0].duration_ms, Some(136_000));
    assert!(
        twins[0]
            .art
            .as_deref()
            .is_some_and(|art| art.starts_with("https://") && art.contains("=w544-h544"))
    );
}

#[test]
fn the_flat_layout_of_a_visitor_who_is_not_signed_in_is_read_too() {
    let flat = parse::songs(&answer(recorded("search_flat_live_rows.json")));
    assert!(!flat.is_empty());
    for song in &flat {
        assert_eq!(song.id.len(), 11);
        assert!(!song.title.is_empty());
        // "Song" and "Video" say what the row is; a count of views is not who made it.
        assert!(
            !["Song", "Video"].contains(&song.artist.as_str()),
            "{song:?}"
        );
        assert!(!song.artist.contains("views"), "{song:?}");
    }
    let card = parse::songs(&recorded("search_flat_explicit_top_card.json"));
    assert_eq!(card.len(), 1);
    assert_eq!(
        (card[0].artist.as_str(), card[0].album.as_deref()),
        ("Kanye West", None)
    );
    assert!(parse::songs(&recorded("search_empty.json")).is_empty());
    assert!(parse::songs(&json!({ "contents": 3 })).is_empty());
}

/// What the test's server was asked: the path, the headers (lower-case names) and the body.
#[derive(Clone, Debug)]
struct Asked {
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Asked {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
    fn client(&self) -> &str {
        self.body["context"]["client"]["clientName"]
            .as_str()
            .unwrap_or_default()
    }
}

/// A server that answers as YouTube's API does, for the videos the tests name.
fn server() -> (String, Arc<Mutex<Vec<Asked>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let asked: Arc<Mutex<Vec<Asked>>> = Arc::default();
    let (noted, here) = (Arc::clone(&asked), base.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    headers.push((name.trim().to_lowercase(), value.trim().to_string()));
                }
            }
            let length = headers
                .iter()
                .find(|(n, _)| n == "content-length")
                .and_then(|(_, v)| v.parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let request = Asked {
                path: path.clone(),
                headers,
                body,
            };
            noted.lock().unwrap().push(request.clone());

            let playable = |file: &str| {
                json!({
                    "playabilityStatus": { "status": "OK" },
                    "streamingData": { "adaptiveFormats": [
                        { "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 130000, "contentLength": "5000",
                          "url": format!("{here}/file/{file}-aac?expire=1794900000") },
                        { "mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 141000, "contentLength": "4000",
                          "audioSampleRate": "48000", "approxDurationMs": "136000",
                          "url": format!("{here}/file/{file}?expire=1794900000") },
                        { "mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 160000, "signatureCipher": "s=scrambled" }
                    ] }
                })
            };
            let refused = |status: &str, reason: &str| json!({ "playabilityStatus": { "status": status, "reason": reason } });
            // Signed in: the test's account is "good"; "spent" is one YouTube no longer takes.
            let signed = request.header("cookie").unwrap_or_default().to_string();
            let refuse_spent = signed.contains("SAPISID=spent");
            let (status, answer): (u16, String) = if refuse_spent {
                (401, String::new())
            } else if path.starts_with("/youtubei/v1/account/accounts_list") {
                if signed.contains("SAPISID=good") {
                    let item = json!({ "accountName": { "simpleText": "Stash Tester" }, "channelHandle": { "simpleText": "@tester" }, "isSelected": true });
                    (
                        200,
                        json!({ "contents": [{ "accountItem": item }] }).to_string(),
                    )
                } else if signed.contains("SAPISID=nobody") {
                    // A session that signed out: answered as anyone is, with no account in it.
                    (200, json!({ "contents": [] }).to_string())
                } else {
                    (401, String::new())
                }
            } else if path.starts_with("/youtubei/v1/browse") {
                let page = request.body["browseId"].as_str().unwrap_or_default();
                if !request.body["continuation"].is_null() {
                    (200, recorded("home_more_live.json").to_string())
                } else if page == "FEmusic_home" {
                    (200, recorded("home_live.json").to_string())
                } else if page == "FEmusic_liked_playlists" && signed.contains("SAPISID=good") {
                    let card = |id: &str, title: &str| {
                        json!({ "musicTwoRowItemRenderer": {
                        "title": { "runs": [{ "text": title }] },
                        "subtitle": { "runs": [{ "text": "Stash Tester" }] },
                        "navigationEndpoint": { "browseEndpoint": { "browseId": id, "browseEndpointContextSupportedConfigs": { "browseEndpointContextMusicConfig": { "pageType": "MUSIC_PAGE_TYPE_PLAYLIST" } } } },
                    } })
                    };
                    let grid = json!({ "contents": { "singleColumnBrowseResultsRenderer": { "tabs": [{ "tabRenderer": { "content": { "sectionListRenderer": { "contents": [
                        { "gridRenderer": { "items": [card("VLLM", "Liked music"), card("VLPLmine", "Road trip")] } }
                    ] } } } }] } } });
                    (200, grid.to_string())
                } else {
                    (401, String::new())
                }
            } else if path.starts_with("/youtubei/v1/search") {
                let mut found = recorded("search_songs_explicit_twins.json");
                found["responseContext"] = json!({ "visitorData": "VISITOR%3D" });
                (200, found.to_string())
            } else if path.starts_with("/youtubei/v1/player") {
                let video = request.body["videoId"].as_str().unwrap_or_default();
                let answer = match (video, request.client()) {
                    ("botbotbotbo", _) => {
                        refused("LOGIN_REQUIRED", "Sign in to confirm you’re not a bot")
                    }
                    ("gonegonegon", _) => refused("ERROR", "This video is unavailable"),
                    ("agedagedage", _) => refused("LOGIN_REQUIRED", "Sign in to confirm your age"),
                    // One app asks whether the listener is a bot; the next plays.
                    ("halfbothalf", "ANDROID_VR") => {
                        refused("LOGIN_REQUIRED", "Sign in to confirm you’re not a bot")
                    }
                    ("halfbothalf", _) => playable("good"),
                    // One app may not play it, the next names a file that stops after its start, the third plays.
                    (_, "ANDROID_VR") => refused("UNPLAYABLE", "The page needs to be reloaded."),
                    (_, "IOS") => playable("gated"),
                    _ => playable("good"),
                };
                (200, answer.to_string())
            } else if path.starts_with("/file/gated") {
                (403, String::new())
            } else if path.starts_with("/file/") {
                (206, "x".to_string())
            } else {
                (404, String::new())
            };
            let mut out = stream;
            let _ = write!(
                out,
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    (base, asked)
}

#[test]
fn youtube_is_asked_as_its_own_apps_ask_it() {
    let (base, asked) = server();
    let api = format!("{base}/youtubei/v1");
    let youtube = Arc::new(YouTube::at(&api, &api));
    let source = youtube.source(Reach::Here);
    assert!(source.is_built_in());
    assert_eq!(source.manifest.id, youtube::ID);
    assert!(source.offers_resource("search") && source.offers_resource("stream"));
    assert!(!source.offers_download() && !source.offers_resource("resolve"));

    // A search: as YouTube Music on the web, for songs only; the visitor id it hands out is kept and sent back.
    let found = source.search("father stretch my hands", 3).unwrap();
    assert_eq!(found.len(), 3);
    assert_eq!(
        (
            found[0].id.as_str(),
            found[0].artist.as_str(),
            found[0].explicit
        ),
        ("wuO4_P_8p-Q", "Kanye West", Some(true))
    );
    assert_eq!(found[1].explicit, None);
    assert_eq!(found[0].max_tier, Some(Tier::Low));
    assert!(found[0].art().is_some());
    assert_eq!(youtube.visitor().as_deref(), Some("VISITOR%3D"));
    source.search("again", 3).unwrap();
    {
        let asked = asked.lock().unwrap();
        let first = &asked[0];
        assert!(
            first.path.starts_with("/youtubei/v1/search?key="),
            "{}",
            first.path
        );
        assert_eq!(first.client(), "WEB_REMIX");
        assert_eq!(first.header("x-youtube-client-name"), Some("67"));
        assert_eq!(first.header("x-goog-visitor-id"), None);
        assert_eq!(first.header("cookie"), None);
        assert_eq!(first.header("authorization"), None);
        assert_eq!(first.body["query"], "father stretch my hands");
        assert!(first.body["params"].as_str().is_some_and(|p| !p.is_empty()));
        assert_eq!(first.body["context"]["client"]["hl"], "en");
        let version = first.body["context"]["client"]["clientVersion"]
            .as_str()
            .unwrap();
        assert!(
            version.starts_with("1.20") && version.ends_with(".01.00"),
            "{version}"
        );
        let second = &asked[1];
        assert_eq!(second.header("x-goog-visitor-id"), Some("VISITOR%3D"));
        assert_eq!(
            second.body["context"]["client"]["visitorData"],
            "VISITOR%3D"
        );
    }
    asked.lock().unwrap().clear();

    // A stream: the apps in turn, until one names a file that gives its last byte.
    let stream = source
        .stream("wuO4_P_8p-Q", &StreamRequest::default())
        .unwrap();
    assert_eq!(stream.url, format!("{base}/file/good?expire=1794900000"));
    assert_eq!(
        (
            stream.codec.as_deref(),
            stream.container.as_deref(),
            stream.tier
        ),
        (Some("opus"), Some("webm"), Tier::Low)
    );
    assert_eq!(
        (
            stream.bitrate,
            stream.sample_rate,
            stream.duration_ms,
            stream.expires_at
        ),
        (
            Some(141_000),
            Some(48_000),
            Some(136_000),
            Some(1_794_900_000_000)
        )
    );
    {
        let asked = asked.lock().unwrap();
        let said: Vec<String> = asked
            .iter()
            .map(|a| {
                if a.path.starts_with("/youtubei/") {
                    a.client().to_string()
                } else {
                    format!("{} {}", a.path, a.header("range").unwrap_or("-"))
                }
            })
            .collect();
        assert_eq!(
            said,
            [
                "ANDROID_VR",
                "IOS",
                "/file/gated?expire=1794900000 bytes=3999-",
                "VISIONOS",
                "/file/good?expire=1794900000 bytes=3999-"
            ]
        );
        let player = &asked[0];
        assert!(
            player
                .path
                .starts_with("/youtubei/v1/player?prettyPrint=false"),
            "{}",
            player.path
        );
        assert_eq!(player.header("x-youtube-client-name"), Some("28"));
        assert_eq!(player.body["context"]["client"]["androidSdkVersion"], 32);
        assert_eq!(player.body["videoId"], "wuO4_P_8p-Q");
        assert!(asked.iter().all(|a| a.header("cookie").is_none()));
    }
    // The app that gave it is asked first the next time.
    asked.lock().unwrap().clear();
    source
        .stream("_MIsk8VoNhM", &StreamRequest::default())
        .unwrap();
    assert_eq!(asked.lock().unwrap()[0].client(), "VISIONOS");

    // A video that is gone, one no app plays without signing in, and what is not a video.
    let gone = source
        .stream("gonegonegon", &StreamRequest::default())
        .unwrap_err();
    assert_eq!(gone.code, Code::NotFound);
    assert_eq!(
        gone.in_words(),
        "the video is no longer on YouTube (“This video is unavailable”)"
    );
    let aged = source
        .stream("agedagedage", &StreamRequest::default())
        .unwrap_err();
    assert!(aged.is_miss(), "{aged:?}");
    assert!(
        aged.in_words().contains("Sign in to confirm your age"),
        "{}",
        aged.in_words()
    );
    asked.lock().unwrap().clear();
    let not_one = source
        .stream("../../etc", &StreamRequest::default())
        .unwrap_err();
    assert_eq!(not_one.code, Code::NotFound);
    assert!(asked.lock().unwrap().is_empty());

    // One app asking whether the listener is a bot is that app's trouble: the next is asked.
    Arc::new(YouTube::at(&api, &api))
        .source(Reach::Here)
        .stream("halfbothalf", &StreamRequest::default())
        .unwrap();
    asked.lock().unwrap().clear();

    // Every app asking is YouTube's word: it is left alone, and nothing more is sent for a while.
    let bot = source
        .stream("botbotbotbo", &StreamRequest::default())
        .unwrap_err();
    assert_eq!(bot.code, Code::RateLimited);
    assert!(bot.in_words().contains("not a bot"), "{}", bot.in_words());
    assert_eq!(asked.lock().unwrap().len(), 4);
    let after = source.search("anything", 3).unwrap_err();
    assert_eq!(after.code, Code::RateLimited);
    assert!(after.retry_after.is_some_and(|s| (1..=240).contains(&s)));
    assert_eq!(asked.lock().unwrap().len(), 4);
}

/// The real YouTube, asked once for a search and once for a stream. Run by hand:
/// `cargo test -p stash-core --test youtube -- --ignored --nocapture`.
#[test]
#[ignore = "asks the real YouTube"]
fn the_real_youtube_finds_a_song_and_says_where_it_plays_from() {
    let youtube = Arc::new(YouTube::default());
    let source = youtube.source(Reach::Internet);
    let found = source
        .search("Never Gonna Give You Up Rick Astley", 5)
        .unwrap();
    for track in &found {
        eprintln!(
            "found: {} | {} | {:?} | {:?} ms | art {}",
            track.title,
            track.artist,
            track.album,
            track.duration_ms,
            track.art().is_some()
        );
    }
    assert!(!found.is_empty());
    assert!(youtube.visitor().is_some());
    let started = std::time::Instant::now();
    let stream = source
        .stream(&found[0].id, &StreamRequest::default())
        .unwrap();
    let host = url::Url::parse(&stream.url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.ends_with(".googlevideo.com")));
    eprintln!(
        "stream: {:?} in {:?}, {:?} bit/s, {:?} Hz, {:?} ms, on googlevideo: {:?}, expires: {}, in {} ms",
        stream.codec,
        stream.container,
        stream.bitrate,
        stream.sample_rate,
        stream.duration_ms,
        host,
        stream.expires_at.is_some(),
        started.elapsed().as_millis()
    );
}

#[test]
fn a_search_for_everything_is_sorted_by_what_each_row_opens() {
    use stash_core::youtube::pages::{self, Top};
    // The real YouTube's answer to "Daft Punk", cut down; the rows naming people are given made-up names.
    let found = pages::found(&recorded("found_live.json"));
    let Some(Top::Artist(top)) = &found.top else {
        panic!("the card is the artist: {:?}", found.top);
    };
    assert_eq!(
        (top.id.as_str(), top.name.as_str()),
        ("UCRr1xG_2WIDs18a6cIiCxeA", "Daft Punk")
    );
    assert!(
        top.art
            .as_deref()
            .is_some_and(|art| art.contains("=w544-h544"))
    );
    let albums: Vec<_> = found
        .albums
        .iter()
        .map(|a| {
            (
                a.id.as_str(),
                a.title.as_str(),
                a.artist.as_str(),
                a.year,
                a.single,
            )
        })
        .collect();
    assert_eq!(
        albums,
        [
            (
                "MPREb_K8qWMWVqXGi",
                "Random Access Memories",
                "Daft Punk",
                Some(2013),
                false
            ),
            (
                "MPREb_7ltM34kr0mH",
                "Discovery",
                "Daft Punk",
                Some(2001),
                false
            ),
            (
                "MPREb_CtOaObihCXA",
                "Alive 2007",
                "Daft Punk",
                Some(2007),
                false
            ),
        ]
    );
    assert!(
        found
            .albums
            .iter()
            .all(|a| a.artist_id.as_deref() == Some("UCRr1xG_2WIDs18a6cIiCxeA") && a.art.is_some())
    );
    let artists: Vec<_> = found.artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(artists, ["The Weeknd", "Gesaffelstein", "Pharrell"]);
    // A listener's playlist and YouTube Music's own; who made each.
    let playlists: Vec<_> = found
        .playlists
        .iter()
        .map(|p| (p.title.as_str(), p.author.as_str()))
        .collect();
    assert_eq!(
        playlists,
        [
            ("A listener's playlist 1", "A listener"),
            ("A listener's playlist 2", "A listener"),
            ("Summer Sonic 2026: Lineup", "YouTube Music"),
            ("Indie Disco", "YouTube Music"),
            ("Acclaimed: Music Videos as Art", "YouTube Music"),
        ]
    );
    assert!(found.playlists.iter().all(|p| p.id.starts_with("VL")));
    // Songs and videos; the songs on the artist's card are the artist's. No episode, podcast or person's channel.
    assert_eq!(found.songs.len(), 11);
    assert!(
        found
            .songs
            .iter()
            .all(|s| s.artist.starts_with("Daft Punk")),
        "{:?}",
        found.songs
    );
    assert_eq!(
        (
            found.songs[0].title.as_str(),
            found.songs[0].duration_ms,
            found.songs[0].artist_id.as_deref()
        ),
        (
            "Something About Us (Love Theme from Interstella 5555)",
            Some(135_000),
            Some("UCRr1xG_2WIDs18a6cIiCxeA")
        )
    );
    assert_eq!(
        found
            .songs
            .iter()
            .filter(|s| s.kind == Some(Kind::Video))
            .count(),
        3
    );
    assert_eq!(pages::found(&json!({})), pages::Found::default());
}

#[test]
fn the_pages_of_an_album_an_artist_and_a_playlist_are_read() {
    use stash_core::youtube::pages;
    // An album: its rows name no artist and no album of their own; two of its tracks are one video.
    let album = pages::album(&recorded("album_live.json"), "MPREb_DYSzDJvUSDu").unwrap();
    assert_eq!(
        (
            album.title.as_str(),
            album.artist.as_str(),
            album.year,
            album.single
        ),
        (
            "Something About Us (Love Theme from Interstella)",
            "Daft Punk",
            Some(2003),
            true
        )
    );
    assert_eq!(album.artist_id.as_deref(), Some("UCRr1xG_2WIDs18a6cIiCxeA"));
    let tracks: Vec<_> = album
        .songs
        .iter()
        .map(|s| (s.id.as_str(), s.title.as_str(), s.duration_ms))
        .collect();
    assert_eq!(
        tracks,
        [
            (
                "sOS9aOIXPEk",
                "Something About Us (Love Theme from Interstella 5555)",
                Some(135_000)
            ),
            ("HhZaHf8RP6g", "Veridis Quo", Some(346_000)),
            (
                "c8a-XCZjTRs",
                "Voyager (Dominique Torti's Wild Style Edit)",
                Some(393_000)
            ),
            ("sOS9aOIXPEk", "Something About Us", Some(233_000)),
        ]
    );
    assert!(album.songs.iter().all(|s| s.artist == "Daft Punk"
        && s.album.as_deref() == Some(album.title.as_str())
        && s.album_id.as_deref() == Some("MPREb_DYSzDJvUSDu")
        && s.art == album.art
        && s.art.is_some()));
    assert_eq!(pages::album(&recorded("artist_live.json"), "MPREb_x"), None);

    // An artist: their songs with the album each is on, their albums and their singles apart, and who else.
    let artist = pages::artist(&recorded("artist_live.json")).unwrap();
    assert_eq!(artist.name, "Daft Punk");
    assert!(artist.art.is_some());
    let songs: Vec<_> = artist
        .songs
        .iter()
        .map(|s| (s.title.as_str(), s.artist.as_str(), s.album.as_deref()))
        .collect();
    assert_eq!(songs.len(), 5);
    assert_eq!(
        songs[0],
        (
            "Instant Crush (feat. Julian Casablancas)",
            "Daft Punk, Julian Casablancas",
            Some("Random Access Memories")
        )
    );
    assert_eq!(
        artist.songs[4].album_id.as_deref(),
        Some("MPREb_7ltM34kr0mH")
    );
    let albums: Vec<_> = artist
        .albums
        .iter()
        .map(|a| (a.title.as_str(), a.artist.as_str(), a.year))
        .collect();
    assert_eq!(
        albums,
        [
            (
                "Random Access Memories (Drumless Edition)",
                "Daft Punk",
                Some(2023)
            ),
            ("Random Access Memories", "Daft Punk", Some(2013)),
            ("TRON: Legacy", "Daft Punk", Some(2010)),
        ]
    );
    let singles: Vec<_> = artist
        .singles
        .iter()
        .map(|a| (a.title.as_str(), a.year, a.single))
        .collect();
    assert_eq!(singles[0], ("GLBTM (Studio Outtakes)", Some(2023), true));
    assert_eq!(singles.len(), 3);
    let related: Vec<_> = artist.related.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(related, ["Capital Cities", "Gotye", "Justice"]);
    assert_eq!(pages::artist(&recorded("album_live.json")), None);

    // A playlist: who made it, its rows, and the word that asks for the songs after them.
    let playlist = pages::playlist(&recorded("playlist_live.json")).unwrap();
    assert_eq!(
        (
            playlist.title.as_str(),
            playlist.author.as_str(),
            playlist.songs.len()
        ),
        ("Summer Sonic 2026: Lineup", "YouTube Music", 5)
    );
    assert_eq!(
        (
            playlist.songs[0].title.as_str(),
            playlist.songs[0].artist.as_str(),
            playlist.songs[0].duration_ms
        ),
        ("Reptilia (Official HD Video)", "The Strokes", Some(216_000))
    );
    assert!(playlist.more.as_deref().is_some_and(|word| word.len() > 50));
    let (more, next) = pages::playlist_more(&recorded("playlist_more_live.json"));
    assert_eq!(more.len(), 4);
    assert!(next.is_some());
    assert_eq!(pages::playlist_more(&json!({})), (Vec::new(), None));
}

#[test]
fn a_listener_who_signed_in_is_asked_for_as_themselves_but_never_searches_so() {
    let (base, asked) = server();
    let api = format!("{base}/youtubei/v1");
    let youtube = YouTube::at(&api, &api);
    let last = || asked.lock().unwrap().last().cloned().unwrap();

    // Not signed in: the library is not asked for at all.
    let before = asked.lock().unwrap().len();
    let e = youtube.library(Saved::Playlists).unwrap_err();
    assert_eq!(e.code, Code::Unauthorized);
    assert!(e.message.contains("Sign in"), "{}", e.message);
    assert_eq!(asked.lock().unwrap().len(), before);

    youtube.set_login(Login::read("SAPISID=good; LOGIN_INFO=x; SID=s"));
    let account = youtube.account().unwrap();
    assert_eq!(
        (account.name.as_str(), account.handle.as_deref()),
        ("Stash Tester", Some("@tester"))
    );
    // As YouTube Music's own page asks: the cookies, the hash of the session, its origin, and no key.
    let signed = last();
    assert_eq!(
        signed.header("cookie"),
        Some("SAPISID=good; LOGIN_INFO=x; SID=s")
    );
    assert!(
        signed
            .header("authorization")
            .is_some_and(|a| a.starts_with("SAPISIDHASH ") && a.len() == 12 + 11 + 40)
    );
    assert_eq!(signed.header("origin"), Some("https://music.youtube.com"));
    assert_eq!(signed.header("x-goog-authuser"), Some("0"));
    assert!(!signed.path.contains("key="), "{}", signed.path);

    // A search never is: it would land in the account's history.
    youtube.songs("daft punk").unwrap();
    let search = last();
    assert_eq!(search.header("cookie"), None);
    assert_eq!(search.header("authorization"), None);
    assert!(search.path.contains("key="));

    // Their library, and their home (read on through the rest).
    let cards = youtube.library(Saved::Playlists).unwrap();
    let ids: Vec<_> = cards
        .iter()
        .map(|card| match card {
            Card::Playlist(p) => p.id.as_str(),
            _ => "",
        })
        .collect();
    assert_eq!(ids, ["VLLM", "VLPLmine"]);
    let shelves = youtube.home().unwrap();
    assert_eq!(shelves.len(), 8);
    assert_eq!(
        shelves[..4]
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        [
            "Pump it up",
            "Throwbacks",
            "Feeling happy",
            "Irresistible sing-alongs"
        ]
    );
    assert!(last().header("cookie").is_some());

    // A login YouTube no longer takes: said once, not sent again; pages are asked signed out, the library refused.
    youtube.set_login(Login::read("SAPISID=spent"));
    assert_eq!(youtube.account().unwrap_err().code, Code::Unauthorized);
    assert!(youtube.login_spent());
    assert_eq!(youtube.home().unwrap().len(), 8);
    assert_eq!(last().header("cookie"), None);
    let before = asked.lock().unwrap().len();
    let e = youtube.library(Saved::Albums).unwrap_err();
    assert!(e.message.contains("sign in again"), "{}", e.message);
    assert_eq!(asked.lock().unwrap().len(), before);

    // A session YouTube answers as anyone (it signed out on the web) is one it no longer takes, too.
    youtube.set_login(Login::read("SAPISID=nobody"));
    assert_eq!(youtube.account().unwrap_err().code, Code::Unauthorized);
    assert!(youtube.login_spent());

    // Signed out.
    youtube.set_login(None);
    assert!(!youtube.login_spent() && youtube.login().is_none());
}

#[test]
fn the_home_page_is_read_shelf_by_shelf() {
    let (shelves, more) = home::shelves(&recorded("home_live.json"));
    assert_eq!(shelves.len(), 2);
    assert_eq!(shelves[1].title, "Throwbacks");
    assert_eq!(
        shelves[1].over.as_deref(),
        Some("CLASSICS FROM EVERY DECADE")
    );
    assert_eq!(shelves[0].cards.len(), 3);
    let Card::Playlist(first) = &shelves[0].cards[0] else {
        panic!("{:?}", shelves[0].cards[0])
    };
    assert!(first.id.starts_with("VL") && !first.title.is_empty() && first.art.is_some());
    assert!(more.is_some());
    let (rest, after) = home::shelves(&recorded("home_more_live.json"));
    assert_eq!(rest.len(), 2);
    assert!(after.is_some());
}
