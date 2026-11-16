//! A song's credit as the artists it names ("A feat. B", "A & B", "A, B and C"), each with its artist in the library
//! when the library knows them by themselves.
//!
//! A credit is split where it surely names several artists ("feat.", "ft.", "featuring", ";", " / "). Where it may
//! (",", "&", "and", "x", "with", "vs."), it is split only as far as that names artists the library knows by themselves
//! (some song is credited to that name alone): "Drake & Future" splits when either is known, while "Earth, Wind & Fire"
//! and "Simon & Garfunkel" stay whole. The credit itself is an artist of the library too (every song's credit is one),
//! so it never counts as known.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::Result;
use crate::library::{Library, sort_key};

/// One artist a credit names: `before`, the name, then `after` give the credit back as it was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreditPart {
    /// The words between it and the artist before (" feat. ", " & "); empty for the first.
    pub before: String,
    pub name: String,
    /// What closes it (the ")" of "A (feat. B)"); usually empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub after: String,
    /// The artist in the library, when it knows them by themselves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
}

/// Where a credit surely names several artists.
static SURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*\(\s*(?:feat\.?|ft\.?|featuring)\s+|\s+(?:feat\.?|ft\.?|featuring)\s+|\s*;\s*|\s+/\s+")
        .expect("a valid pattern")
});
/// Where it may.
static MAYBE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*,\s*|\s+(?:&|and|x|with|vs\.?)\s+").expect("a valid pattern")
});

/// The most pieces a credit is split into; a longer list of names is left whole.
const MOST_PIECES: usize = 12;

/// A stretch of the credit between two separators, with the separator before it.
#[derive(Debug, Clone)]
struct Piece {
    before: String,
    sure: bool,
    text: String,
}

/// The credit cut at every separator, sure or not.
fn pieces(credit: &str) -> Vec<Piece> {
    let mut cuts: Vec<(usize, usize, bool)> = SURE
        .find_iter(credit)
        .map(|m| (m.start(), m.end(), true))
        .collect();
    for m in MAYBE.find_iter(credit) {
        // A sure separator wins where the two overlap (" (feat. " holds a space MAYBE does not care for).
        if !cuts.iter().any(|&(s, e, _)| m.start() < e && s < m.end()) {
            cuts.push((m.start(), m.end(), false));
        }
    }
    cuts.sort_unstable();
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let (mut at, mut before, mut sure) = (0, String::new(), true);
    for (start, end, is_sure) in cuts {
        // A separator at the very start, or two in a row, leaves nothing to name: it stays with the text.
        if start == 0 || start <= at {
            continue;
        }
        out.push(Piece {
            before: std::mem::take(&mut before),
            sure,
            text: credit[at..start].to_string(),
        });
        before = credit[start..end].to_string();
        sure = is_sure;
        at = end;
    }
    let rest = &credit[at..];
    match out.last_mut() {
        // It ends with a separator ("A; B;"): that stays with the last name.
        Some(last) if rest.trim().is_empty() => {
            last.text.push_str(&before);
            last.text.push_str(rest);
        }
        _ => out.push(Piece {
            before,
            sure,
            text: rest.to_string(),
        }),
    }
    out
}

impl Library {
    /// The artists `credit` names, in order, each with its artist in the library when it is known by itself. Empty when
    /// the credit names one artist (or none could be told apart).
    pub fn credit_parts(&self, credit: &str) -> Result<Vec<CreditPart>> {
        self.credit_parts_with(credit, &mut HashMap::new())
    }

