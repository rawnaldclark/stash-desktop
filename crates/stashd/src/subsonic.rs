//! The Subsonic API, with OpenSubsonic's additions, as the library is served on the network ([`crate::lan`]): what
//! Subsonic apps (Symfonium, Feishin, Substreamer, DSub…) ask of a server, answered from the library. Songs play from
//! their files when they are here, else from wherever the player would play them (a copy kept, the listener's cloud
//! storage, their add-ons), and the bytes come through the core. Answers are JSON (`f=json`) or XML, as asked.
//!
//! Ids carry what they name: `s-12` a song, `al-3` an album, `ar-4` an artist, `pl-5` a playlist.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use md5::{Digest, Md5};
use serde_json::{Map, Value, json};
use stash_core::served::{Album, AlbumOrder, Artist, Song};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::lan::Device;
use crate::serve::{State, now_ms};

const VERSION: &str = "1.16.1";
/// The most items one list answers.
const CAP: usize = 500;
/// How long where a song plays from is kept (a player asks for it again as it seeks).
const LOCATED_FOR: Duration = Duration::from_secs(5 * 60);

type Body = Box<dyn Read + Send>;
type Out = Response<Body>;

/// Why a request was not answered: Subsonic's code, and words.
#[derive(Debug, PartialEq)]
struct Fail(u32, String);

fn fail(code: u32, message: impl Into<String>) -> Fail {
    Fail(code, message.into())
}

fn lib<T>(r: stash_core::Result<T>) -> Result<T, Fail> {
    r.map_err(|e| fail(0, e.to_string()))
}

enum Reply {
    Payload(Map<String, Value>),
    Raw(Out),
}

fn one(key: &str, value: Value) -> Reply {
    let mut map = Map::new();
    map.insert(key.to_string(), value);
    Reply::Payload(map)
}

/// The request's parameters, from its address and (a form sent by POST) its body; a name may come more than once.
struct Params(Vec<(String, String)>);

impl Params {
    fn one(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
    fn all(&self, name: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }
    fn num<T: std::str::FromStr>(&self, name: &str) -> Option<T> {
        self.one(name).and_then(|v| v.trim().parse().ok())
    }
    fn need(&self, name: &str) -> Result<&str, Fail> {
        self.one(name)
            .ok_or_else(|| fail(10, format!("the request needs {name}")))
    }
}

/// A Stash id with its prefix (`s-12`), or a bare number (taken as of that kind).
fn id_of(text: &str, prefix: &str) -> Option<i64> {
    text.strip_prefix(prefix)
        .unwrap_or(text)
        .parse()
        .ok()
        .filter(|n| *n > 0)
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a well-formed header")
}

fn bytes_out(status: u16, content_type: &str, bytes: Vec<u8>) -> Out {
    let length = bytes.len();
    Response::new(
        StatusCode(status),
        vec![header("Content-Type", content_type)],
        Box::new(Cursor::new(bytes)) as Body,
        Some(length),
        None,
    )
}

// ---- answers in JSON or XML

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `value` as Subsonic's XML: an object is an element whose plain fields are its attributes and whose objects and
/// lists are its children (a list, one child per item).
fn element(out: &mut String, name: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            out.push('<');
            out.push_str(name);
            for (k, v) in map {
                if let Some(text) = scalar(v) {
                    out.push_str(&format!(" {k}=\"{}\"", escape(&text)));
                }
            }
            let children: Vec<(&String, &Value)> = map
                .iter()
                .filter(|(_, v)| v.is_object() || v.is_array())
                .collect();
            if children.is_empty() {
                out.push_str("/>");
                return;
            }
            out.push('>');
            for (k, v) in children {
                element(out, k, v);
            }
            out.push_str(&format!("</{name}>"));
        }
        Value::Array(items) => {
            for item in items {
                element(out, name, item);
            }
        }
        Value::Null => {}
        other => {
            let text = scalar(other).unwrap_or_default();
            out.push_str(&format!("<{name}>{}</{name}>", escape(&text)));
        }
    }
}

fn envelope(json: bool, status: &str, payload: Map<String, Value>) -> Out {
    let mut root = Map::new();
    root.insert("status".into(), status.into());
    root.insert("version".into(), VERSION.into());
    root.insert("type".into(), "stash".into());
    root.insert("serverVersion".into(), env!("CARGO_PKG_VERSION").into());
    root.insert("openSubsonic".into(), true.into());
    root.extend(payload);
    if json {
        let body = json!({ "subsonic-response": Value::Object(root) }).to_string();
        bytes_out(200, "application/json; charset=utf-8", body.into_bytes())
    } else {
        root.insert("xmlns".into(), "http://subsonic.org/restapi".into());
        let mut body = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
        element(&mut body, "subsonic-response", &Value::Object(root));
        bytes_out(200, "text/xml; charset=utf-8", body.into_bytes())
    }
}

