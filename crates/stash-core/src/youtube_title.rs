//! The song a YouTube row really is, read from its video title and channel: what a catalogue would list as artist and
//! title. A row brought in from YouTube carries whatever the upload had: "Artist - Song (Official Video)",
//! "KEXP | Phantogram - When I'm Small (Live on KEXP)", a channel as the artist ("ArtistVEVO", an uploader's name).
//! Searched as it is, an add-on is asked for noise.
//!
//! A port of the Stash Android app's `YouTubeSongTitle` (rule for rule), so the two apps look such songs up alike.
//! Text only: a row whose song needs outside knowledge is left as close to its fields as the rules allow. Not the
//! library's identity (the row keeps its title and artist); this only shapes what add-ons are asked.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use unicode_normalization::UnicodeNormalization;

use crate::addons::Wanted;

/// What a row is: the performing artist, the title without the artist, upload noise or version, and every version
/// the row names, comma-joined ("Live on KEXP", "2015 Remaster, Live"), or "".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub artist: String,
    pub title: String,
    pub version: String,
}

impl Parsed {
    /// The title as a lookup carries it: the version back in brackets, where the matcher reads it.
    pub fn query_title(&self) -> String {
        if self.version.trim().is_empty() {
            self.title.clone()
        } else {
            format!("{} ({})", self.title, self.version)
        }
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

/// Upload noise inside brackets or after a bar: never a version (matched whole).
static NOISE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)^(?:(official\s*)?(hd\s*|4k\s*|hq\s*)?(music\s*)?(video|audio|visuali[sz]er|lyrics?(\s*video)?|clip|mv|m/v|",
        r"videoclip|video\s*oficial|official|hd|hq|4k|remastered\s*(in\s*)?(hd|4k))(\s*(hd|4k|hq))?)$|",
        r"^(?:lyrics?|letra|with lyrics|audio only|full song|explicit|clean|uncensored|out now|new song.*|premiere|\d{4}|visualiser)$",
    ))
});
static NOISE_PHRASE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(official|lyric|music)\b.*\b(video|audio)\b"));
static TRAILING_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\s+(official\s+(music\s+)?(video|audio)|lyric video|music video|visuali[sz]er|lyrics|hd|hq|4k|closed captioned)\s*$",
    )
});
/// Words that make a bracket, bar or dash part a version of the song rather than more title.
static VERSION: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)\b(live|remix|mix|acoustic|unplugged|session|sessions|demo|instrumental|cover|edit|version|remaster(ed)?|mono|",
        r"stereo|sped|slowed|reverb|piano|orchestral|a ?cappella|karaoke|extended|radio|club|dub|rework|bootleg|vip|bonus|",
        r"re-?record(ed|ing)?|outtakes?|performance|concert|colors show)\b",
    ))
});
static CREDIT: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(feat|ft|with|prod|arr)\b"));
/// An uploader rather than an artist: VEVO/Topic channels, live-session channels, "…Official", "…Covers", handles
/// with numbers.
static CHANNELISH: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)(vevo$|\s-\s?topic$|channel|official$|covers?$|\btv$|records?$|recordings$|\bmusic$|\d{3,}|^kexp$|^audiotree|",
        r"^colors$|^npr|^genius$|^cercle$)",
    ))
});
static BRACKET: LazyLock<Regex> = LazyLock::new(|| re(r"\s*[(\[]([^)\]]*)[)\]]"));
static TRACK_NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"^\d{1,2}\s*[-.]\s+"));
static BAR: LazyLock<Regex> = LazyLock::new(|| re(r"\s+(?:[|｜]|//)\s+"));
static QUOTED: LazyLock<Regex> = LazyLock::new(|| re(r#"^(.*?)["“”]([^"“”]+)["“”](.*)$"#));
static LEADING_DASH: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[-–—]\s*"));
static DASH: LazyLock<Regex> = LazyLock::new(|| re(r"\s+[-–—]\s*|\s*[-–—]\s+"));
static FEAT_TAIL: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\s*\b(feat\.?|ft\.?|featuring)\s.*$"));
static PERFORMING: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\s+performing\s*$"));
static COMPOSER_PREFIX: LazyLock<Regex> = LazyLock::new(|| re(r"^([^:]{2,40}):\s+(.+)$"));
static FROM_NOTE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^from\b"));
/// The last " - part" of a string, its text in group 1.
static DASH_TAIL: LazyLock<Regex> = LazyLock::new(|| re(r"\s+[-–—]\s*([^-–—]*)$"));
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));
static TOPIC_SUFFIX: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\s*-\s*topic$"));
static VEVO_SUFFIX: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)vevo$"));
static OFFICIAL_SUFFIX: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\s+official$"));
static TOKEN_BRACKETS: LazyLock<Regex> = LazyLock::new(|| re(r"\([^)]*\)|\[[^\]]*\]"));
static TOKEN_FEAT: LazyLock<Regex> = LazyLock::new(|| re(r"\b(feat\.?|ft\.?|featuring)\b.*"));
static TOKEN_APOSTROPHES: LazyLock<Regex> = LazyLock::new(|| re(r"['’`]"));
static TOKEN_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"[^\p{L}\p{N}\p{S}\s]"));

