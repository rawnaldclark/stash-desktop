//! Scrobbling: each play that counts goes to the services that keep the listener's listening history (ListenBrainz,
//! or a server that speaks its language, and Last.fm). A play waits in the library until the service has taken it, so
//! plays made with no connection go out when there is one.
//!
//! This keeps where plays go and what waits for each; `listenbrainz.rs` and `lastfm.rs` speak to the services; the
//! core's server sends. A service's token (Last.fm's session) is kept here as an add-on's link is, and is never given
//! to the window.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use crate::Result;
use crate::library::Library;

/// The most plays that wait for one service: past this, the oldest make way.
pub const MOST_WAITING: i64 = 50_000;

/// A service the listener's plays go to, as the window sees it: never its token.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Target {
    pub id: i64,
    /// How it is spoken to ("listenbrainz", "lastfm").
    pub kind: String,
    /// Its address.
    pub url: String,
    /// The name it knows the listener by.
    pub user: Option<String>,
    pub enabled: bool,
    /// It refused the token: nothing is sent to it until the token is given again. Plays still wait.
    pub refused: bool,
    pub added_at_ms: i64,
    /// Plays it took, all told, and when it last took one.
    pub sent: i64,
    pub sent_at_ms: Option<i64>,
    /// Plays it said were wrong, let go.
    pub dropped: i64,
    /// Plays waiting for it.
    pub waiting: i64,
    /// Why the last try failed, in words, and when. Cleared when it takes a play.
    pub trouble: Option<Trouble>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trouble {
    pub message: String,
    pub at_ms: i64,
}

/// A play waiting to be sent: its row, when the listening began (seconds), and the song as the service takes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Waiting {
    pub id: i64,
    pub listened_at: i64,
    pub song: Value,
}

/// What the library knows of a song that a service can use, besides its title, artist and album.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub duration_ms: Option<i64>,
    pub isrc: Option<String>,
    pub track_no: Option<i64>,
}