// ---- what the library is, as Subsonic names it

/// A time as Subsonic writes it (ISO 8601, UTC).
fn iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Drops the empty fields of an object.
fn clean(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.retain(|_, v| !v.is_null());
    }
    value
}

const KNOWN_ENDINGS: &[&str] = &[
    "flac", "mp3", "m4a", "m4b", "mp4", "aac", "ogg", "oga", "opus", "wav", "aiff", "aif", "ape",
    "wv", "mpc", "webm",
];

fn suffix_of(s: &Song) -> &str {
    if let Some(ending) = s.ending.as_deref().filter(|e| KNOWN_ENDINGS.contains(e)) {
        return ending;
    }
    match s.codec.as_deref() {
        Some("flac") => "flac",
        Some("aac" | "alac") => "m4a",
        Some("opus") => "opus",
        Some("vorbis") => "ogg",
        Some("pcm") => "wav",
        Some("ape") => "ape",
        Some("wavpack") => "wv",
        Some("musepack") => "mpc",
        _ => "mp3",
    }
}

fn content_type(suffix: &str) -> &'static str {
    match suffix {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "m4b" | "mp4" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "aif" | "aiff" => "audio/aiff",
        "ape" => "audio/x-ape",
        "wv" => "audio/x-wavpack",
        "mpc" => "audio/x-musepack",
        "webm" => "audio/webm",
        _ => "application/octet-stream",
    }
}

fn song_json(s: &Song) -> Value {
    let suffix = suffix_of(s);
    let seconds = (s.duration_ms + 500) / 1000;
    clean(json!({
        "id": format!("s-{}", s.id),
        "parent": s.album_id.map(|a| format!("al-{a}")),
        "isDir": false,
        "title": s.title,
        "album": s.album,
        "artist": s.artist,
        "track": (s.track_no > 0).then_some(s.track_no),
        "discNumber": s.disc_no,
        "year": s.year,
        "coverArt": s.art.as_ref().map(|_| format!("s-{}", s.id)),
        "size": s.size,
        "contentType": content_type(suffix),
        "suffix": suffix,
        "duration": seconds,
        "bitRate": s.size.filter(|_| seconds > 0).map(|size| size * 8 / 1000 / seconds),
        "samplingRate": s.sample_rate,
        "bitDepth": s.bit_depth,
        "playCount": s.play_count,
        "created": iso(s.added_at_s * 1000),
        "starred": s.liked_at_ms.map(iso),
        "albumId": s.album_id.map(|a| format!("al-{a}")),
        "artistId": format!("ar-{}", s.artist_id),
        "type": "music",
        "mediaType": "song",
        "isVideo": false,
    }))
}

fn album_json(a: &Album) -> Value {
    clean(json!({
        "id": format!("al-{}", a.id),
        "parent": format!("ar-{}", a.artist_id),
        "isDir": true,
        "name": a.title,
        "title": a.title,
        "album": a.title,
        "artist": a.artist,
        "artistId": format!("ar-{}", a.artist_id),
        "coverArt": a.art.as_ref().map(|_| format!("al-{}", a.id)),
        "songCount": a.songs,
        "duration": a.duration_ms / 1000,
        "playCount": a.play_count,
        "created": iso(a.added_at_s * 1000),
        "year": a.year,
    }))
}

fn artist_json(a: &Artist) -> Value {
    clean(json!({
        "id": format!("ar-{}", a.id),
        "name": a.name,
        "albumCount": a.albums,
        "coverArt": a.art.as_ref().map(|_| format!("ar-{}", a.id)),
    }))
}

fn songs_json(songs: &[Song]) -> Value {
    Value::Array(songs.iter().map(song_json).collect())
}

fn albums_json(albums: &[Album]) -> Value {
    Value::Array(albums.iter().map(album_json).collect())
}

/// Artists under the letter their name is sorted by ("#" for one that starts with no letter).
fn indexed(artists: &[Artist]) -> Value {
    let mut by: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for a in artists {
        let letter = stash_core::library::sort_key(&a.name)
            .chars()
            .next()
            .filter(|c| c.is_alphabetic())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "#".into());
        by.entry(letter).or_default().push(artist_json(a));
    }
    Value::Array(
        by.into_iter()
            .map(|(name, artist)| json!({ "name": name, "artist": artist }))
            .collect(),
    )
}

// ---- the request

