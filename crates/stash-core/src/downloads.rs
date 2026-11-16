//! Saving songs from add-ons as files of the library: where a song's file goes, fetching it, and its tags.
//!
//! A song is saved under the downloads folder as `Artist/Album/NN Title.ext`, its tags written from what the library
//! knows it by (so the file is the same song wherever it is read later) and its cover put inside it. While it comes
//! in it is a `.part` file beside where it will be; a song that does not finish leaves nothing behind.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::addons::{AddonError, Code};
use crate::library::NewTrack;
use crate::reach::{Guarded, Reach};

/// The largest song file taken (a 24-bit/192 kHz album side is well under it).
pub const MAX_FILE: u64 = 2 << 30;
/// The largest cover taken to put inside a file.
const MAX_COVER: u64 = 8 << 20;
/// A part of a file or folder name is cut to this many characters.
const NAME_CAP: usize = 80;

/// What a downloaded file is known by: the library's description of the song.
#[derive(Debug, Clone, Default)]
pub struct Song {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub track_no: Option<u32>,
    pub isrc: Option<String>,
}

/// A name made safe for a file or folder on every system: no characters a system refuses, no names Windows
/// keeps for itself, no dot or space at the end, not empty, cut to a sensible length.
pub fn safe_name(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let mut name: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((cut, _)) = name.char_indices().nth(NAME_CAP) {
        name.truncate(cut);
    }
    let name = name.trim_end_matches(['.', ' ']).trim_start().to_string();
    let reserved = {
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit())
    };
    if name.is_empty() {
        "_".to_string()
    } else if reserved {
        format!("_{name}")
    } else {
        name
    }
}

/// Where a song's file goes under `root`: `Artist/Album/NN Title.ext` (without an album, `Artist/Title.ext`). A name
/// already taken gets " (2)", " (3)"… so nothing is written over.
pub fn path_for(root: &Path, song: &Song, extension: &str) -> PathBuf {
    let mut folder = root.join(safe_name(&song.artist));
    if let Some(album) = song.album.as_deref().filter(|a| !a.trim().is_empty()) {
        folder = folder.join(safe_name(album));
    }
    let stem = match song.track_no.filter(|n| *n > 0) {
        Some(n) => safe_name(&format!("{n:02} {}", song.title)),
        None => safe_name(&song.title),
    };
    let mut path = folder.join(format!("{stem}.{extension}"));
    let mut n = 2;
    while path.exists() || part_of(&path).exists() {
        path = folder.join(format!("{stem} ({n}).{extension}"));
        n += 1;
    }
    path
}

/// The file a song is written to while it comes in.
pub fn part_of(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// What kind of audio file a file's first bytes say it is, as its ending; `None` for anything that is not one of
/// the kinds the library reads (a web page, an error in JSON, a manifest).
pub fn kind_of(head: &[u8]) -> Option<&'static str> {
    let at = |i: usize, bytes: &[u8]| {
        head.len() >= i + bytes.len() && &head[i..i + bytes.len()] == bytes
    };
    if at(0, b"fLaC") {
        Some("flac")
    } else if at(0, b"ID3")
        || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 && head[1] & 0x06 != 0)
    {
        // An MP3 (an ADTS AAC frame also starts with 0xFFF but has layer bits 00).
        Some("mp3")
    } else if head.len() >= 2 && head[0] == 0xFF && head[1] & 0xF6 == 0xF0 {
        Some("aac")
    } else if at(4, b"ftyp") {
        Some("m4a")
    } else if at(0, b"OggS") {
        // Opus says so in its first page.
        if head.windows(8).any(|w| w == b"OpusHead") {
            Some("opus")
        } else {
            Some("ogg")
        }
    } else if at(0, b"RIFF") && at(8, b"WAVE") {
        Some("wav")
    } else if at(0, b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) {
        Some("aiff")
    } else if at(0, b"wvpk") {
        Some("wv")
    } else if at(0, b"MAC ") {
        Some("ape")
    } else {
        None
    }
}