/// Whether a channel's name is how YouTube names an artist's own audio ("Artist - Topic"): such a row is the
/// recording, and its length is the recording's.
pub fn is_topic_channel(artist: &str) -> bool {
    TOPIC_SUFFIX.is_match(artist.trim())
}

/// How a song that came from YouTube alone is asked for: as the song its title names (the version back in brackets),
/// and with its length only when the row is the artist's own audio (a Topic channel, or a row with an album). Other
/// rows carry the video's length, often not the recording's, so it is not vouched for.
///
/// With it, other ways the row may be read, which a search's results are matched against only when the first finds
/// nothing: the row as it is kept (with the same length rule), since the rules can misread a row whose artist was
/// right ("A Love Supreme, Pt. I - Acknowledgement" by John Coltrane); and, when the artist was taken from the title,
/// the other way round ("No Quarter - Led Zeppelin").
pub fn asked_for(mut kept: Wanted) -> (Wanted, Vec<Wanted>) {
    let own_audio = is_topic_channel(&kept.artist)
        || kept
            .album
            .as_deref()
            .is_some_and(|album| !album.trim().is_empty());
    if !own_audio {
        kept.duration_ms = None;
    }
    let song = parse(&kept.title, &kept.artist);
    let mut asked = kept.clone();
    if !song.artist.is_empty() {
        asked.artist = song.artist.clone();
    }
    let title = song.query_title();
    if !title.trim().is_empty() {
        asked.title = title;
    }
    // "パス・トゥ・ハート - Path To Heart" by FLat7 is one title in two scripts, not an artist and a title.
    if asked.artist != clean_artist(&kept.artist) && in_two_scripts(&kept.title, &asked.artist) {
        asked.artist = clean_artist(&kept.artist);
        asked.title = kept.title.clone();
    }
    let mut also = Vec::new();
    // Not when the would-be artist carries a bracket: "Song - Band (Austin City Limits, 1989)" names a performance.
    if asked.artist != clean_artist(&kept.artist)
        && song.version.is_empty()
        && !song.title.contains(['(', '['])
    {
        also.push(Wanted {
            artist: song.title.clone(),
            title: song.artist.clone(),
            ..asked.clone()
        });
    }
    if asked != kept {
        also.insert(0, kept);
    }
    (asked, also)
}

/// Whether `title` is "left - right" with one side in Latin letters and the other in none, and `side` is one of them.
fn in_two_scripts(title: &str, side: &str) -> bool {
    let mut parts = title.splitn(2, " - ");
    let (Some(left), Some(right)) = (parts.next(), parts.next()) else {
        return false;
    };
    let latin = |s: &str| s.chars().any(|c| c.is_ascii_alphabetic());
    let other = |s: &str| s.chars().any(|c| c.is_alphabetic() && !c.is_ascii());
    let split = (!latin(left) && other(left) && latin(right))
        || (!latin(right) && other(right) && latin(left));
    split && (left.trim() == side.trim() || right.trim() == side.trim())
}