pub(crate) fn answer(state: &State, mut request: Request, path: &str) {
    let ip: Option<IpAddr> = request.remote_addr().map(|a| a.ip());
    let mut pairs: Vec<(String, String)> =
        url::Url::parse(&format!("http://local{}", request.url()))
            .map(|u| u.query_pairs().into_owned().collect())
            .unwrap_or_default();
    let form = request.headers().iter().any(|h| {
        h.field.equiv("Content-Type")
            && h.value
                .as_str()
                .starts_with("application/x-www-form-urlencoded")
    });
    if *request.method() == Method::Post && form {
        let mut body = Vec::new();
        let _ = request.as_reader().take(256 * 1024).read_to_end(&mut body);
        pairs.extend(url::form_urlencoded::parse(&body).into_owned());
    }
    let params = Params(pairs);
    let method = path
        .trim_start_matches("/rest/")
        .trim_end_matches(".view")
        .to_string();
    let json = matches!(params.one("f"), Some("json"));
    let range = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str().to_string());
    let answered = if method == "getOpenSubsonicExtensions" {
        Ok(extensions())
    } else {
        authorise(state, &params, ip)
            .and_then(|device| handle(state, &params, &method, &device, range.as_deref()))
    };
    let out = match answered {
        Ok(Reply::Payload(map)) => envelope(json, "ok", map),
        Ok(Reply::Raw(out)) => out,
        Err(Fail(code, message)) => {
            let mut map = Map::new();
            map.insert("error".into(), json!({ "code": code, "message": message }));
            envelope(json, "failed", map)
        }
    };
    let _ = request.respond(out);
}

