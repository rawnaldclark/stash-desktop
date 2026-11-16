//! Finding a song the library knows among an add-on's search results, for an add-on that can search but cannot be
//! asked "do you have this song".
//!
//! The rule is the contract's own for what "the same song" is: the same performance by the same artist. Case,
//! punctuation, accents and featuring credits make no difference; a release label (a remaster, mono or stereo,
//! deluxe, album or single version, explicit or clean) is not a version; a live take, a remix, an acoustic,
//! instrumental, karaoke, sped-up or slowed version, a re-recording, a cover and an edit are other songs. A length
//! that differs by more than 8 s or 3 % is another recording.
//!
//! It is strict on purpose: a song that is not found plays nothing and says so; a wrong version plays as if it were
//! right.

use crate::addons::{Track, Wanted};
use crate::identity::fold;

/// Words that, alone in a bracket or after a dash, label a release and not a version.
const RELEASE_LABELS: &[&str] = &[
    "remaster",
    "remastered",
    "remastering",
    "mono",
    "stereo",
    "deluxe",
    "edition",
    "version",
    "album",
    "single",
    "explicit",
    "clean",
    "original",
    "bonus",
    "track",
    "anniversary",
    "expanded",
    "digital",
    // What a video's title says of itself: the song is the video's sound.
    "official",
    "video",
    "audio",
    "music",
    "lyric",
    "lyrics",
    "visualizer",
    "hd",
    "hq",
    "4k",
    "mv",
];

/// Words that make a part of a title name another version (used where a part of a title is dropped).
const VERSION_WORDS: &[&str] = &[
    "live",
    "remix",
    "mix",
    "acoustic",
    "instrumental",
    "karaoke",
    "cover",
    "demo",
    "edit",
    "session",
    "sped",
    "slowed",
    "reprise",
    "rework",
    "unplugged",
    "orchestral",
    "piano",
    "rehearsal",
    "take",
    "takes",
    "sessions",
    "extended",
    "alternate",
    "early",
    "outtake",
    "outtakes",
    "recorded",
    "rerecorded",
    "recording",
    "stripped",
    "symphonic",
    "cappella",
    "acapella",
    "acappella",
    "performance",
    "concert",
    "dub",
    "club",
    "vip",
    "bootleg",
    "mashup",
    "nightcore",
    "lofi",
];

/// Whether a part of a title names another version ("Live", "Instrumental", "MTV Unplugged").
fn names_version(part: &str) -> bool {
    fold(part).split(' ').any(|w| VERSION_WORDS.contains(&w))
}

/// Whether a title names another version of a song in a part of its own: in brackets ("Song (Live)", "Song [Remix]")
/// or after a dash ("Song - Acoustic"). A release label ("Song - 2011 Remaster") is not another version.
pub fn names_other_version(title: &str) -> bool {
    let mut parts: Vec<&str> = Vec::new();
    for (open, close) in [('(', ')'), ('[', ']')] {
        let mut rest = title;
        while let Some(at) = rest.find(open) {
            let inner = &rest[at + 1..];
            let end = inner.find(close).unwrap_or(inner.len());
            parts.push(&inner[..end]);
            rest = &inner[end..];
        }
    }
    parts.extend(title.split(" - ").skip(1));
    parts
        .into_iter()
        .any(|part| names_version(part) && !is_release_label(part))
}

/// Whether two titles name the same song, as matching compares them: "Song - 2011 Remaster" is "Song", and
/// "Song (Live)" is not.
pub fn same_title(a: &str, b: &str) -> bool {
    title_key(a) == title_key(b)
}

/// A title as it is compared: folded, and without a trailing release label ("Song - 2011 Remaster", "Song (Mono)",
/// "Song [Deluxe Edition]"). A bracket or a dash part that says anything else stays: it names a version. In a bracket
/// of parts joined by a slash, the parts that are labels go and the rest stays ("(Radio Edit / Remastered 2025)" is
/// "(Radio Edit)").
pub fn title_key(title: &str) -> String {
    let unslashed = without_label_parts(&without_feat_tail(title.trim()));
    let mut rest = unslashed.as_str();
    loop {
        let cut = if rest.ends_with(')') || rest.ends_with(']') {
            let open = if rest.ends_with(')') { '(' } else { '[' };
            rest.rfind(open)
                .map(|at| (at, &rest[at + 1..rest.len() - 1]))
        } else {
            rest.rfind(" - ").map(|at| (at, &rest[at + 3..]))
        };
        match cut {
            Some((at, label)) if at > 0 && is_release_label(label) => rest = rest[..at].trim_end(),
            _ => break,
        }
    }
    fold(rest)
}

