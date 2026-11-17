//! Reading YouTube Music's answers: the rows of a search.
//!
//! YouTube's JSON describes a page (shelves, rows, columns, runs of text), not songs, and it changes without notice.
//! Everything here reads a row by what its parts link to (an artist's page, an album's page) before what they say,
//! and leaves out what it cannot read. The rules are those of Stash for Android (`ResponseParserHelpers.kt`,
//! `SearchResponseParser.kt`, `FlatSearchResponseParser.kt`), checked against the same recorded answers.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::identity::clean_text;

const TEXT: usize = 300;

/// What kind of upload a row is, as YouTube Music marks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The song itself, as its label released it ("art track").
    Song,
    /// The official music video: often another edit, with another length.
    Video,
    /// Someone's upload.
    Upload,
    /// Another official upload (a live session, a lyric video).
    Official,
    Episode,
}

impl Kind {
    fn of(raw: &str) -> Option<Kind> {
        Some(match raw {
            "MUSIC_VIDEO_TYPE_ATV" => Kind::Song,
            "MUSIC_VIDEO_TYPE_OMV" => Kind::Video,
            "MUSIC_VIDEO_TYPE_UGC" => Kind::Upload,
            "MUSIC_VIDEO_TYPE_OFFICIAL_SOURCE_MUSIC" => Kind::Official,
            "MUSIC_VIDEO_TYPE_PODCAST_EPISODE" => Kind::Episode,
            _ => return None,
        })
    }
}

/// A song as a row of YouTube Music names it.
#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    /// The video's id: 11 characters.
    pub id: String,
    pub title: String,
    /// Its artists, joined with ", "; empty when the row names none.
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub art: Option<String>,
    pub explicit: bool,
    pub kind: Option<Kind>,
}

static DURATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,2}:\d{2}(?::\d{2})?$").unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}$").unwrap());
/// "16M plays", "1,234 views": a row's count, never an artist or an album.
static COUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[\d.,]+\s*[KMB]?\s*(plays?|views?)$").unwrap());
static WIDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"=w\d+-h\d+").unwrap());
static SQUARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"=s\d+").unwrap());

/// Between the groups of a row's second line: artists • album • length.
const GROUPS: &str = " • ";
/// Between the names of one group.
const JOINERS: [&str; 3] = [", ", " & ", " x "];
/// What a row of the flat layout says first: its kind, not its artist.
const LABELS: [&str; 4] = ["Song", "Video", "Music video", "Episode"];

/// A video id: 11 characters of YouTube's alphabet.
pub fn video_id(text: &str) -> Option<&str> {
    let fits = text.len() == 11
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    fits.then_some(text)
}

