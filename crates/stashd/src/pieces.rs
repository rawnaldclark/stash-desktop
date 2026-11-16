//! Playlists of pieces (HLS, DASH), rewritten so the engine reaches every piece through the relay. The engine fetches
//! a playlist's pieces by itself, following redirects and looking names up on its own: given an add-on's playlist as
//! it is, it would go wherever the playlist (or a redirect, or a name's answer) says, the listener's network and
//! this computer too. So the relay fetches the playlist (held to the add-on's reach, as every connection for it is)
//! and hands the engine a copy in which every address names the relay: the pieces are then fetched by the relay too,
//! held the same way.
//!
//! - HLS: every address is made whole (against where the playlist came from) and named through the relay: the lines
//!   that name a playlist or a piece, and the `URI="…"` of the tags (keys, maps, other renditions).
//! - DASH: the addresses written whole (`https://…`, `//…`) or from the server's root (`/…`) are named through the
//!   relay: `BaseURL` and `Location`, and the `media`, `initialization`, `sourceURL`, `index` and `href` attributes.
//!   Addresses relative to where the playlist is need nothing: the playlist's own address is the relay's, path and
//!   all, so they resolve to the relay as they are. (A root address under a `BaseURL` of another server is taken
//!   from the playlist's server: rare, and the engine then asks the wrong server, never another place.)
//!
//! An address of another kind than http, https or data (a file, a key system's) refuses the whole playlist.

use url::Url;

/// What kind of playlist a stream is, from what its server says it is, its address, and how it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Hls,
    Dash,
}

/// The content types a playlist of pieces comes with.
pub(crate) const PLAYLISTS: [&str; 5] = [
    "application/vnd.apple.mpegurl",
    "application/x-mpegurl",
    "audio/mpegurl",
    "audio/x-mpegurl",
    "application/dash+xml",
];

/// Whether a stream is a playlist, by its content type (as the server said it, parameters left out) or its address's
/// ending. Its text then tells which kind.
pub(crate) fn is_playlist(content_type: &str, path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    PLAYLISTS.contains(&content_type) || [".m3u8", ".m3u", ".mpd"].iter().any(|e| path.ends_with(e))
}

/// Which kind of playlist `text` is; none when it is neither.
pub(crate) fn kind_of(text: &str) -> Option<Kind> {
    let text = text.trim_start_matches('\u{feff}').trim_start();
    if text.starts_with("#EXTM3U") {
        return Some(Kind::Hls);
    }
    let head: String = text.chars().take(2048).collect();
    (head.starts_with('<') && head.contains("<MPD")).then_some(Kind::Dash)
}

/// The most servers one playlist may name: a playlist naming more is refused.
pub(crate) const MOST_SERVERS: usize = 32;

/// `text`, a playlist of `kind` that came from `at` (after any redirects), with every address it names that the
/// engine would otherwise fetch from where it is named through the relay: `relay(address)` gives the relay's address
/// for a whole http or https address. Refused, in words, when it names an address of another kind.
pub(crate) fn rewrite(
    kind: Kind,
    text: &str,
    at: &Url,
    relay: &mut dyn FnMut(&Url) -> String,
) -> Result<String, String> {
    match kind {
        Kind::Hls => hls(text, at, relay),
        Kind::Dash => dash(text, at, relay),
    }
}

fn scheme_of(text: &str) -> Option<&str> {
    let end = text.find(':')?;
    let scheme = &text[..end];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
        .then_some(scheme)
}

/// One address the playlist names, made whole against `at` and named through the relay; a `data:` address is kept.
fn named(address: &str, at: &Url, relay: &mut dyn FnMut(&Url) -> String) -> Result<String, String> {
    let address = address.trim();
    if scheme_of(address).is_some_and(|s| s.eq_ignore_ascii_case("data")) {
        return Ok(address.to_string());
    }
    let mut whole = at
        .join(address)
        .map_err(|e| format!("the playlist names an address that is not one ({e})"))?;
    match whole.scheme() {
        "http" | "https" => {
            whole.set_fragment(None);
            Ok(relay(&whole))
        }
        other => Err(format!(
            "the playlist names a {other} address, which Stash does not fetch"
        )),
    }
}

fn hls(text: &str, at: &Url, relay: &mut dyn FnMut(&Url) -> String) -> Result<String, String> {
    let mut out = String::with_capacity(text.len() + 256);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
        } else if trimmed.starts_with('#') {
            out.push_str(&hls_tag(line, at, relay)?);
        } else {
            out.push_str(&named(trimmed, at, relay)?);
        }
        out.push('\n');
    }
    Ok(out)
}