/// "2011 Remaster", "Remastered 2009", "Mono", "Deluxe Edition", "Album Version": only label words and years. Also
/// a remaster by whoever did it ("Rudy Van Gelder Remaster 2007"), where the song is from ("From \"Porco Rosso\""),
/// and parts joined by a slash when each is a label or the rest says the same ("Radio Edit / Remastered 2025" is
/// "Radio Edit").
fn is_release_label(label: &str) -> bool {
    let folded = fold(label);
    // "Original Mix" is how dance music says "the song itself".
    if folded == "original mix" {
        return true;
    }
    let words: Vec<&str> = folded.split(' ').filter(|w| !w.is_empty()).collect();
    if words
        .iter()
        .any(|w| matches!(*w, "remaster" | "remastered" | "remastering"))
        && !words.iter().any(|w| VERSION_WORDS.contains(w))
    {
        return true;
    }
    if words.first() == Some(&"from")
        && words.len() > 1
        && !words.iter().any(|w| VERSION_WORDS.contains(w))
    {
        return true;
    }
    let year_or_count = |w: &str| {
        let digits = w.strip_suffix("th").unwrap_or(w);
        (1..=4).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
    };
    let mut words = folded.split(' ').filter(|w| !w.is_empty()).peekable();
    words.peek().is_some() && words.all(|w| RELEASE_LABELS.contains(&w) || year_or_count(w))
}

/// A title whose last bracket joins parts with a slash, without the parts that are release labels.
/// Parts are joined by " / " or ", " ("(Radio Edit / Remastered 2025)", "(Radio Edit, Remastered 2025)"); a bare
/// year or number is not dropped this way ("(Live, 1970)" is not "(Live)").
fn without_label_parts(title: &str) -> String {
    // The last part: a bracket ("Song (Radio Edit / Remastered 2025)") or what follows the last dash ("Song - Live At
    // Fillmore East, 1970 / 50th Anniversary").
    let (at, inside, open, close) = match title.chars().last() {
        Some(close @ (')' | ']')) => {
            let open = if close == ')' { "(" } else { "[" };
            let Some(at) = title.rfind(open) else {
                return title.to_string();
            };
            (
                at,
                &title[at + 1..title.len() - 1],
                open,
                if close == ')' { ")" } else { "]" },
            )
        }
        _ => {
            let Some(at) = title.rfind(" - ") else {
                return title.to_string();
            };
            (at, &title[at + 3..], " - ", "")
        }
    };
    if !inside.contains(" / ") && !inside.contains(", ") {
        return title.to_string();
    }
    let parts: Vec<&str> = inside
        .split(" / ")
        .flat_map(|part| part.split(", "))
        .map(str::trim)
        .collect();
    let kept: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|part| !is_release_label(part) || !part.chars().any(char::is_alphabetic))
        .collect();
    if kept.len() == parts.len() {
        return title.to_string();
    }
    if kept.is_empty() {
        title[..at].trim_end().to_string()
    } else {
        format!("{}{open}{}{close}", &title[..at], kept.join(" / "))
    }
}