fn text(value: Option<&Value>) -> Option<String> {
    let cleaned = clean_text(value?.as_str()?, TEXT);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The runs of text of a row's column `n`.
fn column(row: &Value, n: usize) -> Option<&Vec<Value>> {
    row.get("flexColumns")?
        .get(n)?
        .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?
        .as_array()
}

fn run_text(run: &Value) -> &str {
    run.get("text").and_then(Value::as_str).unwrap_or_default()
}

/// The page a run links to.
fn run_page(run: &Value) -> Option<&str> {
    run.pointer("/navigationEndpoint/browseEndpoint/browseId")?
        .as_str()
}

/// A run that links to an artist, a channel or a podcast.
fn names_artist(run: &Value) -> bool {
    run_page(run).is_some_and(|page| {
        page.starts_with("UC") || page.starts_with("MPLAUC") || page.starts_with("MPSPP")
    })
}

fn is_stat(text: &str) -> bool {
    DURATION.is_match(text) || COUNT.is_match(text)
}

/// "3:45" or "1:02:30" in milliseconds.
fn length_ms(text: &str) -> Option<u64> {
    if !DURATION.is_match(text) {
        return None;
    }
    let seconds = text
        .split(':')
        .try_fold(0u64, |sum, part| Some(sum * 60 + part.parse::<u64>().ok()?))?;
    (seconds > 0).then_some(seconds * 1_000)
}

/// What a row's second line says: its artists, the album it links to, and its length.
fn second_line(runs: Option<&Vec<Value>>) -> (String, Option<String>, Option<u64>) {
    let mut groups: Vec<Vec<&Value>> = vec![Vec::new()];
    for run in runs.into_iter().flatten() {
        if run_text(run) == GROUPS {
            groups.push(Vec::new());
        } else if let Some(last) = groups.last_mut() {
            last.push(run);
        }
    }
    if let [only] = groups[0].as_slice()
        && LABELS.contains(&run_text(only))
    {
        groups.remove(0);
    }
    // The artists: the whole group that links to an artist (one of them may not be linked). Without a link, the
    // first group, unless it links elsewhere or is a count, a length or a year.
    let named = groups
        .iter()
        .find(|group| group.iter().any(|run| names_artist(run)))
        .or_else(|| {
            groups.first().filter(|group| {
                group.iter().all(|run| {
                    let said = run_text(run);
                    run_page(run).is_none() && !is_stat(said) && !YEAR.is_match(said)
                })
            })
        });
    let artist = named
        .into_iter()
        .flatten()
        .map(|run| run_text(run))
        .filter(|said| !JOINERS.contains(said))
        .filter_map(|said| text(Some(&Value::from(said))))
        .collect::<Vec<_>>()
        .join(", ");
    let album = groups
        .iter()
        .flatten()
        .find(|run| run_page(run).is_some_and(|page| page.starts_with("MPREb_")))
        .and_then(|run| text(run.get("text")));
    // A length is never linked: a linked "4:44" is an album.
    let length = groups.iter().find_map(|group| match group.as_slice() {
        [only] if run_page(only).is_none() => length_ms(run_text(only)),
        _ => None,
    });
    (artist, album, length)
}

/// The largest of a row's pictures, asked for at a size fit for a grid.
fn picture(holder: &Value, under: &str) -> Option<String> {
    let pictures = holder
        .get(under)?
        .pointer("/musicThumbnailRenderer/thumbnail/thumbnails")?
        .as_array()?;
    let largest = pictures
        .iter()
        .max_by_key(|p| p.get("width").and_then(Value::as_u64).unwrap_or(0))?;
    let url = largest.get("url")?.as_str()?;
    if !url.starts_with("https://") || url.len() > 2_048 {
        return None;
    }
    Some(sized(url))
}

/// Google's picture servers take the size in the address: a small one is asked for larger.
fn sized(url: &str) -> String {
    if url.contains("googleusercontent.com/") {
        WIDE.replace(url, "=w544-h544").into_owned()
    } else if url.contains("ggpht.com/") {
        SQUARE.replace(url, "=s544").into_owned()
    } else {
        url.to_string()
    }
}

/// Whether a row (or a card) carries the "explicit" badge.
fn explicit(holder: &Value) -> bool {
    ["badges", "subtitleBadges", "subtitleBadge"]
        .iter()
        .filter_map(|key| holder.get(key)?.as_array())
        .flatten()
        .any(|badge| {
            badge
                .pointer("/musicInlineBadgeRenderer/icon/iconType")
                .and_then(Value::as_str)
                == Some("MUSIC_EXPLICIT_BADGE")
        })
}

fn kind(watch: Option<&Value>) -> Option<Kind> {
    Kind::of(
        watch?
            .pointer("/watchEndpointMusicSupportedConfigs/watchEndpointMusicConfig/musicVideoType")?
            .as_str()?,
    )
}

/// One row of a list (`musicResponsiveListItemRenderer`) as a song; nothing when it is not one (an artist, an
/// album, a row without a video or a title).
pub(crate) fn row(row: &Value) -> Option<Song> {
    let play = row.pointer(
        "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint",
    );
    let id = row
        .pointer("/playlistItemData/videoId")
        .or_else(|| play?.get("videoId"))
        .and_then(Value::as_str)
        .and_then(video_id)?;
    let first = column(row, 0)?.first()?;
    let title = text(first.get("text"))?;
    let (artist, linked_album, line_length) = second_line(column(row, 1));
    // The third column is the album on an album's page, and a count ("16M plays") in a search.
    let album = column(row, 2)
        .and_then(|runs| runs.first())
        .map(run_text)
        .filter(|said| !is_stat(said))
        .and_then(|said| text(Some(&Value::from(said))))
        .or(linked_album);
    let fixed = row
        .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text/runs/0/text")
        .and_then(Value::as_str)
        .and_then(length_ms);
    Some(Song {
        id: id.to_string(),
        title,
        artist,
        album,
        duration_ms: fixed.or(line_length),
        art: picture(row, "thumbnail"),
        explicit: explicit(row),
        kind: kind(play.or_else(|| first.pointer("/navigationEndpoint/watchEndpoint"))),
    })
}

/// The shelves of a search's answer.
fn shelves(answer: &Value) -> impl Iterator<Item = &Value> {
    answer
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// The songs of a search's answer, in YouTube's order: the rows of its shelves (a search for songs has one, "Songs"),
/// or of the flat layout YouTube gives a visitor who is not signed in, where each row is a section of its own.
pub fn songs(answer: &Value) -> Vec<Song> {
    let mut found: Vec<Song> = Vec::new();
    for shelf in shelves(answer) {
        let rows = shelf
            .pointer("/musicShelfRenderer/contents")
            .or_else(|| shelf.pointer("/itemSectionRenderer/contents"))
            .and_then(Value::as_array);
        for song in rows
            .into_iter()
            .flatten()
            .filter_map(|item| row(item.get("musicResponsiveListItemRenderer")?))
        {
            if !found.iter().any(|known| known.id == song.id) {
                found.push(song);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(text: &str, page: Option<&str>) -> Value {
        match page {
            Some(page) => {
                json!({ "text": text, "navigationEndpoint": { "browseEndpoint": { "browseId": page } } })
            }
            None => json!({ "text": text }),
        }
    }

    #[test]
    fn a_second_line_is_read_by_what_its_parts_link_to() {
        // The "Songs" shelf: artists • album • length; one artist is not linked.
        let runs = vec![
            run("Yeahman", Some("UC1")),
            run(", ", None),
            run("Hajna", None),
            run(" & ", None),
            run("Mina Shankha", Some("UC2")),
            run(" • ", None),
            run("4:44", Some("MPREb_x")),
            run(" • ", None),
            run("5:13", None),
        ];
        assert_eq!(
            second_line(Some(&runs)),
            (
                "Yeahman, Hajna, Mina Shankha".to_string(),
                Some("4:44".to_string()),
                Some(313_000)
            )
        );
        // The flat layout: its kind first; a video's count is not its artist.
        let runs = vec![
            run("Video", None),
            run(" • ", None),
            run("Grimm's VGM", None),
            run(" • ", None),
            run("845 views", None),
        ];
        assert_eq!(
            second_line(Some(&runs)),
            ("Grimm's VGM".to_string(), None, None)
        );
        // A year alone names nobody; a first group that links elsewhere neither.
        let runs = vec![run("1989", None)];
        assert_eq!(second_line(Some(&runs)).0, "");
        let runs = vec![run("Some album", Some("MPREb_y"))];
        assert_eq!(
            second_line(Some(&runs)),
            (String::new(), Some("Some album".to_string()), None)
        );
        assert_eq!(second_line(None), (String::new(), None, None));
    }

    #[test]
    fn lengths_counts_ids_and_pictures() {
        assert_eq!(length_ms("3:45"), Some(225_000));
        assert_eq!(length_ms("1:02:30"), Some(3_750_000));
        assert_eq!(length_ms("0:00"), None);
        assert_eq!(length_ms("345"), None);
        assert!(is_stat("16M plays") && is_stat("1,234 views") && is_stat("1 play"));
        assert!(!is_stat("Child's Play") && !is_stat("1989"));
        assert_eq!(video_id("dQw4w9WgXcQ"), Some("dQw4w9WgXcQ"));
        assert_eq!(video_id("dQw4w9WgXc"), None);
        assert_eq!(video_id("dQw4w9WgX/Q"), None);
        assert_eq!(
            sized("https://lh3.googleusercontent.com/abc=w60-h60-l90-rj"),
            "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj"
        );
        assert_eq!(
            sized("https://yt3.ggpht.com/abc=s88-c"),
            "https://yt3.ggpht.com/abc=s544-c"
        );
        assert_eq!(
            sized("https://i.ytimg.com/vi/x/hqdefault.jpg"),
            "https://i.ytimg.com/vi/x/hqdefault.jpg"
        );
    }
}