/// A tag line, its `URI="…"` attributes named through the relay.
fn hls_tag(line: &str, at: &Url, relay: &mut dyn FnMut(&Url) -> String) -> Result<String, String> {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(found) = rest.find("URI=\"") {
        // An attribute's name starts the line's attributes or follows a comma (not "XURI=").
        let before = &rest[..found];
        let starts = before
            .chars()
            .last()
            .is_none_or(|c| c == ',' || c == ':' || c.is_whitespace());
        let open = found + "URI=\"".len();
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(close) = rest.find('"') else {
            return Err("the playlist has a tag whose address is not closed".into());
        };
        if starts {
            out.push_str(&named(&rest[..close], at, relay)?);
        } else {
            out.push_str(&rest[..close]);
        }
        rest = &rest[close..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The DASH attributes that hold an address.
const DASH_ATTRIBUTES: [&str; 5] = ["media", "initialization", "sourceURL", "index", "href"];
/// The DASH elements whose text is an address.
const DASH_ELEMENTS: [&str; 2] = ["BaseURL", "Location"];

fn dash(text: &str, at: &Url, relay: &mut dyn FnMut(&Url) -> String) -> Result<String, String> {
    let mut out = String::with_capacity(text.len() + 512);
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        // Comments, declarations and processing instructions pass as they are.
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map(|e| e + 3).unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        let Some(close) = rest.find('>') else {
            out.push_str(rest);
            rest = "";
            break;
        };
        let tag = &rest[..=close];
        out.push_str(&dash_tag(tag, at, relay)?);
        rest = &rest[close + 1..];
        // An element whose text is an address: that text, up to the next tag.
        let name = tag_name(tag);
        if !tag.starts_with("</") && !tag.ends_with("/>") && DASH_ELEMENTS.contains(&name) {
            let end = rest.find('<').unwrap_or(rest.len());
            out.push_str(&dash_address(&rest[..end], at, relay)?);
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// An element's name without its namespace's prefix ("mpd:BaseURL" is "BaseURL").
fn tag_name(tag: &str) -> &str {
    let inner = tag.trim_start_matches('<').trim_start_matches('/');
    let end = inner
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(inner.len());
    let name = &inner[..end];
    name.rsplit(':').next().unwrap_or(name)
}

/// A tag, its address attributes named through the relay.
fn dash_tag(tag: &str, at: &Url, relay: &mut dyn FnMut(&Url) -> String) -> Result<String, String> {
    let mut out = String::with_capacity(tag.len());
    let mut rest = tag;
    // The next attribute: a name after whitespace, "=", and a quoted value.
    while let Some(eq) = rest.find('=') {
        let before = &rest[..eq];
        let name_start = before
            .rfind(|c: char| c.is_whitespace())
            .map(|i| i + 1)
            .unwrap_or(before.len());
        let name = before[name_start..].trim();
        let name = name.rsplit(':').next().unwrap_or(name);
        let after = &rest[eq + 1..];
        let quote = after.chars().next();
        let (Some(q @ ('"' | '\'')), true) = (quote, name_start < before.len()) else {
            out.push_str(&rest[..eq + 1]);
            rest = after;
            continue;
        };
        let value_start = eq + 2;
        let Some(len) = rest[value_start..].find(q) else {
            break;
        };
        out.push_str(&rest[..value_start]);
        let value = &rest[value_start..value_start + len];
        if DASH_ATTRIBUTES.contains(&name) {
            out.push_str(&dash_address(value, at, relay)?);
        } else {
            out.push_str(value);
        }
        rest = &rest[value_start + len..];
    }
    out.push_str(rest);
    Ok(out)
}

/// An address in a DASH playlist (as written in XML): named through the relay when it is whole or from the server's
/// root, kept when it is relative to where the playlist is.
fn dash_address(
    written: &str,
    at: &Url,
    relay: &mut dyn FnMut(&Url) -> String,
) -> Result<String, String> {
    let address = unescape(written);
    let trimmed = address.trim();
    let whole = scheme_of(trimmed).is_some() || trimmed.starts_with('/');
    if trimmed.is_empty() || !whole {
        return Ok(written.to_string());
    }
    Ok(escape(&named(trimmed, at, relay)?))
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relayed(url: &Url) -> String {
        format!(
            "http://127.0.0.1:9/relay/o/[{}]{}",
            url.origin().ascii_serialization(),
            &url[url::Position::BeforePath..]
        )
    }

    #[test]
    fn every_address_of_an_hls_playlist_goes_through_the_relay() {
        let at = Url::parse("https://cdn.example.org/a/b/master.m3u8?t=1").unwrap();
        let text = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x1\n#EXT-X-MAP:URI=\"/init.mp4\"\n#EXTINF:4.0,\nseg1.m4s\n#EXTINF:4.0,\nhttps://other.example.net/x/seg2.m4s?sig=a&b=c\n\n#EXTINF:4,\nhttp://192.168.1.1/admin\n#EXT-X-SESSION-DATA:DATA-ID=\"x\",URI=\"data:text/plain,hi\"\n";
        let out = rewrite(Kind::Hls, text, &at, &mut relayed).unwrap();
        assert_eq!(
            out,
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-KEY:METHOD=AES-128,URI=\"http://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/key.bin\",IV=0x1\n#EXT-X-MAP:URI=\"http://127.0.0.1:9/relay/o/[https://cdn.example.org]/init.mp4\"\n#EXTINF:4.0,\nhttp://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/seg1.m4s\n#EXTINF:4.0,\nhttp://127.0.0.1:9/relay/o/[https://other.example.net]/x/seg2.m4s?sig=a&b=c\n\n#EXTINF:4,\nhttp://127.0.0.1:9/relay/o/[http://192.168.1.1]/admin\n#EXT-X-SESSION-DATA:DATA-ID=\"x\",URI=\"data:text/plain,hi\"\n"
        );
        // An address of another kind refuses the playlist.
        for other in [
            "file:///C:/secret.m4s",
            "#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://key\"",
        ] {
            let text = format!("#EXTM3U\n{other}\n");
            assert!(
                rewrite(Kind::Hls, &text, &at, &mut relayed).is_err(),
                "{other}"
            );
        }
    }

    #[test]
    fn a_dash_playlist_s_whole_and_root_addresses_go_through_the_relay() {
        let at = Url::parse("https://cdn.example.org/m/song.mpd").unwrap();
        let text = r#"<?xml version="1.0"?>
<!-- <BaseURL>http://ignored.example/</BaseURL> -->
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" xmlns:xlink="http://www.w3.org/1999/xlink" type="static">
  <BaseURL>https://media.example.net/a/</BaseURL>
  <Location>/m/song.mpd</Location>
  <Period xlink:href="https://other.example/period.xml">
    <AdaptationSet>
      <Representation id="flac" bandwidth="1">
        <BaseURL>rel/</BaseURL>
        <SegmentTemplate media="http://10.0.0.2/seg-$Number%05d$.mp4?a=1&amp;b=2" initialization='init-$RepresentationID$.mp4' startNumber="1"/>
        <SegmentList><SegmentURL media="/abs/1.mp4"/></SegmentList>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let out = rewrite(Kind::Dash, text, &at, &mut relayed).unwrap();
        for expected in [
            "<BaseURL>http://127.0.0.1:9/relay/o/[https://media.example.net]/a/</BaseURL>",
            "<Location>http://127.0.0.1:9/relay/o/[https://cdn.example.org]/m/song.mpd</Location>",
            r#"xlink:href="http://127.0.0.1:9/relay/o/[https://other.example]/period.xml""#,
            "<BaseURL>rel/</BaseURL>",
            r#"media="http://127.0.0.1:9/relay/o/[http://10.0.0.2]/seg-$Number%05d$.mp4?a=1&amp;b=2""#,
            "initialization='init-$RepresentationID$.mp4'",
            r#"<SegmentURL media="http://127.0.0.1:9/relay/o/[https://cdn.example.org]/abs/1.mp4"/>"#,
            // What is not an address is left as it is: the namespaces, comments, the rest.
            r#"xmlns:xlink="http://www.w3.org/1999/xlink""#,
            "<!-- <BaseURL>http://ignored.example/</BaseURL> -->",
            r#"startNumber="1""#,
        ] {
            assert!(out.contains(expected), "{expected}\n{out}");
        }
        assert!(
            rewrite(
                Kind::Dash,
                r#"<MPD><BaseURL>file:///etc/</BaseURL></MPD>"#,
                &at,
                &mut relayed
            )
            .is_err()
        );
    }

    #[test]
    fn a_playlist_is_told_by_its_type_its_address_and_its_start() {
        assert!(is_playlist("application/vnd.apple.mpegurl", "/x"));
        assert!(is_playlist("application/octet-stream", "/x/master.M3U8"));
        assert!(is_playlist("", "/song.mpd"));
        assert!(!is_playlist("audio/flac", "/song.flac"));
        assert_eq!(kind_of("\u{feff}#EXTM3U\n"), Some(Kind::Hls));
        assert_eq!(kind_of("<?xml version=\"1.0\"?>\n<MPD>"), Some(Kind::Dash));
        assert_eq!(kind_of("fLaC"), None);
    }
}
