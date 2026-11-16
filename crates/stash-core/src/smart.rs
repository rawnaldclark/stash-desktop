//! Smart playlists: a playlist that fills itself by rules ("liked, not played in 90 days, least recently played first,
//! at most 50") and is read anew each time it is opened, so it follows the library as it changes. The rules are kept
//! with the playlist as JSON (`playlists.rules`); a playlist with rules has no songs of its own. Every value a
//! listener typed reaches SQLite as a bound value, never as SQL. Text is matched without case or accents
//! ("beyonce" finds "Beyoncé"), through the `stash_plain` function every connection has (`db.rs`).

use rusqlite::types::Value as Sql;
use rusqlite::{OptionalExtension, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::library::{
    TRACK_COLUMNS_FOR_LISTS, TRACK_JOINS_FOR_LISTS, TrackRow, track_row_for_lists,
};
use crate::{Library, Result};

/// At most this many rules in a playlist.
pub const MAX_RULES: usize = 20;
/// The longest text a rule matches, in characters.
const MAX_TEXT: usize = 200;
const DAY_MS: i64 = 86_400_000;

/// What a smart playlist holds: the songs that meet its rules, in its order, as many as its limit lets in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    /// A song needs to meet any one rule (else every one).
    #[serde(default)]
    pub any: bool,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub order: Order,
    /// At most this many songs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One rule: a field of the song, how it is compared, and with what.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub field: Field,
    pub op: Op,
    /// Text for the title, artist and album; "lossy", "lossless" or "hires" for the quality; nothing for liked and
    /// on this computer; else a whole number (a year, seconds, plays, days).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub value: Value,
    /// For `played_often`: in the last this many days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Title,
    Artist,
    Album,
    /// The album's year.
    Year,
    /// In seconds.
    Length,
    /// Times played, ever.
    Plays,
    /// Times played in the last `days` days.
    PlayedOften,
    LastPlayed,
    Added,
    Liked,
    /// Known from the file: an add-on's song has none until it is saved.
    Quality,
    /// A file on this computer (saved songs too).
    Here,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Contains,
    NotContains,
    Is,
    IsNot,
    StartsWith,
    AtLeast,
    Below,
    Within,
    NotWithin,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// The newest first.
    #[default]
    Added,
    MostPlayed,
    RecentlyPlayed,
    /// Never played first, then the longest since.
    LeastRecentlyPlayed,
    Title,
    Artist,
    Album,
    /// The newest year first.
    Year,
    /// A new order each day.
    Random,
}

impl Rules {
    /// The rules in `value`, checked; why not, in words, when they cannot be used.
    pub fn read(value: &Value) -> std::result::Result<Rules, String> {
        let rules: Rules = serde_json::from_value(value.clone())
            .map_err(|e| format!("these rules are not understood ({e})"))?;
        if rules.rules.len() > MAX_RULES {
            return Err(format!("a smart playlist has at most {MAX_RULES} rules"));
        }
        if rules.limit == Some(0) {
            return Err("a limit of no songs".to_string());
        }
        let mut values = Vec::new();
        for rule in &rules.rules {
            rule.condition(0, &mut values)?;
        }
        Ok(rules)
    }

    /// The rules as kept with the playlist.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("rules are plain data")
    }

    /// The WHERE and ORDER BY of the songs these rules pick at `now_ms`, with the values they bind.
    fn sql(&self, now_ms: i64) -> (String, String, Vec<Sql>) {
        let mut values = Vec::new();
        let conditions: Vec<String> = self
            .rules
            .iter()
            .filter_map(|rule| rule.condition(now_ms, &mut values).ok())
            .map(|c| format!("({c})"))
            .collect();
        let filter = if conditions.is_empty() {
            "1".to_string()
        } else {
            conditions.join(if self.any { " OR " } else { " AND " })
        };
        (filter, self.order.sql(now_ms), values)
    }
}

impl Field {
    /// How a field can be compared.
    pub fn ops(self) -> &'static [Op] {
        use Op::*;
        match self {
            Field::Title | Field::Artist | Field::Album => {
                &[Contains, NotContains, Is, IsNot, StartsWith]
            }
            Field::Year => &[Is, IsNot, AtLeast, Below],
            Field::Length => &[AtLeast, Below],
            Field::Plays => &[Is, AtLeast, Below],
            Field::PlayedOften => &[AtLeast],
            Field::LastPlayed | Field::Added => &[Within, NotWithin],
            Field::Liked | Field::Here => &[Is, IsNot],
            Field::Quality => &[Is, AtLeast],
        }
    }
}

