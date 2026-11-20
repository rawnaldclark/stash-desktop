//! A playlist of pieces (DASH, HLS) played as one stream, for players that only play a whole file: a Subsonic app
//! asking for a song an add-on gives as pieces. The playlist is read, one rendition chosen (the best audio one), and
//! its pieces fetched one after another and passed on back to back: an initialization piece and the media pieces
//! after it make one fragmented MP4 (or MPEG-TS, or AAC) stream, which such players read as a file.
//!
//! The playlist is the core relay's copy ([`crate::pieces`]), so every address in it names the relay, which holds
//! each fetch to the add-on's reach. A piece named anywhere else (another server, or another kind of address) refuses
//! the whole playlist: nothing is fetched from where an add-on's playlist alone says.
//!
//! Not played this way: protected (encrypted) pieces, pieces given as byte ranges of one file, playlists of more than
//! one part (DASH periods). The stream has no length, so a player cannot ask for it from the middle; a song made
//! smaller ([`crate::transcode`]) starts from the piece a time falls in, when the playlist says when each one starts.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::time::Duration;

use url::Url;

use crate::pieces::Kind;

/// The most pieces one song may be (a song of hours in pieces of a second).
const MOST_PIECES: usize = 20_000;

/// What a playlist comes to.
#[derive(Debug, PartialEq)]
pub(crate) enum Flat {
    /// The pieces, in order, and the content type of the stream they make. The first `head` pieces start every
    /// stream (an initialization piece); `starts` says when each piece after them starts, in seconds from the
    /// song's start, when the playlist says.
    Pieces {
        pieces: Vec<Url>,
        content_type: String,
        head: usize,
        starts: Option<Vec<f64>>,
    },
    /// A playlist of renditions (HLS): the chosen one's playlist, to be read in turn.
    Rendition(Url),
}

/// The pieces of `text`, a playlist of `kind` read from `at` (where it landed, after any redirects).
pub(crate) fn flatten(kind: Kind, text: &str, at: &Url) -> Result<Flat, String> {
    let flat = match kind {
        Kind::Dash => dash(text, at)?,
        Kind::Hls => hls(text, at)?,
    };
    // Only what the relay names: the playlist's own server.
    let own = at.origin();
    let named = match &flat {
        Flat::Pieces { pieces, .. } => pieces.iter().collect::<Vec<_>>(),
        Flat::Rendition(url) => vec![url],
    };
    if named.iter().any(|u| u.origin() != own) {
        return Err("the playlist names a piece on another server".into());
    }
    if let Flat::Pieces { pieces, .. } = &flat {
        if pieces.is_empty() {
            return Err("the playlist names no pieces".into());
        }
        if pieces.len() > MOST_PIECES {
            return Err("the playlist names too many pieces".into());
        }
    }
    Ok(flat)
}

/// The pieces to play from `at` seconds into the song: those every stream starts with, then from the piece `at` falls
/// in; and how far into what they make `at` is. Without the pieces' times, all of them, and `at` as it is.
pub(crate) fn from_time(
    pieces: Vec<Url>,
    head: usize,
    starts: Option<&[f64]>,
    at: f64,
) -> (Vec<Url>, f64) {
    let at = at.max(0.0);
    let Some(starts) = starts.filter(|s| !s.is_empty() && s.len() + head == pieces.len()) else {
        return (pieces, at);
    };
    let k = starts.iter().rposition(|s| *s <= at).unwrap_or(0);
    if k == 0 {
        return (pieces, at);
    }
    let mut kept = pieces[..head].to_vec();
    kept.extend_from_slice(&pieces[head + k..]);
    (kept, at - starts[k])
}

// ---- reading XML, as much as a DASH playlist needs

