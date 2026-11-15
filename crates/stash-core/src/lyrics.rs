//! Lyrics: the words of a song, timed line by line (and word by word, when a source has that), or plain.
//!
//! Where they come from, in order: for a file, a `.lrc` file beside it with the same name, then the lyrics in its
//! own tags; then the listener's add-ons that offer lyrics (contract section 4.6); then LRCLIB, an open database of
//! timed lyrics (lrclib.net), unless the listener switched that off. What was found on the network is kept in the
//! library ([`Library::cached_lyrics`]), so a song is asked for once; a file's own lyrics are read each time.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::identity::clean_text;

/// The most lines, words in a line, characters in a line, and characters of plain lyrics taken from a source
/// (the contract's limits).
const MAX_LINES: usize = 3_000;
const MAX_WORDS: usize = 300;
const MAX_LINE: usize = 600;
const MAX_PLAIN: usize = 100_000;
/// No song is longer than this; a time beyond it is a broken answer.
const MAX_MS: u64 = 24 * 3_600_000;
/// The largest `.lrc` file read.
const MAX_FILE: u64 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced: Option<Vec<Line>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plain: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub start_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<Word>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub start_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    pub text: String,
}

impl Lyrics {
    /// Lyrics as a source gave them, made safe to show: text cleaned and capped, times in range, lines in time
    /// order. `None` when nothing is left.
    pub fn checked(self) -> Option<Lyrics> {
        let synced = self.synced.map(|lines| {
            let mut lines: Vec<Line> = lines
                .into_iter()
                .take(MAX_LINES)
                .filter(|l| l.start_ms <= MAX_MS)
                .map(|l| Line {
                    start_ms: l.start_ms,
                    end_ms: l.end_ms.filter(|e| *e <= MAX_MS && *e >= l.start_ms),
                    text: clean_text(&l.text, MAX_LINE),
                    words: l.words.map(|words| {
                        words
                            .into_iter()
                            .take(MAX_WORDS)
                            .filter(|w| w.start_ms <= MAX_MS)
                            .map(|w| Word {
                                start_ms: w.start_ms,
                                end_ms: w.end_ms.filter(|e| *e <= MAX_MS && *e >= w.start_ms),
                                text: clean_text(&w.text, MAX_LINE),
                            })
                            .collect()
                    }),
                })
                .collect();
            lines.sort_by_key(|l| l.start_ms);
            lines
        });
        let plain = self
            .plain
            .map(|p| clean_plain(&p))
            .filter(|p| !p.is_empty());
        let synced = synced.filter(|lines| lines.iter().any(|l| !l.text.is_empty()));
        (synced.is_some() || plain.is_some()).then_some(Lyrics { synced, plain })
    }
}

/// Plain lyrics: each line cleaned as text, the line breaks kept, runs of empty lines made one, capped.
fn clean_plain(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines() {
        let line = clean_text(line, MAX_LINE);
        if line.is_empty() {
            if !out.is_empty() {
                blank = true;
            }
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank {
                out.push('\n');
            }
        }
        blank = false;
        out.push_str(&line);
        if out.len() >= MAX_PLAIN {
            break;
        }
    }
    out.chars().take(MAX_PLAIN).collect()
}

/// A line's time stamp: `mm:ss.xx`, `mm:ss.xxx` or `mm:ss`.
fn stamp(token: &str) -> Option<u64> {
    let (minutes, rest) = token.split_once(':')?;
    let (seconds, fraction) = rest.split_once(['.', ':']).unwrap_or((rest, ""));
    if minutes.is_empty() || minutes.len() > 3 || seconds.len() != 2 || fraction.len() > 3 {
        return None;
    }
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(minutes) || !all_digits(seconds) || !all_digits(fraction) {
        return None;
    }
    let fraction_ms = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u64>().ok()? * 100,
        2 => fraction.parse::<u64>().ok()? * 10,
        _ => fraction.parse::<u64>().ok()?,
    };
    let seconds: u64 = seconds.parse().ok()?;
    (seconds < 60).then_some(minutes.parse::<u64>().ok()? * 60_000 + seconds * 1_000 + fraction_ms)
}