fn md5_hex(text: &str) -> String {
    Md5::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `enc:`-written hex as text.
fn unhex(hex: &str) -> Option<String> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

/// The device the request comes from, by its user name and password (a token from the password and a salt, the
/// password itself, or an API key that is the password).
fn authorise(state: &State, p: &Params, ip: Option<IpAddr>) -> Result<Device, Fail> {
    let lan = &state.lan;
    if lan.shut_out(ip) {
        return Err(fail(
            40,
            "too many wrong passwords from here: try again in a few minutes",
        ));
    }
    let found = if let Some(key) = p.one("apiKey") {
        lan.device(state, None, |password| password == key)
    } else {
        let user = p.need("u")?;
        match (p.one("t"), p.one("s"), p.one("p")) {
            (Some(token), Some(salt), _) => {
                let token = token.to_ascii_lowercase();
                lan.device(state, Some(user), |password| {
                    md5_hex(&format!("{password}{salt}")) == token
                })
            }
            (_, _, Some(given)) => {
                let given = match given.strip_prefix("enc:") {
                    Some(hex) => unhex(hex).unwrap_or_default(),
                    None => given.to_string(),
                };
                lan.device(state, Some(user), |password| password == given)
            }
            _ => return Err(fail(10, "the request names no password")),
        }
    };
    found.ok_or_else(|| {
        lan.got_wrong(ip);
        fail(40, "wrong user name or password")
    })
}

fn extensions() -> Reply {
    one(
        "openSubsonicExtensions",
        json!([
            { "name": "apiKeyAuthentication", "versions": [1] },
            { "name": "formPost", "versions": [1] },
        ]),
    )
}

fn handle(
    state: &State,
    p: &Params,
    method: &str,
    device: &Device,
    range: Option<&str>,
) -> Result<Reply, Fail> {
    let library = || state.library.lock().unwrap();
    let size = |name: &str, default: usize| p.num::<usize>(name).unwrap_or(default).min(CAP);
    let offset = |name: &str| p.num::<usize>(name).unwrap_or(0);
    match method {
        "ping" => Ok(Reply::Payload(Map::new())),
        "getLicense" => Ok(one("license", json!({ "valid": true }))),
        "getUser" => Ok(one(
            "user",
            json!({
                "username": device.user, "scrobblingEnabled": true, "adminRole": false, "settingsRole": false,
                "downloadRole": true, "uploadRole": false, "playlistRole": true, "coverArtRole": true,
                "commentRole": false, "podcastRole": false, "streamRole": true, "jukeboxRole": false,
                "shareRole": false, "videoConversionRole": false, "folder": [1],
            }),
        )),
        "getMusicFolders" => Ok(one(
            "musicFolders",
            json!({ "musicFolder": [{ "id": 1, "name": "Stash" }] }),
        )),
        "getScanStatus" | "startScan" => {
            let count = lib(library().counts())?.tracks;
            Ok(one(
                "scanStatus",
                json!({ "scanning": false, "count": count }),
            ))
        }
        "getArtists" => {
            let artists = lib(library().served_artists())?;
            Ok(one(
                "artists",
                json!({ "ignoredArticles": "The", "index": indexed(&artists) }),
            ))
        }
        "getIndexes" => {
            let artists = lib(library().served_artists())?;
            Ok(one(
                "indexes",
                json!({ "lastModified": now_ms(), "ignoredArticles": "The", "index": indexed(&artists) }),
            ))
        }
        "getArtist" => {
            let id = id_of(p.need("id")?, "ar-").ok_or_else(|| fail(70, "no such artist"))?;
            let l = library();
            let artist = lib(l.served_artist(id))?.ok_or_else(|| fail(70, "no such artist"))?;
            let albums = lib(l.served_artist_albums(id))?;
            let mut value = artist_json(&artist);
            value["album"] = albums_json(&albums);
            Ok(one("artist", value))
        }
        "getAlbum" => {
            let id = id_of(p.need("id")?, "al-").ok_or_else(|| fail(70, "no such album"))?;
            let l = library();
            let album = lib(l.served_album(id))?.ok_or_else(|| fail(70, "no such album"))?;
            let songs = lib(l.served_album_songs(id))?;
            let mut value = album_json(&album);
            value["song"] = songs_json(&songs);
            Ok(one("album", value))
        }
        "getSong" => {
            let id = id_of(p.need("id")?, "s-").ok_or_else(|| fail(70, "no such song"))?;
            let song = lib(library().served_songs(&[id]))?
                .pop()
                .ok_or_else(|| fail(70, "no such song"))?;
            Ok(one("song", song_json(&song)))
        }
        "getMusicDirectory" => {
            let id = p.need("id")?;
            let l = library();
            if let Some(artist) = id.strip_prefix("ar-").and_then(|n| n.parse().ok()) {
                let found =
                    lib(l.served_artist(artist))?.ok_or_else(|| fail(70, "no such folder"))?;
                let albums = lib(l.served_artist_albums(artist))?;
                return Ok(one(
                    "directory",
                    json!({ "id": id, "name": found.name, "child": albums_json(&albums) }),
                ));
            }
            let album = id_of(id, "al-").ok_or_else(|| fail(70, "no such folder"))?;
            let found = lib(l.served_album(album))?.ok_or_else(|| fail(70, "no such folder"))?;
            let songs = lib(l.served_album_songs(album))?;
            Ok(one(
                "directory",
                json!({ "id": id, "parent": format!("ar-{}", found.artist_id), "name": found.title, "child": songs_json(&songs) }),
            ))
        }
        "getAlbumList" | "getAlbumList2" => {
            let key = if method == "getAlbumList2" {
                "albumList2"
            } else {
                "albumList"
            };
            let order = match p.one("type").unwrap_or("newest") {
                "random" => Some(AlbumOrder::Random),
                "newest" => Some(AlbumOrder::Newest),
                "frequent" | "highest" => Some(AlbumOrder::Frequent),
                "recent" => Some(AlbumOrder::Recent),
                "alphabeticalByName" => Some(AlbumOrder::ByName),
                "alphabeticalByArtist" => Some(AlbumOrder::ByArtist),
                "byYear" => Some(AlbumOrder::Years(
                    p.num("fromYear").unwrap_or(0),
                    p.num("toYear").unwrap_or(9999),
                )),
                // Stash keeps no liked albums, and no genres.
                "starred" | "byGenre" => None,
                other => return Err(fail(10, format!("no such list: {other}"))),
            };
            let albums = match order {
                Some(order) => {
                    lib(library().served_albums(order, offset("offset"), size("size", 10)))?
                }
                None => Vec::new(),
            };
            Ok(one(key, json!({ "album": albums_json(&albums) })))
        }
        "getRandomSongs" => {
            let songs = lib(library().served_random_songs(size("size", 10)))?;
            Ok(one("randomSongs", json!({ "song": songs_json(&songs) })))
        }
        "getStarred" | "getStarred2" => {
            let key = if method == "getStarred2" {
                "starred2"
            } else {
                "starred"
            };
            let songs = lib(library().served_liked(10_000))?;
            Ok(one(
                key,
                json!({ "artist": [], "album": [], "song": songs_json(&songs) }),
            ))
        }
        "search" | "search2" | "search3" => {
            let key = if method == "search3" {
                "searchResult3"
            } else {
                "searchResult2"
            };
            let query = p
                .one("query")
                .or_else(|| p.one("any"))
                .unwrap_or("")
                .trim()
                .trim_matches('"')
                .trim_end_matches('*')
                .trim()
                .to_string();
            let (n_artists, n_albums, n_songs) = (
                size("artistCount", 20),
                size("albumCount", 20),
                size("songCount", 20),
            );
            let (o_artists, o_albums, o_songs) = (
                offset("artistOffset"),
                offset("albumOffset"),
                offset("songOffset"),
            );
            let l = library();
            let (artists, albums, songs) = if query.is_empty() {
                // Every song, album and artist, a page at a time: how an app copies the whole library.
                (
                    lib(l.served_artists_at(o_artists, n_artists))?,
                    lib(l.served_albums_at(o_albums, n_albums))?,
                    lib(l.served_songs_at(o_songs, n_songs))?,
                )
            } else {
                let page = |ids: Vec<i64>, from: usize, n: usize| {
                    ids.into_iter().skip(from).take(n).collect::<Vec<_>>()
                };
                let song_ids = page(
                    lib(l.search(&query, o_songs + n_songs))?
                        .iter()
                        .map(|r| r.id)
                        .collect(),
                    o_songs,
                    n_songs,
                );
                let album_ids = page(
                    lib(l.search_albums(&query, o_albums + n_albums))?
                        .iter()
                        .map(|r| r.id)
                        .collect(),
                    o_albums,
                    n_albums,
                );
                let artist_ids = page(
                    lib(l.search_artists(&query, o_artists + n_artists))?
                        .iter()
                        .map(|r| r.id)
                        .collect(),
                    o_artists,
                    n_artists,
                );
                (
                    lib(l.served_artists_of(&artist_ids))?,
                    lib(l.served_albums_of(&album_ids))?,
                    lib(l.served_songs(&song_ids))?,
                )
            };
            Ok(one(
                key,
                json!({
                    "artist": artists.iter().map(artist_json).collect::<Vec<_>>(),
                    "album": albums_json(&albums),
                    "song": songs_json(&songs),
                }),
            ))
        }
        "getGenres" => Ok(one("genres", json!({ "genre": [] }))),
        "getNowPlaying" => Ok(one("nowPlaying", json!({}))),
        "getPlayQueue" | "savePlayQueue" => Ok(Reply::Payload(Map::new())),
        "getArtistInfo" | "getArtistInfo2" => Ok(one(
            if method == "getArtistInfo2" {
                "artistInfo2"
            } else {
                "artistInfo"
            },
            json!({}),
        )),
        "getAlbumInfo" | "getAlbumInfo2" => Ok(one("albumInfo", json!({}))),
        "getSimilarSongs" | "getSimilarSongs2" => Ok(one(
            if method == "getSimilarSongs2" {
                "similarSongs2"
            } else {
                "similarSongs"
            },
            json!({}),
        )),
        "getTopSongs" => Ok(one("topSongs", json!({}))),
        "getLyrics" => Ok(one("lyrics", json!({}))),
        "getInternetRadioStations" => Ok(one("internetRadioStations", json!({}))),
        "getPodcasts" => Ok(one("podcasts", json!({}))),
        "getNewestPodcasts" => Ok(one("newestPodcasts", json!({}))),
        "getBookmarks" => Ok(one("bookmarks", json!({}))),
        "getShares" => Ok(one("shares", json!({}))),
        "star" | "unstar" => {
            let liked = method == "star";
            let mut l = library();
            for id in p.all("id").into_iter().filter_map(|id| id_of(id, "s-")) {
                lib(l.set_liked(id, liked, now_ms()))?;
            }
            drop(l);
            state.edited();
            Ok(Reply::Payload(Map::new()))
        }
        "scrobble" => {
            if p.one("submission") == Some("false") {
                return Ok(Reply::Payload(Map::new()));
            }
            let times = p.all("time");
            let send = crate::scrobbling::on_play(
                Arc::clone(&state.library),
                Arc::clone(&state.scrobbling),
            );
            for (i, id) in p.all("id").into_iter().enumerate() {
                let Some(id) = id_of(id, "s-") else { continue };
                let at = times
                    .get(i)
                    .and_then(|t| t.parse::<i64>().ok())
                    .unwrap_or_else(now_ms);
                let item = {
                    let mut l = library();
                    lib(l.record_play(id, at))?;
                    lib(l.queue_items(&[id]))?.into_iter().next()
                };
                if let Some(item) = item {
                    send(&item, at);
                }
            }
            Ok(Reply::Payload(Map::new()))
        }
        "getPlaylists" => {
            let rows = lib(library().playlists())?;
            let mut out = Vec::new();
            for row in &rows {
                out.push(playlist_json(state, row, device, false)?);
            }
            Ok(one("playlists", json!({ "playlist": out })))
        }
        "getPlaylist" => {
            let row = playlist_row(state, p.need("id")?)?;
            Ok(one("playlist", playlist_json(state, &row, device, true)?))
        }
        "createPlaylist" => {
            let songs: Vec<i64> = p
                .all("songId")
                .into_iter()
                .filter_map(|id| id_of(id, "s-"))
                .collect();
            let id = match p.one("playlistId") {
                Some(given) => {
                    let row = editable(state, given)?;
                    let mut l = library();
                    for at in (0..row.tracks as usize).rev() {
                        lib(l.remove_from_playlist(row.id, at, now_ms()))?;
                    }
                    if let Some(name) = p.one("name").filter(|n| !n.trim().is_empty()) {
                        lib(l.rename_playlist(row.id, name.trim(), now_ms()))?;
                    }
                    row.id
                }
                None => {
                    let name = p.need("name")?.trim();
                    if name.is_empty() {
                        return Err(fail(10, "the playlist needs a name"));
                    }
                    lib(library().create_playlist(name, now_ms()))?
                }
            };
            if !songs.is_empty() {
                lib(library().add_to_playlist(id, &songs, now_ms()))?;
            }
            state.edited();
            let row = playlist_row(state, &format!("pl-{id}"))?;
            Ok(one("playlist", playlist_json(state, &row, device, true)?))
        }
        "updatePlaylist" => {
            let row = editable(state, p.need("playlistId")?)?;
            let mut l = library();
            if let Some(name) = p.one("name").filter(|n| !n.trim().is_empty()) {
                lib(l.rename_playlist(row.id, name.trim(), now_ms()))?;
            }
            let mut gone: Vec<usize> = p
                .all("songIndexToRemove")
                .into_iter()
                .filter_map(|i| i.parse().ok())
                .collect();
            gone.sort_unstable();
            gone.dedup();
            for at in gone.into_iter().rev() {
                lib(l.remove_from_playlist(row.id, at, now_ms()))?;
            }
            let added: Vec<i64> = p
                .all("songIdToAdd")
                .into_iter()
                .filter_map(|id| id_of(id, "s-"))
                .collect();
            if !added.is_empty() {
                lib(l.add_to_playlist(row.id, &added, now_ms()))?;
            }
            drop(l);
            state.edited();
            Ok(Reply::Payload(Map::new()))
        }
        "deletePlaylist" => {
            let row = editable(state, p.need("id")?)?;
            lib(library().delete_playlist(row.id))?;
            state.edited();
            Ok(Reply::Payload(Map::new()))
        }
        // A picture asked for is a picture or nothing (an image loader does not read an answer in words).
        "getCoverArt" => Ok(cover(state, p.need("id")?).unwrap_or_else(|_| {
            Reply::Raw(bytes_out(
                404,
                "text/plain; charset=utf-8",
                b"no picture".to_vec(),
            ))
        })),
        "stream" | "download" => stream(state, p, range),
        "jukeboxControl" | "createUser" | "updateUser" | "deleteUser" | "changePassword" => {
            Err(fail(50, "Stash does not let devices do that"))
        }
        other => Err(fail(0, format!("Stash does not answer {other}"))),
    }
}

// ---- playlists

fn playlist_row(state: &State, id: &str) -> Result<stash_core::PlaylistRow, Fail> {
    let id = id_of(id, "pl-").ok_or_else(|| fail(70, "no such playlist"))?;
    lib(state.library.lock().unwrap().playlists())?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| fail(70, "no such playlist"))
}