/// A title without a featuring credit written after it, unbracketed ("Potholderz feat. Count Bass D", "LOYALTY. FEAT.
/// RIHANNA."). A bracketed one is already left out by `fold`. The credit ends where a bracket or a dash part begins;
/// what follows stays when it names a version ("Song feat. X (Live)" is "Song (Live)", "Song feat. X (Album Name)" is
/// "Song").
fn without_feat_tail(title: &str) -> String {
    // Lowered letter for letter in ASCII only, so every place in it is the same place in the title.
    let lower = title.to_ascii_lowercase();
    let Some(at) = [" feat. ", " ft. ", " featuring "]
        .iter()
        .filter_map(|credit| lower.find(credit))
        .filter(|&at| at > 0 && !title[..at].contains('(') && !title[..at].contains('['))
        .min()
    else {
        return title.to_string();
    };
    let after = at + 1;
    match [" (", " [", " - "]
        .iter()
        .filter_map(|mark| title[after..].find(mark).map(|i| after + i))
        .min()
    {
        Some(end) if names_version(&title[end..]) => {
            format!("{}{}", title[..at].trim_end(), &title[end..])
        }
        _ => title[..at].trim_end().to_string(),
    }
}

/// A title without a trailing "(with …)" that names artists the credit already has: Spotify writes a song's other
/// artists that way ("Sunday Again (with 2 Chainz)" by 6LACK, 2 Chainz).
fn without_with_credit<'t>(title: &'t str, credit: &str) -> Option<&'t str> {
    let trimmed = title.trim_end();
    let at = trimmed.strip_suffix(')').and_then(|_| trimmed.rfind('('))?;
    let inner = trimmed[at + 1..trimmed.len() - 1].trim();
    let named = inner
        .get(..5)
        .filter(|w| w.eq_ignore_ascii_case("with "))
        .map(|_| &inner[5..])?;
    let credited = artists(credit);
    let theirs = artists(named);
    (!theirs.is_empty()
        && theirs
            .iter()
            .all(|name| credited.iter().any(|c| same_name(c, name))))
    .then(|| trimmed[..at].trim_end())
}

/// The ways a title may be the same song's: as it is; without the artist's name in front ("Cynthoni - HAYABUSA" by
/// Cynthoni); and either side of a title that gives the song's name in two scripts ("紅炎 - Crimson Flames").
fn title_keys(title: &str, credit: &str) -> Vec<String> {
    let mut keys = vec![title_key(title)];
    if let Some(plain) = without_with_credit(title, credit) {
        keys.push(title_key(plain));
    }
    let mut parts = title.splitn(2, " - ");
    if let (Some(left), Some(right)) = (parts.next(), parts.next()) {
        let names = artists(credit);
        let named = fold(left);
        if named == fold(credit) || names.iter().any(|n| same_name(n, &named)) {
            keys.push(title_key(right));
        } else {
            let latin = |s: &str| s.chars().any(|c| c.is_ascii_alphabetic());
            let other_script = |s: &str| s.chars().any(|c| c.is_alphabetic() && !c.is_ascii());
            if ((!latin(left) && other_script(left) && latin(right))
                || (!latin(right) && other_script(right) && latin(left)))
                && !names_version(left)
                && !names_version(right)
            {
                keys.push(fold(left));
                keys.push(title_key(right));
            }
        }
    }
    keys.retain(|k| !k.is_empty());
    keys
}

/// Whether `theirs` is `wanted` with a composer's or a work's name in front, the way classical catalogues write a
/// movement ("Rachmaninoff: Prélude…", "Serenade…, Op. 48 : Tchaikovsky: Serenade…: 1. Pezzo…"). What is in front
/// may not name a version.
fn with_name_in_front(wanted: &str, theirs: &str) -> bool {
    theirs.match_indices(':').any(|(at, _)| {
        let (front, rest) = (&theirs[..at], &theirs[at + 1..]);
        !rest.trim().is_empty() && title_key(rest) == wanted && !names_version(front)
    })
}

/// Two names of one artist: the same, or the same but for a leading "The" or the spaces ("AZ Chike", "AzChike").
fn same_name(a: &str, b: &str) -> bool {
    let plain = |s: &str| s.strip_prefix("the ").unwrap_or(s).replace(' ', "");
    a == b || plain(a) == plain(b)
}

/// The artists a credit names: "A, B & C feat. D" is A, B, C and D.
pub(crate) fn artists(credit: &str) -> Vec<String> {
    let lower = credit.to_lowercase();
    let mut names = vec![lower.as_str()];
    for separator in [
        ",",
        "&",
        ";",
        " x ",
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " with ",
        " and ",
        " / ",
    ] {
        names = names
            .into_iter()
            .flat_map(|name| name.split(separator))
            .collect();
    }
    names
        .into_iter()
        .map(fold)
        .filter(|name| !name.is_empty())
        .collect()
}