/// LRC: lines that start with one or more `[mm:ss.xx]` stamps (the same words at each), and tags like `[ar:…]`,
/// of which `[offset:±ms]` moves every line. Lines in time order.
pub fn parse_lrc(body: &str) -> Vec<(u64, String)> {
    let mut offset: i64 = 0;
    let mut lines = Vec::new();
    for raw in body.lines() {
        let line = raw.trim_end();
        if let Some(tag) = line
            .trim_start()
            .strip_prefix("[offset:")
            .and_then(|t| t.strip_suffix(']'))
        {
            offset = tag.trim().parse().unwrap_or(0);
            continue;
        }
        let mut stamps = Vec::new();
        let mut rest = line.trim_start();
        while let Some(inner) = rest.strip_prefix('[') {
            let Some(close) = inner.find(']') else { break };
            let Some(ms) = stamp(&inner[..close]) else {
                break;
            };
            stamps.push(ms);
            rest = &inner[close + 1..];
        }
        let text = rest.trim();
        if stamps.is_empty() || text.is_empty() {
            continue;
        }
        for ms in stamps {
            // A positive offset shows the words earlier (the LRC convention).
            let at = (ms as i64 - offset).max(0) as u64;
            lines.push((at, text.to_string()));
        }
    }
    lines.sort_by_key(|(at, _)| *at);
    lines
}