/// A playlist the listener edits here: not one that follows another's, nor a smart one.
fn editable(state: &State, id: &str) -> Result<stash_core::PlaylistRow, Fail> {
    let row = playlist_row(state, id)?;
    if row.followed || row.rules.is_some() {
        return Err(fail(50, "this playlist is not edited by hand"));
    }
    Ok(row)
}

fn playlist_json(
    state: &State,
    row: &stash_core::PlaylistRow,
    device: &Device,
    with_songs: bool,
) -> Result<Value, Fail> {
    let songs = {
        let l = state.library.lock().unwrap();
        let ids = lib(l.playlist_track_ids(row.id, 0, 10_000))?;
        lib(l.served_songs(&ids))?
    };
    let created = row.created_at_ms.unwrap_or(0);
    let mut value = clean(json!({
        "id": format!("pl-{}", row.id),
        "name": row.name,
        "owner": device.user,
        "public": false,
        "songCount": songs.len(),
        "duration": songs.iter().map(|s| s.duration_ms).sum::<u64>() / 1000,
        "created": iso(created),
        "changed": iso(row.updated_at_ms.unwrap_or(created)),
        "coverArt": songs.iter().any(|s| s.art.is_some()).then(|| format!("pl-{}", row.id)),
        "readonly": row.followed || row.rules.is_some(),
    }));
    if with_songs {
        value["entry"] = songs_json(&songs);
    }
    Ok(value)
}

