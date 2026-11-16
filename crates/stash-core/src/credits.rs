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

/// A credit with this many places it may be cut, or more, is left whole (a long list of names, or text that only
/// looks like one).
const MOST_PIECES: usize = 12;

/// A place the credit may be cut: the separator's bytes, and whether it surely parts two artists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cut {
    start: usize,
    end: usize,
    sure: bool,
}

/// Every place the credit may be cut, in order. Places that may be cut can overlap: the spaces after one word may be
/// the spaces before the next ("Lil Nas X & Jack Harlow" has " X " and " & "), so each search for one goes on from the
/// spaces that end the one before. Two marks that share their spaces and part the same two names (", and", ", &", a
/// doubled ", ,") may also be cut as one. A sure separator wins where the two overlap.
fn cuts(credit: &str) -> Vec<Cut> {
    let mut cuts: Vec<Cut> = SURE
        .find_iter(credit)
        .map(|m| Cut {
            start: m.start(),
            end: m.end(),
            sure: true,
        })
        .collect();
    let mut at = 0;
    let mut before: Option<(usize, usize, String)> = None;
    while let Some(m) = MAYBE.find_at(credit, at) {
        let mark = m.as_str().trim().to_lowercase();
        if !cuts
            .iter()
            .any(|c| c.sure && m.start() < c.end && c.start < m.end())
        {
            cuts.push(Cut {
                start: m.start(),
                end: m.end(),
                sure: false,
            });
            // ", and" (and ", &", ", ,"): the two marks together part the names on either side.
            if let Some((start, end, first)) = &before
                && m.start() < *end
                && (*first == mark || (first == "," && (mark == "&" || mark == "and")))
            {
                cuts.push(Cut {
                    start: *start,
                    end: m.end(),
                    sure: false,
                });
            }
        }
        before = Some((m.start(), m.end(), mark));
        at = m.start() + m.as_str().trim_end().len();
    }
    cuts.sort_by_key(|c| (c.start, c.end));
    cuts
}

/// A stretch of the credit with no sure separator in it: its bytes, the separator before it, and what closes it (the
/// ")" of "A (feat. B)").
#[derive(Debug)]
struct Run {
    start: usize,
    end: usize,
    before: String,
    closing: String,
}

/// The credit as its runs, each with the places inside it that may be cut.
fn runs(credit: &str, cuts: &[Cut]) -> Vec<(Run, Vec<Cut>)> {
    // The sure separators that part names: not one at the very start, nor one that ends the credit ("A; B;": it stays
    // with the last name). One right after another (nothing to name between: "A; ; B") is one with it.
    let mut bounds: Vec<Cut> = Vec::new();
    let mut at = 0;
    for cut in cuts.iter().filter(|c| c.sure) {
        match bounds.last_mut() {
            Some(last) if cut.start <= at || credit[at..cut.start].trim().is_empty() => {
                last.end = last.end.max(cut.end);
                at = last.end;
            }
            _ if cut.start > at => {
                bounds.push(*cut);
                at = cut.end;
            }
            _ => {}
        }
    }
    if bounds
        .last()
        .is_some_and(|c| credit[c.end..].trim().is_empty())
    {
        bounds.pop();
    }
    let mut runs = Vec::new();
    let (mut start, mut before) = (0, String::new());
    for bound in bounds.iter().map(Some).chain([None]) {
        let end = bound.map_or(credit.len(), |c| c.start);
        runs.push(Run {
            start,
            end,
            before: std::mem::take(&mut before),
            closing: String::new(),
        });
        if let Some(c) = bound {
            before = credit[c.start..c.end].to_string();
            start = c.end;
        }
    }
    // "A (feat. B)": the ")" closes the group the "(" opened, in whichever run it falls ("A (feat. B; C)"). What comes
    // after it ("A (feat. B) & C") is a name of its own, when a separator comes first.
    let mut open = false;
    let mut i = 0;
    while i < runs.len() {
        open |= runs[i].before.contains('(');
        if open
            && let Some(q) = closer(&credit[runs[i].start..runs[i].end]).map(|q| runs[i].start + q)
        {
            open = false;
            let end = runs[i].end;
            let r = q + credit[q..end].bytes().take_while(|&b| b == b')').count();
            if credit[r..end].trim().is_empty() {
                runs[i].closing = credit[q..end].to_string();
                runs[i].end = q;
            } else if let Some(next) = cuts.iter().find(|c| !c.sure && c.start == r) {
                runs[i].closing = credit[q..r].to_string();
                runs[i].end = q;
                runs.insert(
                    i + 1,
                    Run {
                        start: next.end,
                        end,
                        before: credit[next.start..next.end].to_string(),
                        closing: String::new(),
                    },
                );
            }
        }
        i += 1;
    }
    runs.into_iter()
        .map(|run| {
            let inside = cuts
                .iter()
                .filter(|c| !c.sure && c.start >= run.start && c.end <= run.end)
                .copied()
                .collect();
            (run, inside)
        })
        .collect()
}

