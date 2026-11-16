//! What the listener's add-ons were asked for: each stream and each download Stash got from one, with its time, so
//! the window can say how much of an add-on's allowance Stash has used today (the day is the window's: the core has
//! no time zone). Kept for a week.

use std::collections::HashMap;

use rusqlite::params;

use crate::{Library, Result};

/// How long what an add-on was asked for is kept.
const KEPT_MS: i64 = 7 * 24 * 3600 * 1000;

/// What an add-on gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Given {
    Stream,
    Download,
}

impl Given {
    fn as_str(self) -> &'static str {
        match self {
            Given::Stream => "stream",
            Given::Download => "download",
        }
    }
}

/// How many streams and downloads an add-on gave.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Use {
    pub streams: u64,
    pub downloads: u64,
}

impl Library {
    /// Notes that the add-on `addon` gave a stream or a download at `at_ms` (and forgets what is a week old).
    pub fn note_addon_use(&mut self, addon: &str, given: Given, at_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO addon_events(addon, kind, at_ms) VALUES (?1, ?2, ?3)",
            params![addon, given.as_str(), at_ms],
        )?;
        self.conn.execute(
            "DELETE FROM addon_events WHERE at_ms < ?1",
            [at_ms - KEPT_MS],
        )?;
        Ok(())
    }

    /// What each add-on gave since `since_ms`.
    pub fn addon_use_since(&self, since_ms: i64) -> Result<HashMap<String, Use>> {
        let mut used: HashMap<String, Use> = HashMap::new();
        let mut stmt = self.conn.prepare_cached(
            "SELECT addon, kind, COUNT(*) FROM addon_events WHERE at_ms >= ?1 GROUP BY addon, kind",
        )?;
        let rows = stmt.query_map([since_ms], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? as u64,
            ))
        })?;
        for row in rows {
            let (addon, kind, n) = row?;
            let entry = used.entry(addon).or_default();
            match kind.as_str() {
                "stream" => entry.streams += n,
                "download" => entry.downloads += n,
                _ => {}
            }
        }
        Ok(used)
    }
}