impl Rule {
    /// The rule as an SQL condition over `t` (tracks), `a` (artists) and `al` (albums), its values pushed to
    /// `values`; why not, in words, when the rule makes no sense.
    fn condition(&self, now_ms: i64, values: &mut Vec<Sql>) -> std::result::Result<String, String> {
        use Field::*;
        use Op::*;
        let mut bind = |value: Sql| {
            values.push(value);
            format!("?{}", values.len())
        };
        let field = self.field;
        let wrong = || {
            let name = serde_json::to_value(field).unwrap_or_default();
            let name = name.as_str().unwrap_or_default().replace('_', " ");
            format!("{name} cannot be compared that way")
        };
        if !field.ops().contains(&self.op) {
            return Err(wrong());
        }
        Ok(match field {
            Title | Artist | Album => {
                let text = self.text()?;
                let p = bind(Sql::Text(plain(text)));
                let column = match field {
                    Title => "t.title",
                    Artist => "a.name",
                    _ => "al.title",
                };
                let x = format!("stash_plain({column})");
                match self.op {
                    Contains => format!("instr({x}, {p}) > 0"),
                    NotContains => format!("IFNULL(instr({x}, {p}), 0) = 0"),
                    Is => format!("{x} = {p}"),
                    IsNot => format!("IFNULL({x}, '') <> {p}"),
                    StartsWith => format!("substr({x}, 1, length({p})) = {p}"),
                    _ => return Err(wrong()),
                }
            }
            Year => {
                let p = bind(Sql::Integer(self.number(0, 9999)?));
                match self.op {
                    Is => format!("al.year = {p}"),
                    IsNot => format!("IFNULL(al.year, -1) <> {p}"),
                    AtLeast => format!("al.year >= {p}"),
                    Below => format!("al.year < {p}"),
                    _ => return Err(wrong()),
                }
            }
            Length => {
                let p = bind(Sql::Integer(self.number(0, 86_400)? * 1000));
                match self.op {
                    AtLeast => format!("t.duration_ms >= {p}"),
                    Below => format!("t.duration_ms > 0 AND t.duration_ms < {p}"),
                    _ => return Err(wrong()),
                }
            }
            Plays => {
                let p = bind(Sql::Integer(self.number(0, 10_000_000)?));
                match self.op {
                    Is => format!("t.play_count = {p}"),
                    AtLeast => format!("t.play_count >= {p}"),
                    Below => format!("t.play_count < {p}"),
                    _ => return Err(wrong()),
                }
            }
            PlayedOften => {
                if self.op != AtLeast {
                    return Err(wrong());
                }
                let times = self.number(1, 10_000_000)?;
                let days = self
                    .days
                    .filter(|d| (1..=36_500).contains(d))
                    .ok_or("played often: in how many days?")?;
                let since = bind(Sql::Integer(now_ms - days * DAY_MS));
                let times = bind(Sql::Integer(times));
                format!(
                    "(SELECT COUNT(*) FROM plays pl WHERE pl.track_id = t.id AND pl.played_at_ms >= {since}) >= {times}"
                )
            }
            // Kept in seconds.
            LastPlayed | Added => {
                let days = self.number(1, 36_500)?;
                let since = bind(Sql::Integer((now_ms - days * DAY_MS) / 1000));
                let column = if field == LastPlayed {
                    "t.last_played_at"
                } else {
                    "t.added_at"
                };
                match self.op {
                    Within => format!("{column} >= {since}"),
                    // Never played is not played within.
                    NotWithin => format!("({column} IS NULL OR {column} < {since})"),
                    _ => return Err(wrong()),
                }
            }
            Liked => match self.op {
                Is => "t.liked_at_ms IS NOT NULL".to_string(),
                IsNot => "t.liked_at_ms IS NULL".to_string(),
                _ => return Err(wrong()),
            },
            Quality => {
                let level = match self.value.as_str() {
                    Some("lossy") => 0,
                    Some("lossless") => 1,
                    Some("hires") => 2,
                    _ => return Err("quality: lossy, lossless or hires".to_string()),
                };
                match self.op {
                    Is => format!("({QUALITY}) = {level}"),
                    AtLeast => format!("({QUALITY}) >= {level}"),
                    _ => return Err(wrong()),
                }
            }
            Here => match self.op {
                Is => "t.origin = 'file'".to_string(),
                IsNot => "t.origin <> 'file'".to_string(),
                _ => return Err(wrong()),
            },
        })
    }