/// `title` as YouTube had it, `artist` the row's artist (often the uploading channel).
pub fn parse(title: &str, artist: &str) -> Parsed {
    let mut t = title.trim().to_string();
    let mut a = clean_artist(artist);
    // Every version the row names is kept: "Purple Rain (2015 Remaster) (Live)" is the live take.
    let mut versions: Vec<String> = Vec::new();
    let add_version = |versions: &mut Vec<String>, v: &str| {
        let clean = LEADING_DASH.replace(v, "").trim().to_string();
        if !clean.is_empty() && !versions.iter().any(|it| it.eq_ignore_ascii_case(&clean)) {
            versions.push(clean);
        }
    };
    let mut had_noise = false;

    // 1. Bracket groups: noise goes, a version is kept apart, anything else is part of the title. A version word
    // beats a noise phrase: "(Official Live Video)" is still the live take.
    t = BRACKET
        .replace_all(&t, |m: &Captures| {
            let inner = m[1].trim();
            if NOISE.is_match(inner) {
                had_noise = true;
                String::new()
            } else if VERSION.is_match(inner) && !CREDIT.is_match(inner) {
                add_version(&mut versions, inner);
                String::new()
            } else if NOISE_PHRASE.is_match(inner) {
                had_noise = true;
                String::new()
            } else {
                m[0].to_string()
            }
        })
        .into_owned();
    if TRAILING_NOISE.is_match(&t) {
        had_noise = true;
        t = TRAILING_NOISE.replace_all(&t, "").into_owned();
    }
    t = TRACK_NUMBER.replace_all(&t, "").into_owned();

    // 2. "Song | rest" / "Song // rest": the rest is a version or noise ("aloneintokyo // hollows" puts the artist
    // first).
    let bar: Vec<String> = BAR.split(&t).map(str::to_string).collect();
    if bar.len() > 1 {
        if bar.len() == 2 && jaccard(&bar[0], &a) >= 0.8 {
            t = bar[1].clone();
        } else {
            t = bar[0].clone();
            let rest = bar[1..].join(" ");
            if VERSION.is_match(&rest) {
                add_version(&mut versions, &rest);
            }
            had_noise = true;
        }
    }

    // 3. Artist "Title", or "Title" - Artist. Version words after the quote stay: 'Cage The Elephant performing
    // "Cold Cold Cold" Live on KCRW'.
    if let Some(q) = QUOTED.captures(&t) {
        let (before, quoted, after) = (q[1].to_string(), q[2].to_string(), q[3].to_string());
        let before_dash = DASH_TAIL.captures(&before);
        if before.trim().is_empty()
            && LEADING_DASH.is_match(&after)
            && after.trim().chars().count() > 1
        {
            a = LEADING_DASH.replace(&after, "").trim().to_string();
            t = quoted.trim().to_string();
        } else if let Some(dash) = before_dash.as_ref().filter(|d| FROM_NOTE.is_match(&d[1])) {
            // 'Lose Yourself - From "8 Mile" Soundtrack': a soundtrack note, not an artist and a title.
            t = before[..dash.get(0).unwrap().start()].to_string();
        } else if !before.trim().is_empty()
            && (jaccard(&FEAT_TAIL.replace_all(&before, ""), &a) >= 0.5
                || had_noise
                || CHANNELISH.is_match(artist)
                || PERFORMING.is_match(&before))
        {
            let unfeatured = FEAT_TAIL.replace_all(&before, "");
            let lead = PERFORMING
                .replace_all(&unfeatured, "")
                .trim_end_matches(['.', ' ', '-'])
                .trim()
                .to_string();
            if !lead.is_empty() && jaccard(&lead, &a) < 0.5 {
                a = lead;
            }
            t = quoted.trim().to_string();
            if VERSION.is_match(&after) {
                add_version(&mut versions, &after);
            }
        }
    }

    // 4. "Left - Right": YouTube's "Artist - Title", unless the row says otherwise.
    let segs: Vec<String> = DASH.split(&t).map(str::to_string).collect();
    if segs.len() > 1 {
        let left = segs[0].clone();
        let artist_tokens = tokens(&a);
        let left_is_artist = jaccard(&left, &a) >= 0.5
            || (!artist_tokens.is_empty() && tokens(&left).is_superset(&artist_tokens));
        // "Orchestra, Conductor, Composer": a catalogue-style credit.
        let credited = artist.contains(',');
        let keeps_dash = credited || left.contains(':');
        let right = segs[1].clone();
        let channel = CHANNELISH.is_match(artist);
        let mut rest: Vec<String> = segs[1..].to_vec();
        if left_is_artist {
            if left.chars().count() >= a.chars().count() {
                a = left.clone();
            }
            t = rest.remove(0);
        } else if jaccard(&right, &a) >= 0.5 {
            // "Title - Artist".
            t = left.clone();
            rest.remove(0);
        } else if (VERSION.is_match(&right) || FROM_NOTE.is_match(&right)) && !channel {
            // A real artist's row keeps its artist when the right side is a version or a note, not a title:
            // "Until Morning - A COLORS SHOW" (James Vickery), "Song - From X".
            t = left.clone();
        } else if !keeps_dash
            && VERSION.is_match(&left)
            && tokens(&left).len() >= 2
            && !VERSION.is_match(&right)
            && !channel
        {
            // ...or when the left side is a version: "BBC Sessions - Love Or Confusion" (Jimi Hendrix).
            add_version(&mut versions, &left);
            t = right.clone();
            rest.remove(0);
        } else if keeps_dash {
            t = left.clone();
        } else {
            a = left.clone();
            t = rest.remove(0);
        }
        for r in &rest {
            if FROM_NOTE.is_match(r) || NOISE.is_match(r.trim()) {
                continue;
            }
            if VERSION.is_match(r) {
                add_version(&mut versions, r);
            } else if keeps_dash {
                t = format!("{t} - {r}");
            }
        }
    }

    t = strip_composer(&t, &a);
    Parsed {
        artist: a.trim().to_string(),
        title: WHITESPACE.replace_all(&t, " ").trim().to_string(),
        version: versions.join(", "),
    }
}