#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Node>,
    text: String,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
    fn kids(&self, name: &'static str) -> impl Iterator<Item = &Node> {
        self.kids.iter().filter(move |k| k.name == name)
    }
    fn kid(&self, name: &'static str) -> Option<&Node> {
        self.kids(name).next()
    }
    fn any(&self, name: &str) -> bool {
        self.name == name || self.kids.iter().any(|k| k.any(name))
    }
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// An element's name without its namespace's prefix.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// The document's root element.
fn parse_xml(text: &str) -> Result<Node, String> {
    let bad = || "the playlist is not XML Stash can read".to_string();
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let between = &rest[..open];
        if !between.trim().is_empty() {
            stack
                .last_mut()
                .ok_or_else(bad)?
                .text
                .push_str(&unescape(between));
        }
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            let end = after.find("-->").ok_or_else(bad)?;
            rest = &after[end + 3..];
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            let end = rest.find('>').ok_or_else(bad)?;
            rest = &rest[end + 1..];
            continue;
        }
        // The tag's end: the first '>' outside a quoted value.
        let mut quote = None;
        let mut end = None;
        for (i, c) in rest.char_indices().skip(1) {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '>') => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let end = end.ok_or_else(bad)?;
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(closing) = tag.strip_prefix('/') {
            let node = stack.pop().ok_or_else(bad)?;
            if node.name != local(closing.trim()) || stack.is_empty() {
                return Err(bad());
            }
            stack.last_mut().ok_or_else(bad)?.kids.push(node);
            continue;
        }
        let empty = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
        let mut node = Node {
            name: local(&tag[..name_end]).to_string(),
            ..Node::default()
        };
        let mut attrs = &tag[name_end..];
        while let Some(eq) = attrs.find('=') {
            let name = local(attrs[..eq].trim()).to_string();
            let after = attrs[eq + 1..].trim_start();
            let q = after.chars().next().filter(|c| *c == '"' || *c == '\'');
            let Some(q) = q else { return Err(bad()) };
            let close = after[1..].find(q).ok_or_else(bad)?;
            node.attrs.push((name, unescape(&after[1..1 + close])));
            attrs = &after[close + 2..];
        }
        if empty {
            stack.last_mut().ok_or_else(bad)?.kids.push(node);
        } else {
            stack.push(node);
        }
    }
    let mut top = stack.pop().ok_or_else(bad)?;
    if !stack.is_empty() {
        return Err(bad());
    }
    top.kids.pop().ok_or_else(bad)
}

// ---- DASH

/// Seconds in an ISO 8601 duration as DASH writes them (`PT3M40.523S`, `P1DT2H`).
fn seconds(text: &str) -> Option<f64> {
    let rest = text.trim().strip_prefix('P')?;
    let (days, time) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };
    let mut total = 0.0;
    let mut add = |part: &str, units: &[(char, f64)]| -> Option<()> {
        let mut number = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' {
                number.push(c);
            } else {
                let (_, scale) = units.iter().find(|(u, _)| *u == c)?;
                total += number.parse::<f64>().ok()? * scale;
                number.clear();
            }
        }
        number.is_empty().then_some(())
    };
    add(
        days,
        &[
            ('Y', 365.0 * 86400.0),
            ('M', 30.0 * 86400.0),
            ('W', 7.0 * 86400.0),
            ('D', 86400.0),
        ],
    )?;
    add(time, &[('H', 3600.0), ('M', 60.0), ('S', 1.0)])?;
    Some(total)
}