/// Why a download could not be finished.
#[derive(Debug)]
pub enum Trouble {
    /// The listener took it off the list while it came in.
    Cancelled,
    /// The song's own (in words, for the list): it is given up on.
    Failed(String),
    /// Everyone's, never the song's: the add-on's allowance is spent or its key refused, too many asks, no add-on can
    /// be asked, the downloads folder cannot be written (a full disk, a drive unplugged). The song waits its turn
    /// again, and saving waits a while before it asks again: as many seconds as the second field says, when an add-on
    /// or server said.
    Held(String, Option<u32>),
    /// The add-on could not be reached at all (no connection, its server down): everyone's, and nothing was asked of
    /// it. The song waits its turn again, and saving waits a minute, then longer, but never long, so it goes on soon
    /// after the connection is back.
    Away(String),
    /// A server (the add-on's, or the file's) was busy, too slow, or stopped sending: maybe everyone's trouble, maybe
    /// this song's. Tried again later, and given up on at last.
    Busy(String, Option<u32>),
    /// "No" from add-ons that are not finding other songs either: maybe the song's own answer, maybe theirs (a broken
    /// lookup, a spent allowance that answers "nothing"). Tried again later; the flag says whether an add-on was asked
    /// this time (not only "no"s remembered from before).
    Later(String, bool),
    /// The add-on refused the request, or answered it with what the contract does not allow (its code): the song's
    /// own, unless other songs get the same.
    Refused(String, crate::addons::Code),
}

/// The downloads folder could not be written: not the song's trouble, and saving waits (a full disk, a drive that
/// was unplugged).
impl From<std::io::Error> for Trouble {
    fn from(e: std::io::Error) -> Self {
        Trouble::Held(
            format!("the downloads folder could not be written: {e}"),
            None,
        )
    }
}

/// Whether a request that got no answer may get one later: the line or the server (it could not be reached, it took
/// too long, it stopped sending), as opposed to the link itself (a bad address, a redirect to http, too many
/// redirects, a certificate that does not hold).
fn passing(e: &ureq::Error) -> bool {
    matches!(
        e,
        ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::BodyStalled
            | ureq::Error::ConnectProxyFailed(_)
    )
}

/// How long a server asked to be left alone (`Retry-After` in seconds), when it said.
fn retry_after(answer: &ureq::http::Response<ureq::Body>) -> Option<u32> {
    answer
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u32>().ok())
}

/// Fetches `url` into `part`, saying how far it got (`progress(bytes, of)`) and stopping when `cancelled` says so.
/// Answers the first bytes (to tell what kind of file it is) and the size. Redirects are followed, to https only, and
/// only as near as `reach` (the add-on's that named it: see [`crate::reach`]).
pub fn fetch(
    url: &str,
    reach: Reach,
    part: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(Vec<u8>, u64), Trouble> {
    crate::addons::checked_url(url, "the download link")
        .map_err(|e| Trouble::Failed(e.in_words()))?;
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        // A hi-res album side on a slow line takes a while; a line that stops altogether is let go at last.
        .timeout_recv_body(Some(Duration::from_secs(30 * 60)))
        .max_redirects(5)
        .https_only(!is_loopback(url))
        .build();
    let agent = Guarded::agent(reach, config);
    let mut answer = agent.get(url).call().map_err(|e| {
        if let Some(words) = crate::reach::refusal(&e) {
            return Trouble::Failed(format!("the file could not be fetched: {words}"));
        }
        let why = format!("the file could not be fetched: {e}");
        if passing(&e) {
            Trouble::Busy(why, None)
        } else {
            Trouble::Failed(why)
        }
    })?;
    let status = answer.status().as_u16();
    // Too many asks: everyone's. A server overwhelmed or in between: asked again later. Anything else is this file's own
    // answer.
    if status == 429 {
        return Err(Trouble::Held(
            format!("the file's server answered {status}"),
            retry_after(&answer),
        ));
    }
    if matches!(status, 502..=504) {
        return Err(Trouble::Busy(
            format!("the file's server answered {status}"),
            retry_after(&answer),
        ));
    }
    if !(200..300).contains(&status) {
        return Err(Trouble::Failed(format!(
            "the file's server answered {status}"
        )));
    }
    let of = answer
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if of.is_some_and(|n| n > MAX_FILE) {
        return Err(Trouble::Failed(
            "the file is larger than a song can be".into(),
        ));
    }
    if let Some(folder) = part.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(part)?);
    let mut body = answer.body_mut().with_config().limit(MAX_FILE + 1).reader();
    let mut buffer = vec![0u8; 64 * 1024];
    let (mut head, mut got) = (Vec::with_capacity(64), 0u64);
    let mut told = std::time::Instant::now();
    loop {
        if cancelled() {
            return Err(Trouble::Cancelled);
        }
        let n = body
            .read(&mut buffer)
            .map_err(|e| Trouble::Busy(format!("the file stopped coming in: {e}"), None))?;
        if n == 0 {
            break;
        }
        if head.len() < 64 {
            let take = (64 - head.len()).min(n);
            head.extend_from_slice(&buffer[..take]);
            // A web page or an error is not saved as a song.
            if head.len() >= 12 && kind_of(&head).is_none() {
                return Err(Trouble::Failed(
                    "the add-on's link did not give an audio file".into(),
                ));
            }
        }
        out.write_all(&buffer[..n])?;
        got += n as u64;
        if got > MAX_FILE {
            return Err(Trouble::Failed(
                "the file is larger than a song can be".into(),
            ));
        }
        if told.elapsed() >= Duration::from_millis(250) {
            progress(got, of);
            told = std::time::Instant::now();
        }
    }
    out.flush()?;
    drop(out);
    if of.is_some_and(|n| n != got) {
        return Err(Trouble::Failed("the file came in cut short".into()));
    }
    if kind_of(&head).is_none() {
        return Err(Trouble::Failed(
            "the add-on's link did not give an audio file".into(),
        ));
    }
    progress(got, Some(got));
    Ok((head, got))
}