/// Lyrics from LRC and, when a source has them, plain words: each line ends where the next starts. Text that is
/// not LRC at all is taken as plain lyrics.
pub fn from_lrc(lrc: Option<&str>, plain: Option<&str>) -> Option<Lyrics> {
    let lines = lrc.map(parse_lrc).unwrap_or_default();
    let synced: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(i, (at, text))| Line {
            start_ms: *at,
            end_ms: lines.get(i + 1).map(|(next, _)| *next),
            text: text.clone(),
            words: None,
        })
        .collect();
    let plain = plain
        .map(str::to_string)
        .filter(|p| !p.trim().is_empty())
        .or_else(|| {
            if synced.is_empty() {
                // Not timed: the text itself is the lyrics.
                lrc.map(str::to_string)
            } else {
                Some(
                    synced
                        .iter()
                        .map(|l| l.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }
        });
    Lyrics {
        synced: (!synced.is_empty()).then_some(synced),
        plain,
    }
    .checked()
}

/// A file's own lyrics: `song.lrc` beside `song.flac`, else the lyrics in its tags (timed or plain). With where
/// they came from: "the .lrc file" or "the file's tags".
pub fn of_file(path: &Path) -> Option<(Lyrics, &'static str)> {
    let beside = path.with_extension("lrc");
    if let Ok(meta) = std::fs::metadata(&beside)
        && meta.is_file()
        && meta.len() <= MAX_FILE
        && let Ok(bytes) = std::fs::read(&beside)
        && let Some(found) = from_lrc(Some(&String::from_utf8_lossy(&bytes)), None)
    {
        return Some((found, "the .lrc file"));
    }
    use lofty::file::TaggedFileExt;
    use lofty::tag::ItemKey;
    let file = lofty::read_from_path(path).ok()?;
    let text = file
        .tags()
        .iter()
        .find_map(|tag| tag.get_string(ItemKey::Lyrics).map(str::to_string))?;
    from_lrc(Some(&text), None).map(|found| (found, "the file's tags"))
}

// ---- LRCLIB

/// What LRCLIB answers for one recording.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibHit {
    track_name: Option<String>,
    artist_name: Option<String>,
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

impl LrclibHit {
    fn lyrics(&self) -> Option<Lyrics> {
        if self.instrumental {
            return None;
        }
        from_lrc(self.synced_lyrics.as_deref(), self.plain_lyrics.as_deref())
    }
}

/// How alike two strings are, 0 to 1 (Jaro-Winkler, as the other Stashes match LRCLIB's search results).
pub fn jaro_winkler(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let (mut seen_a, mut seen_b) = (vec![false; a.len()], vec![false; b.len()]);
    let mut matches = 0usize;
    for (i, ca) in a.iter().enumerate() {
        let (from, to) = (i.saturating_sub(window), (i + window + 1).min(b.len()));
        for j in from..to {
            if !seen_b[j] && b[j] == *ca {
                seen_a[i] = true;
                seen_b[j] = true;
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 {
        return 0.0;
    }
    let mut transpositions = 0usize;
    let mut k = 0usize;
    for (i, ca) in a.iter().enumerate() {
        if !seen_a[i] {
            continue;
        }
        while !seen_b[k] {
            k += 1;
        }
        if *ca != b[k] {
            transpositions += 1;
        }
        k += 1;
    }
    let m = matches as f64;
    let jaro =
        (m / a.len() as f64 + m / b.len() as f64 + (m - transpositions as f64 / 2.0) / m) / 3.0;
    let prefix = a.iter().zip(&b).take(4).take_while(|(x, y)| x == y).count();
    jaro + prefix as f64 * 0.1 * (1.0 - jaro)
}

/// The search result that is this song: within 5 s of its length (when both are known), and the most alike by
/// artist and title, if that is alike enough.
fn pick(title: &str, artist: &str, duration_ms: Option<u64>, hits: &[LrclibHit]) -> Option<usize> {
    let wanted = format!("{artist} {title}").to_lowercase();
    let seconds = duration_ms.map(|ms| (ms / 1000) as f64);
    let mut best: Option<(usize, f64)> = None;
    for (i, hit) in hits.iter().enumerate() {
        if let (Some(want), Some(have)) = (seconds, hit.duration)
            && (want - have).abs() > 5.0
        {
            continue;
        }
        let found = format!(
            "{} {}",
            hit.artist_name.as_deref().unwrap_or_default(),
            hit.track_name.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        let score = jaro_winkler(&wanted, &found);
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((i, score));
        }
    }
    best.filter(|(_, score)| *score >= 0.85).map(|(i, _)| i)
}

/// Why a network source gave no answer: not "it has none", but that it could not be asked (so the miss is not kept).
#[derive(Debug)]
pub struct Unreachable(pub String);

/// LRCLIB's lyrics for a song: the exact recording when its length is known, else the best search result.
/// `Ok(None)`: LRCLIB has none.
pub fn from_lrclib(
    base: &str,
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration_ms: Option<u64>,
) -> Result<Option<Lyrics>, Unreachable> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(12)))
        .user_agent(concat!(
            "Stash desktop/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/rawnaldclark/stash-desktop)"
        ))
        .build()
        .into();
    let get = |url: url::Url| -> Result<Option<String>, Unreachable> {
        let mut answer = agent
            .get(url.as_str())
            .call()
            .map_err(|e| Unreachable(e.to_string()))?;
        match answer.status().as_u16() {
            404 => Ok(None),
            200..=299 => answer
                .body_mut()
                .with_config()
                .limit(4 << 20)
                .read_to_string()
                .map(Some)
                .map_err(|e| Unreachable(e.to_string())),
            status => Err(Unreachable(format!("LRCLIB answered {status}"))),
        }
    };
    let address = |path: &str| {
        url::Url::parse(&format!("{base}{path}")).map_err(|e| Unreachable(e.to_string()))
    };
    if let Some(ms) = duration_ms.filter(|ms| *ms >= 1000) {
        let mut url = address("/api/get")?;
        url.query_pairs_mut()
            .append_pair("track_name", title)
            .append_pair("artist_name", artist)
            .append_pair("duration", &(ms / 1000).to_string());
        if let Some(album) = album.filter(|a| !a.is_empty()) {
            url.query_pairs_mut().append_pair("album_name", album);
        }
        if let Some(body) = get(url)?
            && let Ok(hit) = serde_json::from_str::<LrclibHit>(&body)
        {
            if hit.instrumental {
                return Ok(None);
            }
            if let Some(found) = hit.lyrics() {
                return Ok(Some(found));
            }
        }
    }
    let mut url = address("/api/search")?;
    url.query_pairs_mut()
        .append_pair("q", &format!("{artist} {title}"));
    let Some(body) = get(url)? else {
        return Ok(None);
    };
    let hits: Vec<LrclibHit> = serde_json::from_str(&body).unwrap_or_default();
    Ok(pick(title, artist, duration_ms, &hits).and_then(|i| hits[i].lyrics()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_lines_are_timed_ordered_and_shared_stamps_repeat_the_words() {
        let lrc = "[ar:Someone]\n[offset:+500]\n[00:12.30]First\n[00:05.5][01:00.120]Chorus\n\n[00:20]Last \n[xx:yy]not a line\nno stamp\n";
        assert_eq!(
            parse_lrc(lrc),
            [
                (5_000, "Chorus".to_string()),
                (11_800, "First".into()),
                (19_500, "Last".into()),
                (59_620, "Chorus".into()),
            ]
        );
        let lyrics = from_lrc(Some(lrc), None).unwrap();
        let synced = lyrics.synced.unwrap();
        assert_eq!((synced[0].end_ms, synced[3].end_ms), (Some(11_800), None));
        assert_eq!(lyrics.plain.as_deref(), Some("Chorus\nFirst\nLast\nChorus"));
    }

    #[test]
    fn text_that_is_not_lrc_is_plain_lyrics_and_nothing_is_nothing() {
        let plain = from_lrc(Some("Line one\n\n\n\nLine two\u{202e}"), None).unwrap();
        assert_eq!(
            (plain.synced, plain.plain.as_deref()),
            (None, Some("Line one\n\nLine two"))
        );
        assert_eq!(from_lrc(Some("  \n "), None), None);
        assert_eq!(from_lrc(None, None), None);
    }

    #[test]
    fn a_sources_lyrics_are_checked_before_they_are_shown() {
        let given = Lyrics {
            synced: Some(vec![
                Line {
                    start_ms: 9_000,
                    end_ms: Some(1),
                    text: "late\u{0}".into(),
                    words: None,
                },
                Line {
                    start_ms: 1_000,
                    end_ms: None,
                    text: "early".into(),
                    words: Some(vec![Word {
                        start_ms: MAX_MS + 1,
                        end_ms: None,
                        text: "x".into(),
                    }]),
                },
                Line {
                    start_ms: MAX_MS + 1,
                    end_ms: None,
                    text: "never".into(),
                    words: None,
                },
            ]),
            plain: Some("   ".into()),
        };
        let checked = given.checked().unwrap();
        let lines = checked.synced.unwrap();
        assert_eq!(
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["early", "late"]
        );
        assert_eq!(
            (lines[1].end_ms, lines[0].words.as_ref().unwrap().len()),
            (None, 0)
        );
        assert_eq!(checked.plain, None);
        assert_eq!(Lyrics::default().checked(), None);
    }

    #[test]
    fn the_search_result_picked_is_the_same_song_of_about_the_same_length() {
        let hit = |artist: &str, title: &str, duration: f64| LrclibHit {
            track_name: Some(title.into()),
            artist_name: Some(artist.into()),
            duration: Some(duration),
            instrumental: false,
            plain_lyrics: Some("words".into()),
            synced_lyrics: None,
        };
        let hits = [
            hit("Someone Else", "Other Song", 200.0),
            hit("The Band", "The Song (Live)", 320.0),
            hit("The Band", "The Song", 201.0),
        ];
        assert_eq!(pick("The Song", "The Band", Some(200_400), &hits), Some(2));
        // Without a length the closest words win; nothing alike enough is nothing.
        assert_eq!(pick("The Song", "The Band", None, &hits), Some(2));
        assert_eq!(pick("Unrelated", "Nobody", None, &hits), None);
        assert!((jaro_winkler("martha", "marhta") - 0.961).abs() < 0.001);
    }
}