    /// [`Library::credit_parts`], with what is known of each name kept in `known` for the next credit.
    pub fn credit_parts_with(
        &self,
        credit: &str,
        known: &mut HashMap<String, Option<i64>>,
    ) -> Result<Vec<CreditPart>> {
        let credit = credit.trim();
        let pieces = pieces(credit);
        if pieces.len() < 2 || pieces.len() > MOST_PIECES {
            return Ok(Vec::new());
        }
        let whole = sort_key(credit);
        let mut lookup = |name: &str| -> Result<Option<i64>> {
            let key = sort_key(name);
            if key.is_empty() || key == whole {
                return Ok(None);
            }
            if let Some(id) = known.get(&key) {
                return Ok(*id);
            }
            let id = self
                .conn
                .prepare_cached(
                    "SELECT a.id FROM artists a WHERE a.sort_key = ?1
                       AND (EXISTS (SELECT 1 FROM tracks t WHERE t.artist_id = a.id)
                         OR EXISTS (SELECT 1 FROM albums al WHERE al.artist_id = a.id))
                     ORDER BY lower(a.name) = lower(?2) DESC, a.id LIMIT 1",
                )?
                .query_row(rusqlite::params![key, name.trim()], |r| r.get(0))
                .optional()?;
            known.insert(key, id);
            Ok(id)
        };
        // Runs of pieces between sure separators, each split only as far as it names known artists.
        let mut parts = Vec::new();
        let mut start = 0;
        for end in 1..=pieces.len() {
            if end == pieces.len() || pieces[end].sure {
                parts.extend(segment(&pieces[start..end], &mut lookup)?);
                start = end;
            }
        }
        if parts.len() < 2 {
            return Ok(Vec::new());
        }
        Ok(parts)
    }
}

/// The best way found to cut the first pieces of a run: how many known artists it names, in how many parts, where its
/// last part starts, and that part's artist.
type Cut = (usize, usize, usize, Option<i64>);