/// How two credits are the same artist.
#[derive(Debug, PartialEq)]
enum Credits {
    /// The whole credit, or the first artist named in each.
    Same,
    /// The artist that leads one credit is named somewhere in the other (a library that credits the composer first and
    /// the orchestra after, a catalogue that credits only the orchestra). Weaker: a result whose credit is the same is
    /// preferred ("Interlude" by "J. Cole, Kendrick Lamar" is J. Cole's, not Kendrick Lamar's, when both come up).
    Named,
    Other,
}

fn credits(wanted: &str, theirs: &str) -> Credits {
    if same_name(&fold(wanted), &fold(theirs)) {
        return Credits::Same;
    }
    let (wanted, theirs) = (artists(wanted), artists(theirs));
    let named = |list: &[String], name: &str| list.iter().any(|n| same_name(n, name));
    match (wanted.first(), theirs.first()) {
        (Some(first), Some(other)) if same_name(first, other) => Credits::Same,
        (Some(first), Some(other)) if named(&theirs, first) || named(&wanted, other) => {
            Credits::Named
        }
        _ => Credits::Other,
    }
}

/// Whether `candidate` is the song `wanted`, by the rule above. With `Some`: whether its credit only names the wanted
/// artist somewhere ([`Credits::Named`]), and how far its length is from the wanted one in milliseconds (0 when one
/// of them is not known).
fn same_song(wanted: &Wanted, candidate: &Track) -> Option<(bool, u64)> {
    let apart = match (wanted.duration_ms, candidate.duration_ms) {
        (Some(asked), Some(theirs)) => {
            let apart = asked.abs_diff(theirs);
            if apart > 8_000.max(asked * 3 / 100) {
                return None;
            }
            apart
        }
        _ => 0,
    };
    // The same recording by its code is the same song, whatever it is called there.
    if let (Some(asked), Some(theirs)) = (&wanted.isrc, &candidate.isrc)
        && asked.eq_ignore_ascii_case(theirs)
    {
        return Some((false, apart));
    }
    let named = match credits(&wanted.artist, &candidate.artist) {
        Credits::Same => false,
        Credits::Named => true,
        Credits::Other => return None,
    };
    let theirs = title_keys(&candidate.title, &candidate.artist);
    let ours = title_keys(&wanted.title, &wanted.artist);
    // The same title, or the same with a composer's or a work's name in front on either side.
    let same_title = ours
        .iter()
        .any(|key| theirs.contains(key) || with_name_in_front(key, &candidate.title))
        || theirs
            .iter()
            .any(|key| with_name_in_front(key, &wanted.title));
    same_title.then_some((named, apart))
}

/// The search result that is the song `wanted`, or `None`. Among several that are, the explicit or clean twin that
/// was asked for, then one credited the same way, then the one closest in length, then the first. (Tried on the labelled sets in
/// `tests/matcher_corpus.rs` and dropped: preferring a plain title, or a video's length, when the song's own length is
/// not known, picked more wrong versions than it saved; refusing when such results disagree on length missed many
/// more songs and saved no wrong pick.)
pub fn pick<'t>(wanted: &Wanted, found: &'t [Track]) -> Option<&'t Track> {
    found
        .iter()
        .enumerate()
        .filter_map(|(place, track)| Some((same_song(wanted, track)?, place, track)))
        .min_by_key(|((named, apart), place, track)| {
            let other_twin = matches!((wanted.explicit, track.explicit), (Some(asked), Some(theirs)) if asked != theirs);
            (other_twin, *named, *apart, *place)
        })
        .map(|(_, _, track)| track)
}