/// A stream as its server answers it, to be relayed to the engine: the status, the headers the engine reads, and the
/// body.
pub struct Upstream {
    pub status: u16,
    pub content_type: Option<String>,
    pub length: Option<u64>,
    /// `Content-Range`, for an answer to a range.
    pub range: Option<String>,
    /// The server answers ranges (the engine can seek in the stream).
    pub ranges: bool,
    /// The server sent it compressed although it was asked not to: its length and ranges are not the bytes read.
    pub encoded: bool,
    pub body: Box<dyn Read + Send>,
}

/// Opens `url` to relay it, from `range` on ("bytes=N-") when one is asked for. The same rules as [`fetch`]: https
/// only (http on this computer), redirects followed to https only and only as near as `reach`, no file larger than a
/// song can be.
pub fn open_stream(url: &str, reach: Reach, range: Option<&str>) -> Result<Upstream, Trouble> {
    crate::addons::checked_url(url, "the stream's link")
        .map_err(|e| Trouble::Failed(e.in_words()))?;
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        // The engine reads as it plays once its own buffer is full: a long song keeps the stream open that long.
        .timeout_recv_body(Some(Duration::from_secs(4 * 3600)))
        .max_redirects(5)
        .https_only(!is_loopback(url))
        .build();
    let agent = Guarded::agent(reach, config);
    // As it is: a compressed answer's length and ranges are not those of the bytes read from it.
    let mut asked = agent.get(url).header("Accept-Encoding", "identity");
    if let Some(range) = range {
        asked = asked.header("Range", range);
    }
    let answer = asked.call().map_err(|e| {
        let why = crate::reach::refusal(&e).unwrap_or_else(|| e.to_string());
        Trouble::Failed(format!("the stream could not be opened: {why}"))
    })?;
    let text = |name: &str| {
        answer
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let length = text("content-length").and_then(|v| v.parse::<u64>().ok());
    if length.is_some_and(|n| n > MAX_FILE) {
        return Err(Trouble::Failed(
            "the file is larger than a song can be".into(),
        ));
    }
    let encoded = text("content-encoding")
        .is_some_and(|v| !v.trim().is_empty() && !v.trim().eq_ignore_ascii_case("identity"));
    Ok(Upstream {
        status: answer.status().as_u16(),
        content_type: text("content-type"),
        length: length.filter(|_| !encoded),
        range: text("content-range").filter(|_| !encoded),
        ranges: !encoded && text("accept-ranges").is_some_and(|v| v.eq_ignore_ascii_case("bytes")),
        encoded,
        body: Box::new(
            answer
                .into_body()
                .into_with_config()
                .limit(MAX_FILE + 1)
                .reader(),
        ),
    })
}

fn is_loopback(url: &str) -> bool {
    url::Url::parse(url).ok().is_some_and(|u| match u.host() {
        Some(url::Host::Domain(name)) => name == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    })
}

/// A cover from `url` (https, or this computer), as long as it is a JPEG or PNG and not huge, fetched no nearer than
/// `reach`.
pub fn fetch_cover(url: &str, reach: Reach) -> Option<Vec<u8>> {
    crate::addons::checked_url(url, "the cover").ok()?;
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(5)
        .https_only(!is_loopback(url))
        .build();
    let agent = Guarded::agent(reach, config);
    let mut answer = agent.get(url).call().ok()?;
    if !(200..300).contains(&answer.status().as_u16()) {
        return None;
    }
    let bytes = answer
        .body_mut()
        .with_config()
        .limit(MAX_COVER)
        .read_to_vec()
        .ok()?;
    let picture = bytes.starts_with(&[0xFF, 0xD8, 0xFF]) || bytes.starts_with(b"\x89PNG\r\n\x1a\n");
    picture.then_some(bytes)
}

/// Writes what the library knows the song by into the file's tags (title, artist, album, track number, ISRC), and
/// `cover` as its front cover when it has none. Answers whether the file has a cover now.
pub fn tag(path: &Path, song: &Song, cover: Option<&[u8]>) -> Result<bool, Trouble> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::tag::{Accessor, ItemKey, Tag};

    let mut file = lofty::read_from_path(path)
        .map_err(|e| Trouble::Failed(format!("the saved file cannot be read as audio: {e}")))?;
    let kind = file.primary_tag_type();
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.primary_tag_mut().expect("a tag was just put in");
    tag.set_title(song.title.clone());
    tag.set_artist(song.artist.clone());
    if let Some(album) = &song.album {
        tag.set_album(album.clone());
    }
    if let Some(n) = song.track_no.filter(|n| *n > 0) {
        tag.set_track(n);
    }
    if let Some(isrc) = &song.isrc {
        tag.insert_text(ItemKey::Isrc, isrc.clone());
    }
    let mut has_cover = !tag.pictures().is_empty();
    if !has_cover && let Some(bytes) = cover {
        let mime = if bytes.starts_with(b"\x89PNG") {
            MimeType::Png
        } else {
            MimeType::Jpeg
        };
        tag.push_picture(
            Picture::unchecked(bytes.to_vec())
                .pic_type(PictureType::CoverFront)
                .mime_type(mime)
                .build(),
        );
        has_cover = true;
    }
    file.save_to_path(path, WriteOptions::default())
        .map_err(|e| Trouble::Failed(format!("the tags could not be written: {e}")))?;
    Ok(has_cover)
}