/// A run of pieces (no sure separator inside it) as the artists it names: of every way to cut it, the one that names
/// the most known artists, and of those the one with the fewest parts (so nothing is cut that names no one known).
fn segment(
    run: &[Piece],
    lookup: &mut impl FnMut(&str) -> Result<Option<i64>>,
) -> Result<Vec<CreditPart>> {
    let mut run = run.to_vec();
    let n = run.len();
    // "A (feat. B, C)": the ")" closes the run; it is no part of the last name.
    let mut closing = String::new();
    if run[0].before.contains('(') {
        let last = &mut run[n - 1].text;
        let kept = last.trim_end().len();
        if last[..kept].ends_with(')') && last.matches(')').count() > last.matches('(').count() {
            closing = last[kept - 1..].to_string();
            last.truncate(kept - 1);
        }
    }
    // The text from piece `j` up to (not including) piece `i`, as written.
    let text = |j: usize, i: usize| -> String {
        let mut out = run[j].text.clone();
        for p in &run[j + 1..i] {
            out.push_str(&p.before);
            out.push_str(&p.text);
        }
        out
    };
    // best[i]: the best way to cut the first i pieces: (known artists, parts), and where its last part starts.
    let mut best: Vec<Option<Cut>> = vec![None; n + 1];
    best[0] = Some((0, 0, 0, None));
    for i in 1..=n {
        for j in 0..i {
            let Some((known, count, _, _)) = best[j] else {
                continue;
            };
            let id = lookup(&text(j, i))?;
            let candidate = (known + usize::from(id.is_some()), count + 1, j, id);
            let better = match best[i] {
                None => true,
                Some((k, c, _, _)) => candidate.0 > k || (candidate.0 == k && candidate.1 < c),
            };
            if better {
                best[i] = Some(candidate);
            }
        }
    }
    let mut parts = Vec::new();
    let mut i = n;
    while i > 0 {
        let (_, _, j, id) = best[i].expect("every length can be cut");
        let mut name = text(j, i);
        let trimmed = name.trim_end().len();
        let mut after = name[trimmed..].to_string();
        name.truncate(trimmed);
        if i == n {
            after.push_str(&closing);
        }
        parts.push(CreditPart {
            before: run[j].before.clone(),
            name,
            after,
            id,
        });
        i = j;
    }
    parts.reverse();
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::Song;

    /// A library whose songs are credited to these names, one song each.
    fn library(credits: &[&str]) -> Library {
        let mut lib = Library::open_in_memory().unwrap();
        let songs: Vec<Song> = credits
            .iter()
            .enumerate()
            .map(|(n, credit)| Song {
                title: format!("Song {n}"),
                artist: (*credit).to_string(),
                ..Song::default()
            })
            .collect();
        lib.adopt_songs(&songs).unwrap();
        lib
    }

    /// The credit's parts as `name#known` (`?` when the library does not know them by themselves).
    fn parts(lib: &Library, credit: &str) -> Vec<String> {
        lib.credit_parts(credit)
            .unwrap()
            .into_iter()
            .map(|p| format!("{}{}", p.name, if p.id.is_some() { "" } else { "?" }))
            .collect()
    }

    #[test]
    fn a_credit_is_split_as_far_as_it_names_known_artists() {
        let lib = library(&[
            "Drake",
            "Future",
            "Tyler, The Creator",
            "Earth, Wind & Fire",
            "Simon & Garfunkel",
            "Drake & Future",
        ]);
        // Both known, or one: split. The other is named, not known.
        assert_eq!(parts(&lib, "Drake & Future"), ["Drake", "Future"]);
        assert_eq!(
            parts(&lib, "Drake, Future and Newcomer"),
            ["Drake", "Future", "Newcomer?"]
        );
        // A band's own name holds no artist known by themselves: whole.
        assert!(parts(&lib, "Earth, Wind & Fire").is_empty());
        assert!(parts(&lib, "Simon & Garfunkel").is_empty());
        assert!(parts(&lib, "Crosby, Stills, Nash & Young").is_empty());
        // A known name with a comma in it stays one.
        assert_eq!(
            parts(&lib, "Tyler, The Creator & Drake"),
            ["Tyler, The Creator", "Drake"]
        );
        // "feat." always splits, known or not; the case of the words does not matter.
        assert_eq!(parts(&lib, "Newcomer feat. drake"), ["Newcomer?", "drake"]);
        assert_eq!(
            parts(&lib, "Someone ft. Someone Else"),
            ["Someone?", "Someone Else?"]
        );
        // One name: nothing to split.
        assert!(parts(&lib, "Drake").is_empty());
        assert!(parts(&lib, "Andrew & ").is_empty());
    }

    #[test]
    fn an_artist_has_their_songs_with_others() {
        let lib = library(&[
            "Drake",
            "Future",
            "Drake & Future",
            "Newcomer feat. DRAKE",
            "Simon & Garfunkel",
            "Drakeo the Ruler",
            "The Weeknd",
            "Drake, The Weeknd",
        ]);
        let id = |name: &str| -> i64 {
            lib.conn
                .query_row("SELECT id FROM artists WHERE name = ?1", [name], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let artists = |name: &str| -> Vec<String> {
            let mut out: Vec<String> = lib
                .artist_with_others(id(name), 50)
                .unwrap()
                .into_iter()
                .map(|r| r.artist)
                .collect();
            out.sort();
            out
        };
        // Not their own songs, nor a name that only holds theirs ("Drakeo the Ruler").
        assert_eq!(
            artists("Drake"),
            [
                "Drake & Future",
                "Drake, The Weeknd",
                "Newcomer feat. DRAKE"
            ]
        );
        assert_eq!(artists("Future"), ["Drake & Future"]);
        assert_eq!(artists("The Weeknd"), ["Drake, The Weeknd"]);
        assert!(artists("Simon & Garfunkel").is_empty());
    }

    #[test]
    fn the_parts_give_the_credit_back_as_written() {
        let lib = library(&["Drake", "Future"]);
        for credit in [
            "Drake & Future",
            "Drake (feat. Future)",
            "Drake feat. Future, Newcomer",
            "Drake; Future",
            "Drake / Future x Newcomer",
            "Drake (feat. Future, Newcomer)",
            "Drake (Ft. Future (UK) )",
            "Drake; Future;",
        ] {
            let parts = lib.credit_parts(credit).unwrap();
            assert!(parts.len() >= 2, "{credit}: {parts:?}");
            let back: String = parts
                .iter()
                .map(|p| format!("{}{}{}", p.before, p.name, p.after))
                .collect();
            assert_eq!(back, credit, "{parts:?}");
        }
        let feat = lib.credit_parts("Drake (feat. Future)").unwrap();
        assert_eq!(
            (
                feat[1].before.as_str(),
                feat[1].name.as_str(),
                feat[1].after.as_str()
            ),
            (" (feat. ", "Future", ")")
        );
    }
}