    fn text(&self) -> std::result::Result<&str, String> {
        let text = self.value.as_str().map(str::trim).unwrap_or_default();
        if text.is_empty() {
            return Err("a rule is missing its words".to_string());
        }
        if text.chars().count() > MAX_TEXT {
            return Err(format!("a rule's words are at most {MAX_TEXT} characters"));
        }
        Ok(text)
    }

    fn number(&self, least: i64, most: i64) -> std::result::Result<i64, String> {
        self.value
            .as_i64()
            .filter(|n| (least..=most).contains(n))
            .ok_or_else(|| format!("a rule needs a whole number from {least} to {most}"))
    }
}

/// A song's quality from its file: 2 hi-res (a lossless codec over 16 bits or 48 kHz), 1 lossless, 0 lossy, NULL not
/// known. As the window's quality badges count it.
const QUALITY: &str = "CASE
    WHEN t.codec IS NULL OR t.codec = '' THEN NULL
    WHEN t.codec IN ('flac', 'alac', 'ape', 'wavpack') OR t.codec LIKE 'pcm%' THEN
        CASE WHEN IFNULL(t.bit_depth, 0) > 16 OR IFNULL(t.sample_rate, 0) > 48000 THEN 2 ELSE 1 END
    ELSE 0 END";

impl Order {
    fn sql(self, now_ms: i64) -> String {
        match self {
            Order::Added => "t.added_at DESC, t.id DESC".to_string(),
            Order::MostPlayed => "t.play_count DESC, t.last_played_at DESC, t.id".to_string(),
            Order::RecentlyPlayed => {
                "t.last_played_at IS NULL, t.last_played_at DESC, t.id".to_string()
            }
            Order::LeastRecentlyPlayed => {
                "t.last_played_at IS NOT NULL, t.last_played_at, t.id".to_string()
            }
            Order::Title => "t.sort_key, t.id".to_string(),
            Order::Artist => "a.sort_key, al.sort_key, t.disc_no, t.track_no, t.id".to_string(),
            Order::Album => {
                "al.sort_key IS NULL, al.sort_key, t.disc_no, t.track_no, t.id".to_string()
            }
            Order::Year => {
                "al.year IS NULL, al.year DESC, al.sort_key, t.disc_no, t.track_no, t.id"
                    .to_string()
            }
            // The same all day (UTC), another the next: a page read later in the day finds the songs where they were.
            Order::Random => {
                let day = now_ms.div_euclid(DAY_MS);
                let seed = (day.wrapping_mul(2_654_435_761)).rem_euclid(4_294_967_311);
                format!("(t.id * 2654435761 + {seed}) % 4294967311, t.id")
            }
        }
    }
}