impl Library {
    /// The services plays go to, in the order they were added.
    pub fn scrobble_targets(&self) -> Result<Vec<Target>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.id, t.kind, t.url, t.user, t.enabled, t.refused, t.added_at_ms, t.sent, t.sent_at_ms, t.dropped,
                    (SELECT COUNT(*) FROM scrobbles s WHERE s.target = t.id), t.trouble, t.trouble_at_ms
             FROM scrobble_targets t ORDER BY t.added_at_ms, t.id",
        )?;
        let rows = stmt.query_map([], |r| {
            let trouble = match r.get::<_, Option<String>>(11)? {
                Some(message) => Some(Trouble {
                    message,
                    at_ms: r.get::<_, Option<i64>>(12)?.unwrap_or(0),
                }),
                None => None,
            };
            Ok(Target {
                id: r.get(0)?,
                kind: r.get(1)?,
                url: r.get(2)?,
                user: r.get(3)?,
                enabled: r.get::<_, i64>(4)? != 0,
                refused: r.get::<_, i64>(5)? != 0,
                added_at_ms: r.get(6)?,
                sent: r.get(7)?,
                sent_at_ms: r.get(8)?,
                dropped: r.get(9)?,
                waiting: r.get(10)?,
                trouble,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds a service, or gives the one already at that address its new token and name: it is used again (on, and
    /// no longer refused). Plays already waiting for it stay.
    pub fn add_scrobble_target(
        &mut self,
        kind: &str,
        url: &str,
        token: &str,
        user: Option<&str>,
        now_ms: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO scrobble_targets(kind, url, token, user, added_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(kind, url) DO UPDATE SET token = excluded.token, user = excluded.user, enabled = 1,
                 refused = 0, trouble = NULL, trouble_at_ms = NULL",
            params![kind, url, token, user, now_ms],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM scrobble_targets WHERE kind = ?1 AND url = ?2",
            params![kind, url],
            |r| r.get(0),
        )?)
    }

    /// Removes a service and the plays waiting for it. False when there is no such service.
    pub fn remove_scrobble_target(&mut self, id: i64) -> Result<bool> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM scrobbles WHERE target = ?1", [id])?;
        let gone = tx.execute("DELETE FROM scrobble_targets WHERE id = ?1", [id])? > 0;
        tx.commit()?;
        Ok(gone)
    }

    /// Turns a service on or off. While it is off, plays are not kept for it. False when there is no such service.
    pub fn set_scrobble_target_enabled(&mut self, id: i64, enabled: bool) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE scrobble_targets SET enabled = ?2 WHERE id = ?1",
            params![id, enabled as i64],
        )? > 0)
    }

    /// How to reach a service: its kind, its address and the listener's token.
    pub fn scrobble_token(&self, id: i64) -> Result<Option<(String, String, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT kind, url, token FROM scrobble_targets WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }

    /// What the library knows of song `id` that a service can use.
    pub fn scrobble_facts(&self, id: i64) -> Result<Facts> {
        Ok(self
            .conn
            .query_row(
                "SELECT duration_ms, isrc, track_no FROM tracks WHERE id = ?1",
                [id],
                |r| {
                    Ok(Facts {
                        duration_ms: r.get(0)?,
                        isrc: r.get(1)?,
                        track_no: r.get(2)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    /// A play that counted, begun at `listened_at` (seconds): it waits for every service that is on. The number of
    /// services it waits for (the same play is not kept twice).
    pub fn queue_scrobble(&mut self, listened_at: i64, song: &Value) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let added = tx.execute(
            "INSERT OR IGNORE INTO scrobbles(target, listened_at, song)
             SELECT id, ?1, ?2 FROM scrobble_targets WHERE enabled = 1",
            params![listened_at, song.to_string()],
        )?;
        if added > 0 {
            // Past the most that may wait for a service, its oldest plays make way.
            let over: Vec<(i64, i64)> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT target, COUNT(*) - ?1 FROM scrobbles GROUP BY target HAVING COUNT(*) > ?1",
                )?;
                stmt.query_map([MOST_WAITING], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for (target, extra) in over {
                tx.execute(
                    "DELETE FROM scrobbles WHERE id IN
                         (SELECT id FROM scrobbles WHERE target = ?1 ORDER BY listened_at, id LIMIT ?2)",
                    params![target, extra],
                )?;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// The plays waiting for service `target`, oldest first, at most `most`.
    pub fn waiting_scrobbles(&self, target: i64, most: usize) -> Result<Vec<Waiting>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, listened_at, song FROM scrobbles WHERE target = ?1 ORDER BY listened_at, id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![target, most as i64], |r| {
            Ok(Waiting {
                id: r.get(0)?,
                listened_at: r.get(1)?,
                // A song that cannot be read is sent as nothing, and the sender lets it go.
                song: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or(Value::Null),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Service `target` took these plays: they wait no more, and its trouble is over.
    pub fn scrobbles_sent(&mut self, target: i64, ids: &[i64], now_ms: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut taken = 0i64;
        for id in ids {
            taken += tx.execute(
                "DELETE FROM scrobbles WHERE id = ?1 AND target = ?2",
                params![id, target],
            )? as i64;
        }
        tx.execute(
            "UPDATE scrobble_targets SET sent = sent + ?2, sent_at_ms = ?3, trouble = NULL, trouble_at_ms = NULL
             WHERE id = ?1",
            params![target, taken, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Service `target` said these plays are wrong and will never take them: they wait no more.
    pub fn scrobbles_dropped(&mut self, target: i64, ids: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut gone = 0i64;
        for id in ids {
            gone += tx.execute(
                "DELETE FROM scrobbles WHERE id = ?1 AND target = ?2",
                params![id, target],
            )? as i64;
        }
        tx.execute(
            "UPDATE scrobble_targets SET dropped = dropped + ?2 WHERE id = ?1",
            params![target, gone],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The last try to reach service `target` failed: why, in words. `refused`: it refused that token, and nothing more
    /// is sent to it until the token is given again. A token given again meanwhile is not the one refused: then
    /// nothing is noted.
    pub fn scrobble_trouble(
        &mut self,
        target: i64,
        message: &str,
        refused: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        match refused {
            None => self.conn.execute(
                "UPDATE scrobble_targets SET trouble = ?2, trouble_at_ms = ?3 WHERE id = ?1",
                params![target, message, now_ms],
            )?,
            Some(token) => self.conn.execute(
                "UPDATE scrobble_targets SET trouble = ?2, trouble_at_ms = ?3, refused = 1 WHERE id = ?1 AND token = ?4",
                params![target, message, now_ms, token],
            )?,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn song(title: &str) -> Value {
        json!({ "artist_name": "The Lanterns", "track_name": title })
    }

    #[test]
    fn a_play_waits_for_each_service_that_is_on_until_it_is_taken() {
        let mut library = Library::open_in_memory().unwrap();
        // With no service, a play is not kept.
        assert_eq!(
            library
                .queue_scrobble(1_700_000_000, &song("Paper Boats"))
                .unwrap(),
            0
        );
        let lb = library
            .add_scrobble_target(
                "listenbrainz",
                "https://api.listenbrainz.org",
                "token-a",
                Some("listener"),
                10,
            )
            .unwrap();
        let mine = library
            .add_scrobble_target(
                "listenbrainz",
                "http://192.168.1.20:42010/apis/listenbrainz",
                "token-b",
                None,
                20,
            )
            .unwrap();
        assert_eq!(
            library
                .queue_scrobble(1_700_000_000, &song("Paper Boats"))
                .unwrap(),
            2
        );
        // The same play again is the same play.
        assert_eq!(
            library
                .queue_scrobble(1_700_000_000, &song("Paper Boats"))
                .unwrap(),
            0
        );
        library.set_scrobble_target_enabled(mine, false).unwrap();
        assert_eq!(
            library
                .queue_scrobble(1_700_000_300, &song("Afterglow"))
                .unwrap(),
            1
        );

        let targets = library.scrobble_targets().unwrap();
        assert_eq!(
            targets
                .iter()
                .map(|t| (t.id, t.waiting, t.enabled))
                .collect::<Vec<_>>(),
            vec![(lb, 2, true), (mine, 1, false)]
        );
        assert_eq!(targets[0].user.as_deref(), Some("listener"));
        // The window is never given a token.
        assert!(!serde_json::to_string(&targets).unwrap().contains("token-"));
        assert_eq!(
            library.scrobble_token(lb).unwrap(),
            Some((
                "listenbrainz".into(),
                "https://api.listenbrainz.org".into(),
                "token-a".into()
            ))
        );

        // Oldest first; taken ones wait no more and end the trouble.
        let waiting = library.waiting_scrobbles(lb, 10).unwrap();
        assert_eq!(
            waiting
                .iter()
                .map(|w| (w.listened_at, w.song["track_name"].clone()))
                .collect::<Vec<_>>(),
            vec![
                (1_700_000_000, json!("Paper Boats")),
                (1_700_000_300, json!("Afterglow")),
            ]
        );
        library
            .scrobble_trouble(lb, "it could not be reached", None, 500)
            .unwrap();
        assert_eq!(
            library.scrobble_targets().unwrap()[0].trouble,
            Some(Trouble {
                message: "it could not be reached".into(),
                at_ms: 500
            })
        );
        library.scrobbles_sent(lb, &[waiting[0].id], 600).unwrap();
        library.scrobbles_dropped(lb, &[waiting[1].id]).unwrap();
        let after = &library.scrobble_targets().unwrap()[0];
        assert_eq!(
            (
                after.waiting,
                after.sent,
                after.sent_at_ms,
                after.dropped,
                after.trouble.clone()
            ),
            (0, 1, Some(600), 1, None)
        );

        // A refused token holds sending back until it is given again; giving it again keeps what waits. A token that
        // is no longer the service's (given again while the old one was being refused) changes nothing.
        library
            .scrobble_trouble(mine, "the token was refused", Some("an older token"), 690)
            .unwrap();
        assert!(!library.scrobble_targets().unwrap()[1].refused);
        library
            .scrobble_trouble(mine, "the token was refused", Some("token-b"), 700)
            .unwrap();
        assert!(library.scrobble_targets().unwrap()[1].refused);
        let again = library
            .add_scrobble_target(
                "listenbrainz",
                "http://192.168.1.20:42010/apis/listenbrainz",
                "token-c",
                Some("me"),
                800,
            )
            .unwrap();
        assert_eq!(again, mine);
        let renewed = &library.scrobble_targets().unwrap()[1];
        assert_eq!(
            (
                renewed.refused,
                renewed.enabled,
                renewed.waiting,
                renewed.trouble.clone()
            ),
            (false, true, 1, None)
        );
        assert_eq!(library.scrobble_token(mine).unwrap().unwrap().2, "token-c");

        // Removed, with what waited for it.
        assert!(library.remove_scrobble_target(mine).unwrap());
        assert!(!library.remove_scrobble_target(mine).unwrap());
        assert_eq!(library.scrobble_targets().unwrap().len(), 1);
        assert!(library.waiting_scrobbles(mine, 10).unwrap().is_empty());
    }

    #[test]
    fn past_the_most_that_may_wait_the_oldest_make_way() {
        let mut library = Library::open_in_memory().unwrap();
        let lb = library
            .add_scrobble_target("listenbrainz", "https://api.listenbrainz.org", "t", None, 0)
            .unwrap();
        {
            let tx = library.conn.transaction().unwrap();
            tx.execute(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?2)
                 INSERT INTO scrobbles(target, listened_at, song) SELECT ?1, 1000 + i, 'x' || i FROM n",
                params![lb, MOST_WAITING],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        library.queue_scrobble(9_999_999, &song("Newest")).unwrap();
        let left = library.scrobble_targets().unwrap()[0].waiting;
        assert_eq!(left, MOST_WAITING);
        let oldest = library.waiting_scrobbles(lb, 1).unwrap();
        assert_eq!(oldest[0].listened_at, 1002, "the oldest play made way");
    }
}
