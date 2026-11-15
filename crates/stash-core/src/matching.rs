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
];

/// A title as it is compared: folded, and without a trailing release label ("Song - 2011 Remaster", "Song (Mono)",
/// "Song [Deluxe Edition]"). A bracket or a dash part that says anything else stays: it names a version.
fn title_key(title: &str) -> String {
    let mut rest = title.trim();
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

/// "2011 Remaster", "Remastered 2009", "Mono", "Deluxe Edition", "Album Version": only label words and years.
fn is_release_label(label: &str) -> bool {
    let folded = fold(label);
    // "Original Mix" is how dance music says "the song itself".
    if folded == "original mix" {
        return true;
    }
    let year_or_count = |w: &str| {
        let digits = w.strip_suffix("th").unwrap_or(w);
        (1..=4).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
    };
    let mut words = folded.split(' ').filter(|w| !w.is_empty()).peekable();
    words.peek().is_some() && words.all(|w| RELEASE_LABELS.contains(&w) || year_or_count(w))
}

/// The artists a credit names: "A, B & C feat. D" is A, B, C and D.
fn artists(credit: &str) -> Vec<String> {
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

/// Whether the two credits are the same artist: the whole credit, or the first artist named in each.
fn same_artist(wanted: &str, theirs: &str) -> bool {
    if fold(wanted) == fold(theirs) {
        return true;
    }
    let (wanted, theirs) = (artists(wanted), artists(theirs));
    match (wanted.first(), theirs.first()) {
        // The song's main artist leads one credit and is named in the other.
        (Some(first), Some(other)) => {
            first == other || theirs.contains(first) && wanted.contains(other)
        }
        _ => false,
    }
}

/// Whether `candidate` is the song `wanted`, by the rule above. With `Some`, how far its length is from the wanted
/// one in milliseconds (0 when one of them is not known).
fn same_song(wanted: &Wanted, candidate: &Track) -> Option<u64> {
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
        return Some(apart);
    }
    (title_key(&wanted.title) == title_key(&candidate.title)
        && same_artist(&wanted.artist, &candidate.artist))
    .then_some(apart)
}

/// The search result that is the song `wanted`, or `None`. Among several that are, the explicit or clean twin that
/// was asked for, then the one closest in length, then the first.
pub fn pick<'t>(wanted: &Wanted, found: &'t [Track]) -> Option<&'t Track> {
    found
        .iter()
        .enumerate()
        .filter_map(|(place, track)| Some((same_song(wanted, track)?, place, track)))
        .min_by_key(|(apart, place, track)| {
            let other_twin = matches!((wanted.explicit, track.explicit), (Some(asked), Some(theirs)) if asked != theirs);
            (other_twin, *apart, *place)
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
}