/// Text as rules compare it: no case, no accents, spaces as one.
pub fn plain(text: &str) -> String {
    let folded: String = text
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The time now, in milliseconds.
pub(crate) fn clock_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Library {
    /// The rules of the playlist `id`, when it is a smart one.
    pub fn smart_rules(&self, id: i64) -> Result<Option<Rules>> {
        let kept: Option<String> = self
            .conn
            .prepare_cached("SELECT rules FROM playlists WHERE id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(kept.and_then(|json| {
            serde_json::from_str::<Value>(&json)
                .ok()
                .and_then(|v| Rules::read(&v).ok())
        }))
    }

    /// Makes a smart playlist, last in the list.
    pub fn create_smart_playlist(&mut self, name: &str, rules: &Rules, now_ms: i64) -> Result<i64> {
        let id = self.create_playlist(name, now_ms)?;
        self.conn.execute(
            "UPDATE playlists SET rules = ?2 WHERE id = ?1",
            rusqlite::params![id, rules.to_json()],
        )?;
        Ok(id)
    }

    /// Gives the smart playlist `id` new rules. False when there is no such smart playlist.
    pub fn set_smart_rules(&mut self, id: i64, rules: &Rules, now_ms: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE playlists SET rules = ?2, updated_at_ms = ?3 WHERE id = ?1 AND rules IS NOT NULL",
            rusqlite::params![id, rules.to_json(), now_ms],
        )? > 0)
    }

    /// How many songs `rules` pick.
    pub fn smart_count(&self, rules: &Rules, now_ms: i64) -> Result<u64> {
        let (filter, _, values) = rules.sql(now_ms);
        let cap = rules.limit.map_or(-1, i64::from);
        let sql = format!(
            "SELECT COUNT(*) FROM (SELECT t.id FROM tracks t {TRACK_JOINS_FOR_LISTS} WHERE {filter} LIMIT {cap})"
        );
        let n: i64 = self
            .conn
            .prepare_cached(&sql)?
            .query_row(params_from_iter(values), |r| r.get(0))?;
        Ok(n as u64)
    }

    /// The page of songs `rules` pick at `offset`.
    pub fn smart_tracks_at(
        &self,
        rules: &Rules,
        offset: usize,
        limit: usize,
        now_ms: i64,
    ) -> Result<Vec<TrackRow>> {
        let Some(take) = page(rules, offset, limit) else {
            return Ok(Vec::new());
        };
        let (filter, order, values) = rules.sql(now_ms);
        // The page's songs are picked first, by the rules and the order alone; their columns (which look more up for
        // each song: whether it is here, whether it is chosen not to be played) are read for the page only. The order
        // ends on the song's id, so the page comes out as it was picked.
        let sql = format!(
            "SELECT {TRACK_COLUMNS_FOR_LISTS}
             FROM (SELECT t.id FROM tracks t {TRACK_JOINS_FOR_LISTS}
                   WHERE {filter} ORDER BY {order} LIMIT {take} OFFSET {offset}) p
             JOIN tracks t ON t.id = p.id {TRACK_JOINS_FOR_LISTS}
             ORDER BY {order}"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(values), track_row_for_lists)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The ids of the songs `rules` pick, from `offset` on.
    pub fn smart_track_ids(
        &self,
        rules: &Rules,
        offset: usize,
        limit: usize,
        now_ms: i64,
    ) -> Result<Vec<i64>> {
        let Some(take) = page(rules, offset, limit) else {
            return Ok(Vec::new());
        };
        let (filter, order, values) = rules.sql(now_ms);
        let sql = format!(
            "SELECT t.id FROM tracks t {TRACK_JOINS_FOR_LISTS}
             WHERE {filter} ORDER BY {order} LIMIT {take} OFFSET {offset}"
        );
        let ids = self
            .conn
            .prepare_cached(&sql)?
            .query_map(params_from_iter(values), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }
}

/// How many songs of the page at `offset` the limit lets in; `None` past it.
fn page(rules: &Rules, offset: usize, limit: usize) -> Option<usize> {
    let cap = rules.limit.map_or(usize::MAX, |n| n as usize);
    (offset < cap && limit > 0).then(|| limit.min(cap - offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rules_are_read_and_checked() {
        let good = json!({ "rules": [
            { "field": "artist", "op": "contains", "value": "Lanterns" },
            { "field": "played_often", "op": "at_least", "value": 3, "days": 30 },
            { "field": "quality", "op": "at_least", "value": "lossless" },
            { "field": "liked", "op": "is" }
        ], "order": "most_played", "limit": 25 });
        let rules = Rules::read(&good).unwrap();
        assert_eq!(rules.order, Order::MostPlayed);
        assert_eq!(
            Rules::read(&serde_json::from_str(&rules.to_json()).unwrap()),
            Ok(rules)
        );

        for (bad, why) in [
            (
                json!({ "rules": [{ "field": "artist", "op": "at_least", "value": 3 }] }),
                "cannot be compared",
            ),
            (
                json!({ "rules": [{ "field": "title", "op": "contains", "value": "  " }] }),
                "missing its words",
            ),
            (
                json!({ "rules": [{ "field": "year", "op": "is", "value": "ninety" }] }),
                "whole number",
            ),
            (
                json!({ "rules": [{ "field": "played_often", "op": "at_least", "value": 3 }] }),
                "how many days",
            ),
            (
                json!({ "rules": [{ "field": "quality", "op": "is", "value": "great" }] }),
                "lossy, lossless or hires",
            ),
            (
                json!({ "rules": [{ "field": "mood", "op": "is" }] }),
                "not understood",
            ),
            (json!({ "limit": 0 }), "no songs"),
        ] {
            let said = Rules::read(&bad).unwrap_err();
            assert!(said.contains(why), "{bad} → {said}");
        }
    }

    #[test]
    fn text_is_compared_without_case_or_accents() {
        assert_eq!(plain("  Beyoncé   KNOWLES "), "beyonce knowles");
        assert_eq!(plain("Sigur Rós"), plain("SIGUR ROS"));
    }

    #[test]
    fn a_limit_ends_the_pages() {
        let rules = Rules {
            any: false,
            rules: vec![],
            order: Order::Added,
            limit: Some(30),
        };
        assert_eq!(page(&rules, 0, 25), Some(25));
        assert_eq!(page(&rules, 25, 25), Some(5));
        assert_eq!(page(&rules, 30, 25), None);
    }
}