/// A template's address for one piece: `$RepresentationID$`, `$Bandwidth$`, `$Number$`, `$Time$` (each may carry a
/// width, `$Number%05d$`), and `$$`.
fn fill(
    template: &str,
    id: &str,
    bandwidth: &str,
    number: u64,
    time: u64,
) -> Result<String, String> {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after
            .find('$')
            .ok_or("the playlist's address template is not closed")?;
        let token = &after[..end];
        let (name, width) = match token.split_once('%') {
            Some((name, format)) => {
                let digits = format
                    .strip_prefix('0')
                    .unwrap_or(format)
                    .strip_suffix('d')
                    .ok_or("the playlist's address template has a width Stash does not know")?;
                (name, digits.parse::<usize>().unwrap_or(0))
            }
            None => (token, 0),
        };
        let value = match name {
            "" => "$".to_string(),
            "RepresentationID" => id.to_string(),
            "Bandwidth" => bandwidth.to_string(),
            "Number" => number.to_string(),
            "Time" => time.to_string(),
            other => return Err(format!("the playlist's address template names ${other}$")),
        };
        if value.len() < width {
            out.push_str(&"0".repeat(width - value.len()));
        }
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The address at `base` joined with the `BaseURL` of `node`, if it has one.
fn based(base: &Url, node: &Node) -> Result<Url, String> {
    match node
        .kid("BaseURL")
        .map(|b| b.text.trim())
        .filter(|t| !t.is_empty())
    {
        Some(text) => base
            .join(text)
            .map_err(|_| "the playlist names an address that is not one".to_string()),
        None => Ok(base.clone()),
    }
}

fn is_audio(set: &Node, rep: &Node) -> bool {
    let kind = |n: &Node| {
        n.attr("contentType").map(str::to_string).or_else(|| {
            n.attr("mimeType")
                .map(|m| m.split('/').next().unwrap_or("").to_string())
        })
    };
    match kind(rep).or_else(|| kind(set)) {
        Some(k) => k == "audio",
        None => true,
    }
}

fn dash(text: &str, at: &Url) -> Result<Flat, String> {
    let mpd = parse_xml(text)?;
    if mpd.name != "MPD" {
        return Err("the playlist is not a DASH playlist".into());
    }
    if mpd.any("ContentProtection") {
        return Err("the song's pieces are protected".into());
    }
    if mpd.attr("type") == Some("dynamic") {
        return Err("the playlist is of a live stream".into());
    }
    let mut periods = mpd.kids("Period");
    let period = periods.next().ok_or("the playlist has no part")?;
    if periods.next().is_some() {
        return Err("the playlist has more than one part".into());
    }
    let length = period
        .attr("duration")
        .or_else(|| mpd.attr("mediaPresentationDuration"))
        .and_then(seconds);
    // The best audio rendition.
    let (set, rep) = period
        .kids("AdaptationSet")
        .flat_map(|set| set.kids("Representation").map(move |rep| (set, rep)))
        .filter(|(set, rep)| is_audio(set, rep))
        .max_by_key(|(_, rep)| {
            rep.attr("bandwidth")
                .and_then(|b| b.parse::<u64>().ok())
                .unwrap_or(0)
        })
        .ok_or("the playlist has no audio")?;
    let base = based(&based(&based(&based(at, &mpd)?, period)?, set)?, rep)?;
    let content_type = rep
        .attr("mimeType")
        .or_else(|| set.attr("mimeType"))
        .unwrap_or("audio/mp4")
        .to_string();
    let join = |address: &str| {
        base.join(address)
            .map_err(|_| "the playlist names an address that is not one".to_string())
    };
    let id = rep.attr("id").unwrap_or("");
    let bandwidth = rep.attr("bandwidth").unwrap_or("");

    let mut pieces = Vec::new();
    let mut head = 0;
    let mut starts = Some(Vec::new());
    if let Some(template) = rep
        .kid("SegmentTemplate")
        .or_else(|| set.kid("SegmentTemplate"))
    {
        let attr = |name: &str| template.attr(name);
        let timescale: u64 = attr("timescale")
            .and_then(|t| t.parse().ok())
            .filter(|t| *t > 0)
            .unwrap_or(1);
        let mut number: u64 = attr("startNumber")
            .and_then(|n| n.parse().ok())
            .unwrap_or(1);
        if let Some(init) = attr("initialization") {
            pieces.push(join(&fill(init, id, bandwidth, number, 0)?)?);
            head = 1;
        }
        let media = attr("media").ok_or("the playlist's pieces have no address")?;
        let end = length.map(|s| (s * timescale as f64).round() as u64);
        let at = |ticks: u64| ticks as f64 / timescale as f64;
        if let Some(timeline) = template.kid("SegmentTimeline") {
            let mut time: u64 = 0;
            let mut first: Option<u64> = None;
            for s in timeline.kids("S") {
                if let Some(t) = s.attr("t").and_then(|t| t.parse().ok()) {
                    time = t;
                }
                let d: u64 = s
                    .attr("d")
                    .and_then(|d| d.parse().ok())
                    .filter(|d| *d > 0)
                    .ok_or("the playlist has a piece with no length")?;
                let r: i64 = s.attr("r").and_then(|r| r.parse().ok()).unwrap_or(0);
                let times = if r < 0 {
                    let end =
                        end.ok_or("the playlist repeats a piece without saying how long it is")?;
                    end.saturating_sub(time).div_ceil(d)
                } else {
                    r as u64 + 1
                };
                for _ in 0..times {
                    if pieces.len() > MOST_PIECES {
                        return Err("the playlist names too many pieces".into());
                    }
                    pieces.push(join(&fill(media, id, bandwidth, number, time)?)?);
                    let first = *first.get_or_insert(time);
                    if let Some(starts) = &mut starts {
                        starts.push(at(time.saturating_sub(first)));
                    }
                    number += 1;
                    time += d;
                }
            }
        } else {
            let d: u64 = attr("duration")
                .and_then(|d| d.parse().ok())
                .filter(|d| *d > 0)
                .ok_or("the playlist's pieces have no length")?;
            let end = end.ok_or("the playlist does not say how long it is")?;
            let count = end.div_ceil(d);
            if count as usize > MOST_PIECES {
                return Err("the playlist names too many pieces".into());
            }
            for i in 0..count {
                pieces.push(join(&fill(media, id, bandwidth, number, i * d)?)?);
                if let Some(starts) = &mut starts {
                    starts.push(at(i * d));
                }
                number += 1;
            }
        }
    } else if let Some(list) = rep.kid("SegmentList").or_else(|| set.kid("SegmentList")) {
        if let Some(init) = list.kid("Initialization") {
            if init.attr("range").is_some() {
                return Err("the playlist's pieces are parts of one file".into());
            }
            pieces.push(join(
                init.attr("sourceURL")
                    .ok_or("the playlist's first piece has no address")?,
            )?);
            head = 1;
        }
        // Even pieces, when the list says how long each is.
        let timescale: f64 = list
            .attr("timescale")
            .and_then(|t| t.parse().ok())
            .filter(|t: &f64| *t > 0.0)
            .unwrap_or(1.0);
        let d: Option<f64> = list
            .attr("duration")
            .and_then(|d| d.parse().ok())
            .filter(|d: &f64| *d > 0.0);
        if d.is_none() {
            starts = None;
        }
        for (i, s) in list.kids("SegmentURL").enumerate() {
            if s.attr("mediaRange").is_some() {
                return Err("the playlist's pieces are parts of one file".into());
            }
            pieces.push(join(
                s.attr("media")
                    .ok_or("the playlist has a piece with no address")?,
            )?);
            if let (Some(starts), Some(d)) = (&mut starts, d) {
                starts.push(i as f64 * d / timescale);
            }
        }
    } else {
        // One whole file (SegmentBase, or nothing): its address.
        pieces.push(base.clone());
        starts = None;
    }
    Ok(Flat::Pieces {
        pieces,
        content_type,
        head,
        starts,
    })
}

// ---- HLS

/// The value of attribute `name` in an HLS tag's attribute list.
fn hls_attr<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let list = line.split_once(':')?.1;
    let mut rest = list;
    while !rest.is_empty() {
        let eq = rest.find('=')?;
        let key = rest[..eq].trim();
        let after = &rest[eq + 1..];
        let (value, next) = if let Some(quoted) = after.strip_prefix('"') {
            let close = quoted.find('"')?;
            (
                &quoted[..close],
                quoted[close + 1..].trim_start_matches(','),
            )
        } else {
            let comma = after.find(',').unwrap_or(after.len());
            (&after[..comma], after.get(comma + 1..).unwrap_or(""))
        };
        if key == name {
            return Some(value);
        }
        rest = next;
    }
    None
}