/// Where the ")" is that closes a "(" just before `text`: the first one not closing a "(" in it.
fn closer(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(i),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
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
        let cuts = cuts(credit);
        if cuts.is_empty() || cuts.len() >= MOST_PIECES {
            return Ok(Vec::new());
        }
        let whole = sort_key(credit);
        let mut lookup = |name: &str| -> Result<Option<i64>> {
            let key = sort_key(name);
            if key.is_empty() || key == whole {
                return Ok(None);
            }
            // What the lookup answers depends on the name as written too (the one written the same way comes first,
            // as SQLite folds case).
            let asked = format!("{key}|{}", name.trim().to_ascii_lowercase());
            if let Some(id) = known.get(&asked) {
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
            known.insert(asked, id);
            Ok(id)
        };
        // Each run is split only as far as it names known artists.
        let mut parts = Vec::new();
        for (run, inside) in runs(credit, &cuts) {
            parts.extend(segment(credit, &run, &inside, &mut lookup)?);
        }
        if parts.len() < 2 {
            return Ok(Vec::new());
        }
        Ok(parts)
    }
}

/// The best way found to reach a place where a part may start: how many known artists it names, in how many parts,
/// the place it came from, and the artist of the part that ends there.
type Best = (usize, usize, usize, Option<i64>);