fn clean_artist(a: &str) -> String {
    let a = TOPIC_SUFFIX.replace_all(a, "");
    let a = VEVO_SUFFIX.replace_all(&a, "");
    OFFICIAL_SUFFIX.replace_all(&a, "").trim().to_string()
}

/// "Chopin: Nocturne…" when Chopin is already in the artist credit.
fn strip_composer(t: &str, a: &str) -> String {
    let Some(m) = COMPOSER_PREFIX.captures(t) else {
        return t.to_string();
    };
    let words: Vec<String> = tokens(&m[1])
        .into_iter()
        .filter(|w| w.chars().count() > 2)
        .collect();
    let artist_tokens = tokens(a);
    if !words.is_empty() && words.iter().all(|w| artist_tokens.contains(w)) {
        m[2].to_string()
    } else {
        t.to_string()
    }
}

/// Comparison words: accents folded, brackets and "feat." tails dropped, punctuation out.
pub fn tokens(s: &str) -> HashSet<String> {
    let plain: String = s
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    let plain = TOKEN_BRACKETS.replace_all(&plain, " ");
    let plain = TOKEN_FEAT.replace_all(&plain, " ");
    let plain = TOKEN_APOSTROPHES.replace_all(&plain, "");
    let plain = TOKEN_PUNCT.replace_all(&plain, " ");
    plain.split_whitespace().map(str::to_string).collect()
}