/// What a saved file is, as a scan reads it: its format, size and time.
pub fn read(path: &Path) -> Result<NewTrack, Trouble> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    crate::scan::read_track(path, &path.to_string_lossy(), meta.len(), mtime, now)
        .map_err(Trouble::Failed)
}

/// Why an add-on gave no download, in the list's words.
pub fn said(e: &AddonError) -> String {
    match e.code {
        Code::NotFound => "the add-on does not have it".into(),
        Code::QualityUnavailable => {
            "the add-on does not have it in a quality that can be saved".into()
        }
        _ => e.in_words(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_on_every_system() {
        assert_eq!(safe_name("AC/DC: Live?"), "AC_DC_ Live_");
        assert_eq!(safe_name("  The   End...  "), "The End");
        assert_eq!(safe_name("con"), "_con");
        assert_eq!(safe_name("COM1.flac"), "_COM1.flac");
        assert_eq!(safe_name("Common"), "Common");
        assert_eq!(safe_name("..."), "_");
        assert_eq!(safe_name(&"x".repeat(200)).len(), NAME_CAP);
    }

    #[test]
    fn a_song_goes_under_its_artist_and_album_and_never_over_another() {
        let root =
            std::env::temp_dir().join(format!("stash-downloads-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let song = Song {
            title: "Song".into(),
            artist: "The Band".into(),
            album: Some("Record".into()),
            track_no: Some(3),
            isrc: None,
        };
        let first = path_for(&root, &song, "flac");
        assert_eq!(
            first,
            root.join("The Band").join("Record").join("03 Song.flac")
        );
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(
            path_for(&root, &song, "flac"),
            root.join("The Band")
                .join("Record")
                .join("03 Song (2).flac")
        );
        let loose = Song {
            album: None,
            track_no: None,
            ..song
        };
        assert_eq!(
            path_for(&root, &loose, "mp3"),
            root.join("The Band").join("Song.mp3")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn audio_is_told_from_anything_else_by_its_first_bytes() {
        assert_eq!(kind_of(b"fLaC\0\0\0\x22"), Some("flac"));
        assert_eq!(kind_of(b"ID3\x04\0\0"), Some("mp3"));
        assert_eq!(kind_of(&[0xFF, 0xFB, 0x90, 0x64]), Some("mp3"));
        assert_eq!(kind_of(&[0xFF, 0xF1, 0x50, 0x80]), Some("aac"));
        assert_eq!(kind_of(b"\0\0\0\x20ftypM4A "), Some("m4a"));
        assert_eq!(
            kind_of(b"OggS\0\x02\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x01\x13OpusHead"),
            Some("opus")
        );
        assert_eq!(kind_of(b"RIFF\x24\0\0\0WAVEfmt "), Some("wav"));
        assert_eq!(kind_of(b"<!DOCTYPE html>"), None);
        assert_eq!(kind_of(b"{\"error\":{}}"), None);
        assert_eq!(kind_of(b"#EXTM3U\n"), None);
    }
}