fn hls(text: &str, at: &Url) -> Result<Flat, String> {
    let join = |address: &str| {
        at.join(address.trim())
            .map_err(|_| "the playlist names an address that is not one".to_string())
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    // A playlist of renditions: the one of the most bandwidth.
    if lines.iter().any(|l| l.starts_with("#EXT-X-STREAM-INF")) {
        let mut best: Option<(u64, &str)> = None;
        for pair in lines.windows(2) {
            if pair[0].starts_with("#EXT-X-STREAM-INF") && !pair[1].starts_with('#') {
                let bandwidth = hls_attr(pair[0], "BANDWIDTH")
                    .and_then(|b| b.parse().ok())
                    .unwrap_or(0);
                if best.is_none_or(|(b, _)| bandwidth > b) {
                    best = Some((bandwidth, pair[1]));
                }
            }
        }
        let (_, address) = best.ok_or("the playlist names no rendition")?;
        return Ok(Flat::Rendition(join(address)?));
    }
    let mut pieces = Vec::new();
    let mut map = false;
    // When each piece starts, from each one's #EXTINF (a piece without one leaves the times unknown).
    let (mut starts, mut time, mut length) = (Some(Vec::new()), 0.0f64, None::<f64>);
    for line in &lines {
        if let Some(tag) = line.strip_prefix('#') {
            if let Some(info) = tag.strip_prefix("EXTINF:") {
                length = info
                    .split(',')
                    .next()
                    .and_then(|d| d.trim().parse().ok())
                    .filter(|d: &f64| d.is_finite() && *d >= 0.0);
            } else if tag.starts_with("EXT-X-KEY") || tag.starts_with("EXT-X-SESSION-KEY") {
                if hls_attr(line, "METHOD").is_some_and(|m| m != "NONE") {
                    return Err("the song's pieces are protected".into());
                }
            } else if tag.starts_with("EXT-X-BYTERANGE") {
                return Err("the playlist's pieces are parts of one file".into());
            } else if tag.starts_with("EXT-X-MAP") {
                if map {
                    return Err("the playlist changes its first piece midway".into());
                }
                if hls_attr(line, "BYTERANGE").is_some() {
                    return Err("the playlist's pieces are parts of one file".into());
                }
                pieces.push(join(
                    hls_attr(line, "URI").ok_or("the playlist's first piece has no address")?,
                )?);
                map = true;
            }
        } else {
            pieces.push(join(line)?);
            match (&mut starts, length.take()) {
                (Some(starts), Some(d)) => {
                    starts.push(time);
                    time += d;
                }
                _ => starts = None,
            }
        }
    }
    let ending = pieces
        .last()
        .and_then(|u| u.path().rsplit('.').next().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    let content_type = if map {
        "audio/mp4"
    } else {
        match ending.as_str() {
            "ts" => "video/mp2t",
            "aac" => "audio/aac",
            "mp3" => "audio/mpeg",
            "m4s" | "mp4" | "m4a" => "audio/mp4",
            _ => "application/octet-stream",
        }
    };
    Ok(Flat::Pieces {
        pieces,
        content_type: content_type.to_string(),
        head: usize::from(map),
        starts,
    })
}

// ---- the pieces, back to back

/// The pieces, fetched one after another as they are read.
pub(crate) struct Joined {
    agent: ureq::Agent,
    left: VecDeque<Url>,
    now: Option<Box<dyn Read + Send>>,
}

impl Joined {
    pub(crate) fn new(agent: ureq::Agent, pieces: Vec<Url>) -> Self {
        Self {
            agent,
            left: pieces.into(),
            now: None,
        }
    }

    fn open(&self, url: &Url) -> io::Result<Box<dyn Read + Send>> {
        let mut why = String::new();
        // A piece that fails is asked for once more.
        for _ in 0..2 {
            match self.agent.get(url.as_str()).call() {
                Ok(response) if response.status().is_success() => {
                    return Ok(Box::new(response.into_body().into_reader()));
                }
                Ok(response) => why = format!("a piece answered {}", response.status().as_u16()),
                Err(e) => why = e.to_string(),
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        Err(io::Error::other(why))
    }
}

impl Read for Joined {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.now.is_none() {
                let Some(next) = self.left.pop_front() else {
                    return Ok(0);
                };
                self.now = Some(self.open(&next)?);
            }
            let n = self.now.as_mut().map_or(Ok(0), |r| r.read(buf))?;
            if n > 0 {
                return Ok(n);
            }
            self.now = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Url {
        Url::parse("http://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/song.mpd?t=1").unwrap()
    }

    fn pieces(flat: Flat) -> (Vec<String>, String) {
        match flat {
            Flat::Pieces {
                pieces,
                content_type,
                ..
            } => (pieces.iter().map(|u| u.to_string()).collect(), content_type),
            other => panic!("{other:?}"),
        }
    }

    const TIDAL: &str = r#"<?xml version='1.0' encoding='UTF-8'?><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT3M40.523S"><Period id="0"><AdaptationSet id="0" contentType="audio" mimeType="audio/mp4" segmentAlignment="true"><Representation id="FLAC,44100,16" codecs="flac" bandwidth="909641" audioSamplingRate="44100"><SegmentTemplate timescale="44100" initialization="init/$RepresentationID$.mp4" media="seg-$Number%03d$.mp4?x=1&amp;y=2" startNumber="1"><SegmentTimeline><S d="176128" r="54"/><S d="38042"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;

    #[test]
    fn a_dash_playlist_of_a_timeline_is_its_first_piece_and_each_numbered_piece() {
        let (p, kind) = pieces(flatten(Kind::Dash, TIDAL, &at()).unwrap());
        assert_eq!(kind, "audio/mp4");
        assert_eq!(p.len(), 1 + 56);
        assert_eq!(
            p[0],
            "http://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/init/FLAC,44100,16.mp4"
        );
        assert_eq!(
            p[1],
            "http://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/seg-001.mp4?x=1&y=2"
        );
        assert_eq!(
            p[56],
            "http://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/seg-056.mp4?x=1&y=2"
        );
    }

    #[test]
    fn a_dash_playlist_of_even_pieces_is_counted_from_its_length_and_the_best_audio_is_chosen() {
        let text = r#"<MPD mediaPresentationDuration="PT10S"><Period><BaseURL>/root/</BaseURL>
            <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="9000000"/></AdaptationSet>
            <AdaptationSet mimeType="audio/mp4"><SegmentTemplate timescale="1000" duration="4000" initialization="$RepresentationID$/i.mp4" media="$RepresentationID$/$Time$.m4s"/>
              <Representation id="lo" bandwidth="96000"/><Representation id="hi" bandwidth="320000"/></AdaptationSet></Period></MPD>"#;
        let (p, _) = pieces(flatten(Kind::Dash, text, &at()).unwrap());
        assert_eq!(
            p,
            [
                "http://127.0.0.1:9/root/hi/i.mp4",
                "http://127.0.0.1:9/root/hi/0.m4s",
                "http://127.0.0.1:9/root/hi/4000.m4s",
                "http://127.0.0.1:9/root/hi/8000.m4s"
            ]
        );
        // A repeat to the end, with a start time.
        let text = r#"<MPD mediaPresentationDuration="PT9S"><Period><AdaptationSet><Representation id="a" bandwidth="1"><SegmentTemplate timescale="1" media="$Time$"><SegmentTimeline><S t="0" d="4" r="-1"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;
        let (p, _) = pieces(flatten(Kind::Dash, text, &at()).unwrap());
        assert_eq!(p.len(), 3);
        // A whole file.
        let text = r#"<MPD><Period><AdaptationSet mimeType="audio/mp4"><Representation id="a"><BaseURL>song.m4a</BaseURL><SegmentBase indexRange="0-99"/></Representation></AdaptationSet></Period></MPD>"#;
        assert_eq!(
            pieces(flatten(Kind::Dash, text, &at()).unwrap()).0,
            ["http://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/song.m4a"]
        );
    }

    #[test]
    fn a_playlist_that_cannot_be_played_whole_or_names_elsewhere_is_refused() {
        for text in [
            // Another server.
            r#"<MPD><Period><AdaptationSet><Representation id="a"><BaseURL>https://cdn.example.org/x.m4a</BaseURL></Representation></AdaptationSet></Period></MPD>"#,
            // Protected.
            r#"<MPD><Period><AdaptationSet><ContentProtection schemeIdUri="x"/><Representation id="a"/></AdaptationSet></Period></MPD>"#,
            // Two parts.
            r#"<MPD><Period/><Period/></MPD>"#,
            // Ranges of one file.
            r#"<MPD><Period><AdaptationSet><Representation id="a"><SegmentList><SegmentURL mediaRange="0-9"/></SegmentList></Representation></AdaptationSet></Period></MPD>"#,
            // Not closed.
            r#"<MPD><Period>"#,
        ] {
            assert!(flatten(Kind::Dash, text, &at()).is_err(), "{text}");
        }
        for text in [
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\na.ts\n",
            "#EXTM3U\n#EXT-X-BYTERANGE:100@0\na.ts\n",
            "#EXTM3U\nhttp://192.168.1.1/a.ts\n",
            "#EXTM3U\nfile:///C:/a.ts\n",
        ] {
            assert!(flatten(Kind::Hls, text, &at()).is_err(), "{text}");
        }
    }

    #[test]
    fn an_hls_playlist_is_its_map_and_pieces_or_its_best_rendition() {
        let text = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-KEY:METHOD=NONE\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\n1.m4s\n#EXTINF:4,\n/x/2.m4s\n#EXT-X-ENDLIST\n";
        let (p, kind) = pieces(flatten(Kind::Hls, text, &at()).unwrap());
        assert_eq!(kind, "audio/mp4");
        assert_eq!(
            p,
            [
                "http://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/init.mp4",
                "http://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/1.m4s",
                "http://127.0.0.1:9/x/2.m4s"
            ]
        );
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.5\"\nlo.m3u8\n#EXT-X-STREAM-INF:CODECS=\"mp4a.40.2\",BANDWIDTH=256000\nhi.m3u8\n";
        assert_eq!(
            flatten(Kind::Hls, text, &at()).unwrap(),
            Flat::Rendition(
                Url::parse("http://127.0.0.1:9/relay/o/[https://cdn.example.org]/a/b/hi.m3u8")
                    .unwrap()
            )
        );
        let (_, kind) = pieces(flatten(Kind::Hls, "#EXTM3U\n#EXTINF:10,\na.ts\n", &at()).unwrap());
        assert_eq!(kind, "video/mp2t");
    }

    #[test]
    fn a_song_in_pieces_starts_from_the_piece_a_time_falls_in() {
        // Tidal's: 55 pieces of 176128 / 44100 s (about 3.99 s), then a short one.
        let Flat::Pieces {
            pieces,
            head,
            starts,
            ..
        } = flatten(Kind::Dash, TIDAL, &at()).unwrap()
        else {
            panic!()
        };
        assert_eq!(head, 1);
        let starts = starts.unwrap();
        assert_eq!(starts.len(), 56);
        let piece = 176128.0 / 44100.0;
        assert!((starts[10] - 10.0 * piece).abs() < 1e-9);
        let (kept, left) = from_time(pieces.clone(), head, Some(&starts), 42.0);
        // The first piece, then from the 11th (40.0 s in).
        assert_eq!(kept.len(), 1 + 46);
        assert_eq!(kept[0], pieces[0]);
        assert_eq!(kept[1], pieces[11]);
        assert!((left - (42.0 - 10.0 * piece)).abs() < 1e-9);
        // From the start, or without times: all of them.
        assert_eq!(
            from_time(pieces.clone(), head, Some(&starts), 0.0).0.len(),
            57
        );
        assert_eq!(from_time(pieces.clone(), head, None, 42.0), (pieces, 42.0));
        // HLS: from each piece's #EXTINF.
        let text = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\n1.m4s\n#EXTINF:4.5,\n2.m4s\n#EXTINF:4,\n3.m4s\n";
        let Flat::Pieces { head, starts, .. } = flatten(Kind::Hls, text, &at()).unwrap() else {
            panic!()
        };
        assert_eq!((head, starts), (1, Some(vec![0.0, 4.0, 8.5])));
        let text = "#EXTM3U\n#EXTINF:4,\n1.ts\n2.ts\n";
        let Flat::Pieces { head, starts, .. } = flatten(Kind::Hls, text, &at()).unwrap() else {
            panic!()
        };
        assert_eq!((head, starts), (0, None));
    }

    #[test]
    fn times_and_templates_are_read_as_dash_writes_them() {
        assert!((seconds("PT3M40.523S").unwrap() - 220.523).abs() < 1e-9);
        assert_eq!(seconds("P1DT1H"), Some(90000.0));
        assert_eq!(seconds("PT"), Some(0.0));
        assert_eq!(seconds("3M"), None);
        assert_eq!(
            fill("a$$b$Number%05d$-$Time$-$Bandwidth$", "r", "96", 7, 12).unwrap(),
            "a$b00007-12-96"
        );
        assert!(fill("$Nope$", "r", "", 1, 0).is_err());
    }
}