/// A run as the artists it names: of every way to cut it (at no two places that overlap), the one that names the most
/// known artists, and of those the one with the fewest parts (so nothing is cut that names no one known).
fn segment(
    credit: &str,
    run: &Run,
    cuts: &[Cut],
    lookup: &mut impl FnMut(&str) -> Result<Option<i64>>,
) -> Result<Vec<CreditPart>> {
    let n = cuts.len();
    // Place 0 is the run's start, place k + 1 is after its cut k, and place n + 1 its end.
    let from = |place: usize| {
        if place == 0 {
            run.start
        } else {
            cuts[place - 1].end
        }
    };
    let to = |place: usize| {
        if place == n + 1 {
            run.end
        } else {
            cuts[place - 1].start
        }
    };
    let mut best: Vec<Option<Best>> = vec![None; n + 2];
    best[0] = Some((0, 0, 0, None));
    // A part leads only to a cut that starts where it ends or later, which comes later in the cuts' order (by start,
    // then end): each place is settled before any part leads on from it.
    for place in 0..=n {
        let Some((known, count, _, _)) = best[place] else {
            continue;
        };
        let start = from(place);
        for next in (1..=n + 1).filter(|&next| next == n + 1 || cuts[next - 1].start >= start) {
            let text = &credit[start..to(next)];
            if text.trim().is_empty() {
                continue;
            }
            let id = lookup(text)?;
            let candidate = (known + usize::from(id.is_some()), count + 1, place, id);
            let better = best[next].is_none_or(|(k, c, _, _)| {
                candidate.0 > k || (candidate.0 == k && candidate.1 < c)
            });
            if better {
                best[next] = Some(candidate);
            }
        }
    }
    // Nothing in it to name (only spaces): kept as written.
    if best[n + 1].is_none() {
        return Ok(vec![CreditPart {
            before: run.before.clone(),
            name: String::new(),
            after: format!("{}{}", &credit[run.start..run.end], run.closing),
            id: None,
        }]);
    }
    let mut parts = Vec::new();
    let mut place = n + 1;
    while place > 0 {
        let (_, _, came, id) = best[place].expect("every place on the way was reached");
        let text = &credit[from(came)..to(place)];
        let name = text.trim_end();
        let mut after = text[name.len()..].to_string();
        if place == n + 1 {
            after.push_str(&run.closing);
        }
        parts.push(CreditPart {
            before: if came == 0 {
                run.before.clone()
            } else {
                credit[cuts[came - 1].start..cuts[came - 1].end].to_string()
            },
            name: name.to_string(),
            after,
            id,
        });
        place = came;
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
    fn names_that_end_in_a_joining_word_and_a_feat_that_closes_early_are_split_too() {
        let lib = library(&["Lil Nas X", "Jack Harlow", "Drake", "Future", "Ben With"]);
        // The space after "X" is the space before "&".
        assert_eq!(
            parts(&lib, "Lil Nas X & Jack Harlow"),
            ["Lil Nas X", "Jack Harlow"]
        );
        assert_eq!(
            parts(&lib, "Lil Nas X and Jack Harlow"),
            ["Lil Nas X", "Jack Harlow"]
        );
        assert_eq!(
            parts(&lib, "Jack Harlow and Lil Nas X"),
            ["Jack Harlow", "Lil Nas X"]
        );
        assert_eq!(parts(&lib, "Ben With x Drake"), ["Ben With", "Drake"]);
        assert!(parts(&lib, "Lil Nas X").is_empty());
        // The ")" closes the feat. where it falls; what comes after it is a name of its own.
        assert_eq!(
            parts(&lib, "Drake (feat. Future) & Newcomer"),
            ["Drake", "Future", "Newcomer?"]
        );
        let feat = lib.credit_parts("Drake (feat. Future), Newcomer").unwrap();
        assert_eq!(
            feat.iter()
                .map(|p| (p.before.as_str(), p.name.as_str(), p.after.as_str()))
                .collect::<Vec<_>>(),
            [
                ("", "Drake", ""),
                (" (feat. ", "Future", ")"),
                (", ", "Newcomer", "")
            ]
        );
        // The ")" closes the group in whichever part it falls ("A (feat. B; C)").
        for credit in [
            "Drake (feat. Future; Newcomer)",
            "Drake (feat. Future / Newcomer)",
        ] {
            let parts = lib.credit_parts(credit).unwrap();
            assert_eq!(
                parts
                    .iter()
                    .map(|p| (p.name.as_str(), p.after.as_str()))
                    .collect::<Vec<_>>(),
                [("Drake", ""), ("Future", ""), ("Newcomer", ")")],
                "{credit}"
            );
        }
    }

    #[test]
    fn two_marks_between_the_same_names_part_them_as_one() {
        let lib = library(&[
            "Ada Vale",
            "Ben Okoro",
            "Cy Marsh",
            "Drake",
            "Future",
            "Ben And",
            "Ben Vs",
        ]);
        // The Oxford comma: ", and", ", &".
        assert_eq!(
            parts(&lib, "Ada Vale, Ben Okoro, and Cy Marsh"),
            ["Ada Vale", "Ben Okoro", "Cy Marsh"]
        );
        assert_eq!(
            parts(&lib, "Ada Vale, Ben Okoro, & Cy Marsh"),
            ["Ada Vale", "Ben Okoro", "Cy Marsh"]
        );
        // A mark written twice (a list with an empty entry).
        assert_eq!(parts(&lib, "Drake, , Future"), ["Drake", "Future"]);
        assert_eq!(parts(&lib, "Drake & & Future"), ["Drake", "Future"]);
        assert_eq!(parts(&lib, "Drake; ; Future"), ["Drake", "Future"]);
        // Names that end in "and" or "vs".
        assert_eq!(parts(&lib, "Ben And & Drake"), ["Ben And", "Drake"]);
        assert_eq!(parts(&lib, "Ben Vs & Drake"), ["Ben Vs", "Drake"]);
    }

    #[test]
    fn a_name_is_looked_up_as_written_whatever_was_looked_up_before() {
        // "Weeknd" and "The Weeknd" sort alike; each credit's link is its own artist, in whichever order they are read.
        let lib = library(&["Drake", "Weeknd", "The Weeknd"]);
        let id = |name: &str| -> i64 {
            lib.conn
                .query_row("SELECT id FROM artists WHERE name = ?1", [name], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let mut known = HashMap::new();
        let second = |parts: Vec<CreditPart>| parts[1].id;
        assert_eq!(
            second(lib.credit_parts_with("Drake, Weeknd", &mut known).unwrap()),
            Some(id("Weeknd"))
        );
        assert_eq!(
            second(
                lib.credit_parts_with("Drake, The Weeknd", &mut known)
                    .unwrap()
            ),
            Some(id("The Weeknd"))
        );
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
    fn an_artists_songs_with_others_are_found_past_many_names_that_only_hold_theirs() {
        // More credits hold "drake" inside a longer name than are read, and come first: the one that names Drake is
        // still found.
        let mut credits: Vec<String> = (0..crate::library::CREDITS_READ + 10)
            .map(|n| format!("Drakeo & Guest {n}"))
            .collect();
        credits.extend([
            "Drake".to_string(),
            "Future".to_string(),
            "Drake & Future".to_string(),
        ]);
        let names: Vec<&str> = credits.iter().map(String::as_str).collect();
        let lib = library(&names);
        let drake: i64 = lib
            .conn
            .query_row("SELECT id FROM artists WHERE name = 'Drake'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let found: Vec<String> = lib
            .artist_with_others(drake, 50)
            .unwrap()
            .into_iter()
            .map(|r| r.artist)
            .collect();
        assert_eq!(found, ["Drake & Future"]);
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
            "Drake (feat. Future) & Newcomer",
            "Drake (feat. Future)) & Newcomer",
            "Lil Nas X & Future x Drake",
            "Drake (feat. ) & Future",
            "Drake; ; Future",
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