// ---- covers

fn cover(state: &State, id: &str) -> Result<Reply, Fail> {
    let address = {
        let l = state.library.lock().unwrap();
        let none = || fail(70, "no picture");
        if let Some(n) = id.strip_prefix("al-").and_then(|n| n.parse().ok()) {
            lib(l.served_album(n))?.and_then(|a| a.art)
        } else if let Some(n) = id.strip_prefix("ar-").and_then(|n| n.parse().ok()) {
            lib(l.served_artist(n))?.and_then(|a| a.art)
        } else if let Some(n) = id.strip_prefix("pl-").and_then(|n| n.parse().ok()) {
            let ids = lib(l.playlist_track_ids(n, 0, 200))?;
            lib(l.served_songs(&ids))?.into_iter().find_map(|s| s.art)
        } else {
            let n = id_of(id, "s-").ok_or_else(none)?;
            lib(l.served_songs(&[n]))?.pop().and_then(|s| s.art)
        }
        .ok_or_else(none)?
    };
    let (content_type, bytes) = picture(state, &address).ok_or_else(|| fail(70, "no picture"))?;
    let mut out = bytes_out(200, content_type, bytes.to_vec());
    out.add_header(header("Cache-Control", "private, max-age=86400"));
    Ok(Reply::Raw(out))
}