fn jaccard(a: &str, b: &str) -> f32 {
    let (x, y) = (tokens(a), tokens(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    let shared = x.intersection(&y).count();
    shared as f32 / (x.len() + y.len() - shared) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(title: &str, artist: &str) -> Parsed {
        parse(title, artist)
    }

    #[test]
    fn the_phone_apps_cases_read_the_same() {
        let r = p(
            "Rage Against The Machine - People of the Sun (Official HD Video)",
            "RageAgainstTheMachineVEVO",
        );
        assert_eq!(
            (r.artist.as_str(), r.title.as_str(), r.version.as_str()),
            ("Rage Against The Machine", "People of the Sun", "")
        );

        let r = p("Phantogram - When I'm Small (Live on KEXP)", "KEXP");
        assert_eq!(
            (r.artist.as_str(), r.title.as_str(), r.version.as_str()),
            ("Phantogram", "When I'm Small", "Live on KEXP")
        );
        assert_eq!(r.query_title(), "When I'm Small (Live on KEXP)");

        assert_eq!(
            p("Supermassive Black Hole", "Muse"),
            Parsed {
                artist: "Muse".into(),
                title: "Supermassive Black Hole".into(),
                version: String::new()
            }
        );

        let r = p("Only A Case - Nipsey Hussle", "Nipsey Hussle");
        assert_eq!(
            (r.artist.as_str(), r.title.as_str()),
            ("Nipsey Hussle", "Only A Case")
        );

        let r = p("An Ending (Ascent) - Remastered 2005", "Brian Eno");
        assert_eq!(
            (r.artist.as_str(), r.title.as_str(), r.version.as_str()),
            ("Brian Eno", "An Ending (Ascent)", "Remastered 2005")
        );

        let credit = "London Philharmonic Orchestra, Christopher Hogwood, Antonín Dvořák";
        let r = p(
            "Dvořák: Serenade for Strings in E, Op. 22 - 1. Moderato",
            credit,
        );
        assert_eq!(
            (r.artist.as_str(), r.title.as_str()),
            (credit, "Serenade for Strings in E, Op. 22 - 1. Moderato")
        );

        let r = p(
            "Cage The Elephant performing \"Cold Cold Cold\" Live on KCRW",
            "KCRW",
        );
        assert_eq!(
            (r.artist.as_str(), r.title.as_str(), r.version.as_str()),
            ("Cage The Elephant", "Cold Cold Cold", "Live on KCRW")
        );

        let r = p("Purple Rain (2015 Remaster) (Live)", "Prince");
        assert_eq!(
            (r.title.as_str(), r.version.as_str()),
            ("Purple Rain", "2015 Remaster, Live")
        );
        assert_eq!(r.query_title(), "Purple Rain (2015 Remaster, Live)");

        let r = p("Song (Official Live Video)", "Band");
        assert_eq!(
            (r.title.as_str(), r.version.as_str()),
            ("Song", "Official Live Video")
        );

        let r = p(
            "Joy Division - Transmission (Performance From \"Control\")",
            "Vaughan Walton",
        );
        assert_eq!(
            (r.artist.as_str(), r.title.as_str(), r.version.as_str()),
            (
                "Joy Division",
                "Transmission",
                "Performance From \"Control\""
            )
        );

        let r = p("Until Morning - A COLORS SHOW", "James Vickery");
        assert_eq!(
            (r.artist.as_str(), r.title.as_str(), r.version.as_str()),
            ("James Vickery", "Until Morning", "A COLORS SHOW")
        );

        let r = p("BBC Sessions - Love Or Confusion", "Jimi Hendrix");
        assert_eq!(
            (r.artist.as_str(), r.title.as_str(), r.version.as_str()),
            ("Jimi Hendrix", "Love Or Confusion", "BBC Sessions")
        );

        let r = p("Lose Yourself - From \"8 Mile\" Soundtrack", "Eminem");
        assert_eq!(
            (r.artist.as_str(), r.title.as_str(), r.version.as_str()),
            ("Eminem", "Lose Yourself", "")
        );

        let r = p("Phantogram - When I'm Small", "KEXP");
        assert_eq!(
            (r.artist.as_str(), r.title.as_str()),
            ("Phantogram", "When I'm Small")
        );
    }

    #[test]
    fn a_topic_channel_is_the_artists_own_audio() {
        assert!(is_topic_channel("Mira Sol - Topic"));
        assert!(!is_topic_channel("Mira Sol"));
        assert_eq!(p("Afterglow", "Mira Sol - Topic").artist, "Mira Sol");
    }

    fn kept(title: &str, artist: &str, album: Option<&str>) -> Wanted {
        Wanted {
            title: title.into(),
            artist: artist.into(),
            album: album.map(str::to_string),
            duration_ms: Some(200_000),
            ..Wanted::default()
        }
    }

    #[test]
    fn a_youtube_row_is_asked_for_as_the_song_it_names() {
        // An upload: the song from its title, the video's length not vouched for; the row as kept, and the other way
        // round, to fall back on.
        let (asked, also) = asked_for(kept(
            "Planet Caravan - Black Sabbath",
            "Some Uploader",
            None,
        ));
        assert_eq!(
            (
                asked.artist.as_str(),
                asked.title.as_str(),
                asked.duration_ms
            ),
            ("Planet Caravan", "Black Sabbath", None)
        );
        assert_eq!(also.len(), 2);
        assert_eq!(
            (also[0].artist.as_str(), also[0].title.as_str()),
            ("Some Uploader", "Planet Caravan - Black Sabbath")
        );
        assert_eq!(
            (also[1].artist.as_str(), also[1].title.as_str()),
            ("Black Sabbath", "Planet Caravan")
        );
        // The artist's own audio keeps its length.
        let (asked, _) = asked_for(kept("Afterglow", "Mira Sol - Topic", None));
        assert_eq!(
            (asked.artist.as_str(), asked.duration_ms),
            ("Mira Sol", Some(200_000))
        );
        assert_eq!(
            asked_for(kept("Afterglow", "Mira Sol", Some("Lines")))
                .0
                .duration_ms,
            Some(200_000)
        );
        // A clean row is asked for as it is, with nothing to fall back on.
        let (asked, also) = asked_for(kept("Afterglow", "Mira Sol", Some("Lines")));
        assert_eq!(asked, kept("Afterglow", "Mira Sol", Some("Lines")));
        assert!(also.is_empty());
        // One title in two scripts keeps its artist.
        let (asked, _) = asked_for(kept("パス・トゥ・ハート - Path To Heart", "FLat7", None));
        assert_eq!(
            (asked.artist.as_str(), asked.title.as_str()),
            ("FLat7", "パス・トゥ・ハート - Path To Heart")
        );
        // A performance named beside the would-be artist is not read the other way round.
        let (_, also) = asked_for(kept(
            "Song - Band (Live at the Hall, 1989)",
            "Uploader",
            None,
        ));
        assert!(
            also.iter()
                .all(|w| w.artist != "Band (Live at the Hall, 1989)")
        );
    }
}