/// What to search an add-on for to find `wanted`: the artist and the title, without what only decorates the title.
pub fn query(wanted: &Wanted) -> String {
    let artist = wanted
        .artist
        .split([',', '&'])
        .next()
        .unwrap_or_default()
        .trim();
    format!("{artist} {}", wanted.title.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(title: &str, artist: &str, seconds: Option<u64>) -> Wanted {
        Wanted {
            title: title.into(),
            artist: artist.into(),
            duration_ms: seconds.map(|s| s * 1000),
            ..Wanted::default()
        }
    }

    fn track(id: &str, title: &str, artist: &str, seconds: Option<u64>) -> Track {
        Track {
            id: id.into(),
            title: title.into(),
            artist: artist.into(),
            album: None,
            duration_ms: seconds.map(|s| s * 1000),
            isrc: None,
            explicit: None,
            max_tier: None,
            artwork: Vec::new(),
        }
    }

    fn picked(want: &Wanted, found: &[Track]) -> Option<String> {
        pick(want, found).map(|t| t.id.clone())
    }

    #[test]
    fn the_same_song_is_found_under_its_other_spellings() {
        let want = wanted("Pink + White", "Frank Ocean", Some(184));
        for (title, artist) in [
            ("pink + white", "FRANK OCEAN"),
            ("Pink + White (feat. Beyoncé)", "Frank Ocean"),
            ("Pink + White - 2016 Remaster", "Frank Ocean"),
            ("Pink + White (Album Version)", "Frank Ocean"),
            ("Pink + White [Explicit]", "Frank Ocean"),
            ("Pink + White (Original Mix)", "Frank Ocean"),
            ("Pink + White (25th Anniversary Edition)", "Frank Ocean"),
            ("Pink + White", "Frank Ocean, Beyoncé"),
            ("Pink + White", "Frank Ocean feat. Beyoncé"),
        ] {
            assert_eq!(
                picked(&want, &[track("a", title, artist, Some(185))]).as_deref(),
                Some("a"),
                "{title} / {artist}"
            );
        }
        // The other way round: the library's title carries the label, the add-on's does not.
        assert!(
            pick(
                &wanted("Heroes - 2017 Remaster", "David Bowie", None),
                &[track("a", "Heroes", "David Bowie", Some(371))]
            )
            .is_some()
        );
        assert!(
            pick(
                &wanted("Halo", "Beyoncé", Some(261)),
                &[track("a", "Halo", "Beyonce", None)]
            )
            .is_some()
        );
    }

    #[test]
    fn another_version_another_artist_or_another_length_is_not_the_song() {
        let want = wanted("Pink + White", "Frank Ocean", Some(184));
        for (title, artist, seconds) in [
            ("Pink + White (Live)", "Frank Ocean", 184),
            ("Pink + White - Acoustic", "Frank Ocean", 184),
            ("Pink + White (Sped Up)", "Frank Ocean", 150),
            ("Pink + White (Instrumental)", "Frank Ocean", 184),
            ("Pink + White (Radio Edit)", "Frank Ocean", 184),
            ("Pink + White (Truth)", "Frank Ocean", 184),
            ("Pink + White (Club Mix)", "Frank Ocean", 184),
            ("Pink + White", "A Tribute Band", 184),
            ("Pink + White", "Frank Ocean", 240),
            ("Pink", "Frank Ocean", 184),
            ("White", "Frank Ocean", 184),
        ] {
            assert_eq!(
                picked(&want, &[track("a", title, artist, Some(seconds))]),
                None,
                "{title} / {artist} / {seconds}"
            );
        }
        // A title that only looks like a label is still a title.
        assert!(
            pick(
                &wanted("Stereo", "Pavement", None),
                &[track("a", "Stereo", "Pavement", None)]
            )
            .is_some()
        );
        assert!(
            pick(
                &wanted("Stereo", "Pavement", None),
                &[track("a", "Mono", "Pavement", None)]
            )
            .is_none()
        );
        assert_eq!(picked(&want, &[]), None);
    }

    #[test]
    fn among_several_the_closest_is_taken() {
        let mut want = wanted("Song", "Band", Some(200));
        let found = [
            track("far", "Song", "Band", Some(206)),
            track("near", "Song", "Band", Some(201)),
            track("live", "Song (Live)", "Band", Some(200)),
        ];
        assert_eq!(picked(&want, &found).as_deref(), Some("near"));
        // The explicit twin that was asked for wins over a closer clean one.
        want.explicit = Some(true);
        let mut twins = [
            track("clean", "Song", "Band", Some(200)),
            track("explicit", "Song", "Band", Some(203)),
        ];
        twins[0].explicit = Some(false);
        twins[1].explicit = Some(true);
        assert_eq!(picked(&want, &twins).as_deref(), Some("explicit"));
        // The same ISRC is the same recording, whatever the add-on calls it.
        want.isrc = Some("USUM71607009".into());
        let mut coded = [track(
            "renamed",
            "Song (2011 Mix)",
            "Band & Friends",
            Some(201),
        )];
        coded[0].isrc = Some("usum71607009".into());
        assert_eq!(picked(&want, &coded).as_deref(), Some("renamed"));

        assert_eq!(
            query(&wanted(" Pink + White ", "Frank Ocean, Beyoncé", None)),
            "Frank Ocean Pink + White"
        );
    }

    #[test]
    fn real_catalogues_spell_the_same_song_their_own_ways() {
        let same = |want: Wanted, title: &str, artist: &str, seconds: Option<u64>| {
            pick(&want, &[track("a", title, artist, seconds)]).is_some()
        };
        // The artist's name in front of a video's title.
        assert!(same(
            wanted("Cynthoni - HAYABUSA", "Cynthoni", None),
            "HAYABUSA",
            "Cynthoni",
            Some(211)
        ));
        // A composer's or a work's name in front of a movement.
        assert!(same(
            wanted(
                "Prélude in C-Sharp Minor, Op. 3, No. 2",
                "Sergei Rachmaninoff",
                Some(223)
            ),
            "Rachmaninoff: Prélude in C-Sharp Minor, Op. 3, No. 2",
            "Sergei Rachmaninoff",
            Some(224)
        ));
        // ...but not a version's name.
        assert!(!same(
            wanted("Song", "Band", Some(200)),
            "Live: Song",
            "Band",
            Some(200)
        ));
        // A title in two scripts.
        assert!(same(
            wanted("ロスタイム - Losstime", "Creepy Nuts", Some(219)),
            "Losstime",
            "Creepy Nuts",
            Some(219)
        ));
        // A remaster by whoever did it, where the song is from, and labels joined by a slash.
        assert!(same(
            wanted(
                "My Funny Valentine - Rudy Van Gelder Remaster 2007",
                "Miles Davis",
                Some(358)
            ),
            "My Funny Valentine",
            "Miles Davis",
            Some(360)
        ));
        assert!(same(
            wanted("Blaze Of Glory", "Jon Bon Jovi", Some(340)),
            "Blaze Of Glory (From \"Young Guns II\" Soundtrack)",
            "Jon Bon Jovi",
            Some(335)
        ));
        assert!(same(
            wanted("Life (Radio Edit) (Remastered 2025)", "Haddaway", Some(257)),
            "Life (Radio Edit / Remastered 2025)",
            "Haddaway",
            Some(256)
        ));
        assert!(!same(
            wanted("Life", "Haddaway", Some(257)),
            "Life (Radio Edit / Remastered 2025)",
            "Haddaway",
            Some(256)
        ));
        // The performer credited after the composer; "The" and spaces in a name.
        assert!(same(
            wanted(
                "Serenade for Strings in C, Op. 48",
                "Pyotr Ilyich Tchaikovsky, Berliner Philharmoniker",
                Some(596)
            ),
            "Serenade for Strings in C, Op. 48",
            "Berliner Philharmoniker",
            Some(593)
        ));
        assert!(same(
            wanted("Games People Play", "Spinners", Some(285)),
            "Games People Play",
            "The Spinners",
            Some(280)
        ));
        assert!(same(
            wanted("Look Like My Mama", "AZ Chike", Some(196)),
            "Look Like My Mama",
            "AzChike",
            Some(196)
        ));
        // A video's own words are not a version.
        assert!(same(
            wanted("Hayloft (Official Video)", "Mother Mother", None),
            "Hayloft",
            "Mother Mother",
            Some(181)
        ));
        // Still another artist is another song.
        assert!(!same(
            wanted("Hayloft", "Mother Mother", Some(181)),
            "Hayloft",
            "A Tribute Band",
            Some(181)
        ));
        // Spotify's "(with …)" names artists the credit has; unbracketed featuring credits.
        assert!(same(
            wanted("Sunday Again (with 2 Chainz)", "6LACK, 2 Chainz", Some(220)),
            "Sunday Again",
            "6LACK & 2 Chainz",
            Some(220)
        ));
        assert!(!same(
            wanted("Sunday Again (with Strings)", "6LACK", Some(220)),
            "Sunday Again",
            "6LACK",
            Some(220)
        ));
        assert!(same(
            wanted(
                "LOYALTY. FEAT. RIHANNA.",
                "Kendrick Lamar, Rihanna",
                Some(227)
            ),
            "LOYALTY.",
            "Kendrick Lamar",
            Some(227)
        ));
        assert!(same(
            wanted("Potholderz feat. Count Bass D", "MF DOOM", Some(200)),
            "Potholderz",
            "MF Doom",
            Some(201)
        ));
        // Labels after a dash, as after a bracket; a year stays.
        assert!(same(
            wanted(
                "Machine Gun - Live At Fillmore East, 1970 / 50th Anniversary",
                "Jimi Hendrix",
                Some(759)
            ),
            "Machine Gun (Live At Fillmore East, 1970 / 50th Anniversary)",
            "Jimi Hendrix",
            Some(753)
        ));
        assert!(!same(
            wanted("Song (Live, 1970)", "Band", Some(300)),
            "Song (Live, 1982)",
            "Band",
            Some(300)
        ));
        // The composer in front on the library's side.
        assert!(same(
            wanted(
                "Rachmaninov: 10 Preludes, Op. 23: No. 5 in G Minor",
                "Sergei Rachmaninoff, Nikolai Lugansky",
                Some(227)
            ),
            "10 Preludes, Op. 23: No. 5 in G Minor",
            "Nikolai Lugansky",
            Some(228)
        ));
    }

    #[test]
    fn a_version_is_not_taken_for_the_song_however_it_is_written() {
        let one = |w: &Wanted, title: &str, artist: &str| {
            pick(w, &[track("1", title, artist, None)]).is_some()
        };
        let song = |title: &str, artist: &str| wanted(title, artist, None);
        for (asked, title, artist) in [
            // A version written in Latin beside a title in another script, either way round.
            (song("紅蓮華", "LiSA"), "紅蓮華 - Instrumental", "LiSA"),
            (song("Группа крови", "Кино"), "Группа крови - Live", "Кино"),
            (song("紅蓮華 - Live", "LiSA"), "紅蓮華", "LiSA"),
            // A version written after a featuring credit.
            (song("Song", "A"), "Song feat. X (Live)", "A"),
            (song("Song", "A"), "Song ft. X - Remix", "A"),
            (song("Song feat. X (Acoustic)", "A"), "Song", "A"),
            // Where it is from, when that is a live album; a remaster that is also another take.
            (song("Song", "A"), "Song (From \"MTV Unplugged\")", "A"),
            (song("Song", "A"), "Song - From \"Live at Wembley\"", "A"),
            (
                song("Song", "A"),
                "Song (2015 Remastered Extended Version)",
                "A",
            ),
            (song("Song", "A"), "BBC Sessions: Song", "A"),
        ] {
            assert!(!one(&asked, title, artist), "{title} by {artist}");
        }
        // Still the song: what follows a credit that names no version, a film it is from.
        assert!(one(&song("Song", "A"), "Song feat. X (Album Name)", "A"));
        assert!(one(&song("Song", "A"), "Song (From \"Porco Rosso\")", "A"));
        // Letters whose small form takes other room than the capital do not throw the reading off.
        assert!(one(&song("ẞé", "A"), "ẞé feat. Xİ", "A"));
        // A result credited the same way is preferred to one that names the artist somewhere in its credit.
        let both = [
            track("k", "Interlude", "Kendrick Lamar", None),
            track("j", "Interlude", "J. Cole", None),
        ];
        assert_eq!(
            pick(&song("Interlude", "J. Cole, Kendrick Lamar"), &both).map(|t| t.id.as_str()),
            Some("j")
        );
    }
}