/// The picture at a row's art address: a file's own, a cloud song's kept one, or one on the web (kept a while).
fn picture(state: &State, address: &str) -> Option<(&'static str, Arc<Vec<u8>>)> {
    if let Some(n) = address
        .strip_prefix(stash_core::covers::ADDRESS)
        .and_then(|n| n.parse::<i64>().ok())
    {
        let file = state.library.lock().unwrap().file_of(n).ok()??;
        let cover = stash_core::covers::of_file(std::path::Path::new(&file))?;
        return Some((cover.content_type, Arc::new(cover.bytes)));
    }
    if let Some(name) = address.strip_prefix(stash_core::cloud::scan::ART) {
        let cover = state.cloud()?.cover(name)?;
        return Some((cover.content_type, Arc::new(cover.bytes)));
    }
    if !address.starts_with("https://") {
        return None;
    }
    if let Some((_, kind, bytes)) = state
        .lan
        .covers
        .lock()
        .unwrap()
        .iter()
        .find(|(a, _, _)| a == address)
    {
        return Some((kind, Arc::clone(bytes)));
    }
    let bytes = stash_core::downloads::fetch_cover(address, stash_core::reach::Reach::Internet)?;
    let kind = stash_core::covers::kind(&bytes)?;
    let bytes = Arc::new(bytes);
    let mut kept = state.lan.covers.lock().unwrap();
    if kept.len() >= 64 {
        kept.remove(0);
    }
    kept.push((address.to_string(), kind, Arc::clone(&bytes)));
    Some((kind, bytes))
}

// ---- playing

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` of a file of `length` bytes: the first and last byte.
fn range_of(range: Option<&str>, length: u64) -> Option<(u64, u64)> {
    let spec = range?.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || length == 0 {
        return None;
    }
    let (from, to) = spec.split_once('-')?;
    let (start, end) = match (from.trim(), to.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (length.saturating_sub(n), length - 1)
        }
        (a, "") => (a.parse().ok()?, length - 1),
        (a, b) => (a.parse().ok()?, b.parse::<u64>().ok()?.min(length - 1)),
    };
    (start <= end && start < length).then_some((start, end))
}

fn serve_file(path: &str, range: Option<&str>) -> Result<Reply, Fail> {
    let mut file =
        std::fs::File::open(path).map_err(|_| fail(70, "the song's file is not there"))?;
    let length = file
        .metadata()
        .map(|m| m.len())
        .map_err(|_| fail(70, "the song's file cannot be read"))?;
    let ending = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let mut headers = vec![
        header("Content-Type", content_type(&ending)),
        header("Accept-Ranges", "bytes"),
    ];
    let (status, body, size): (u16, Body, u64) = match range_of(range, length) {
        Some((start, end)) => {
            file.seek(SeekFrom::Start(start))
                .map_err(|_| fail(0, "the file cannot be read"))?;
            headers.push(header(
                "Content-Range",
                &format!("bytes {start}-{end}/{length}"),
            ));
            (206, Box::new(file.take(end - start + 1)), end - start + 1)
        }
        None => (200, Box::new(file), length),
    };
    Ok(Reply::Raw(Response::new(
        StatusCode(status),
        headers,
        body,
        Some(size as usize),
        None,
    )))
}

/// The bytes at `url` (the core's relay for a song an add-on or the cloud gives), with the range asked for.
/// `Err(None)`: the address no longer answers for the song (it is to be found again).
fn proxy(url: &str, range: Option<&str>) -> Result<Reply, Option<String>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(90)))
        .build()
        .into();
    let mut asked = agent.get(url);
    if let Some(range) = range {
        asked = asked.header("Range", range);
    }
    let response = asked.call().map_err(|e| Some(e.to_string()))?;
    let status = response.status().as_u16();
    if matches!(status, 403 | 404 | 410) {
        return Err(None);
    }
    if !(200..300).contains(&status) {
        return Err(Some(format!("the song's source answered {status}")));
    }
    let given = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let mut headers = vec![
        header(
            "Content-Type",
            &given("content-type").unwrap_or_else(|| "application/octet-stream".into()),
        ),
        header("Accept-Ranges", "bytes"),
    ];
    if let Some(range) = given("content-range") {
        headers.push(header("Content-Range", &range));
    }
    let length = given("content-length").and_then(|l| l.parse::<usize>().ok());
    let body: Body = Box::new(response.into_body().into_reader());
    Ok(Reply::Raw(Response::new(
        StatusCode(status),
        headers,
        body,
        length,
        None,
    )))
}

/// Where the song plays from: as the player finds it (kept a few minutes; `fresh`: found again).
fn locate(state: &State, id: i64, fresh: bool) -> Result<stash_core::player::Located, Fail> {
    if !fresh
        && let Some((located, at)) = state.lan.located.lock().unwrap().get(&id)
        && at.elapsed() < LOCATED_FOR
    {
        return Ok(located.clone());
    }
    let item = lib(state.library.lock().unwrap().queue_items(&[id]))?
        .into_iter()
        .next()
        .ok_or_else(|| fail(70, "no such song"))?;
    let located = (state.resolve)(&item).map_err(|f| fail(70, f.message))?;
    let mut kept = state.lan.located.lock().unwrap();
    kept.retain(|_, (_, at)| at.elapsed() < LOCATED_FOR);
    kept.insert(id, (located.clone(), Instant::now()));
    Ok(located)
}

fn stream(state: &State, p: &Params, range: Option<&str>) -> Result<Reply, Fail> {
    let id = id_of(p.need("id")?, "s-").ok_or_else(|| fail(70, "no such song"))?;
    let file = lib(state.library.lock().unwrap().file_of(id))?;
    if let Some(path) = file {
        return serve_file(&path, range);
    }
    for fresh in [false, true] {
        let located = locate(state, id, fresh)?;
        if !located.locator.starts_with("http://") && !located.locator.starts_with("https://") {
            return serve_file(&located.locator, range);
        }
        match proxy(&located.locator, range) {
            Ok(reply) => return Ok(reply),
            Err(None) if !fresh => continue,
            Err(why) => {
                return Err(fail(
                    0,
                    why.unwrap_or_else(|| "the song could not be had".into()),
                ));
            }
        }
    }
    Err(fail(0, "the song could not be had"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_written_as_subsonic_writes_them() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso(1_795_100_000_123), "2026-11-19T14:53:20.000Z");
    }

    #[test]
    fn a_range_is_read_as_players_send_it() {
        assert_eq!(range_of(Some("bytes=0-"), 100), Some((0, 99)));
        assert_eq!(range_of(Some("bytes=10-19"), 100), Some((10, 19)));
        assert_eq!(range_of(Some("bytes=90-500"), 100), Some((90, 99)));
        assert_eq!(range_of(Some("bytes=-10"), 100), Some((90, 99)));
        assert_eq!(range_of(Some("bytes=100-"), 100), None);
        assert_eq!(range_of(Some("bytes=0-1,5-6"), 100), None);
        assert_eq!(range_of(None, 100), None);
    }

    #[test]
    fn an_answer_is_subsonics_xml_or_json() {
        let mut out = String::new();
        let value = json!({ "status": "ok", "album": [{ "id": "al-1", "name": "A & B", "song": [{ "id": "s-2" }] }], "license": { "valid": true } });
        element(&mut out, "r", &value);
        assert!(out.starts_with("<r status=\"ok\">"));
        assert!(out.contains("<album id=\"al-1\" name=\"A &amp; B\"><song id=\"s-2\"/></album>"));
        assert!(out.contains("<license valid=\"true\"/>"));
        assert!(out.ends_with("</r>"));
    }

    #[test]
    fn a_password_is_checked_as_a_token_or_given_whole() {
        // The Subsonic API's own example: password "sesame", salt "c19b2d".
        assert_eq!(md5_hex("sesamec19b2d"), "26719a1196d2a940705a59634eb18eab");
        assert_eq!(unhex("736573616d65").as_deref(), Some("sesame"));
        assert_eq!(unhex("7"), None);
        assert_eq!(id_of("s-12", "s-"), Some(12));
        assert_eq!(id_of("12", "s-"), Some(12));
        assert_eq!(id_of("al-3", "s-"), None);
    }

    #[test]
    fn a_songs_kind_is_its_files_else_its_audios() {
        let mut s = Song {
            id: 1,
            title: "T".into(),
            artist: "A".into(),
            artist_id: 2,
            album: None,
            album_id: None,
            track_no: 0,
            disc_no: 1,
            year: None,
            duration_ms: 0,
            codec: Some("alac".into()),
            sample_rate: None,
            bit_depth: None,
            size: None,
            play_count: 0,
            added_at_s: 0,
            liked_at_ms: None,
            art: None,
            ending: None,
        };
        assert_eq!(suffix_of(&s), "m4a");
        s.ending = Some("flac".into());
        assert_eq!(suffix_of(&s), "flac");
        let v = song_json(&s);
        assert_eq!(v["contentType"], "audio/flac");
        assert!(v.get("track").is_none() && v.get("coverArt").is_none());
    }
}
