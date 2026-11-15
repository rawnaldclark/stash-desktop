//! Autoplay: when the last song of the queue starts (and nothing repeats), songs like the last ones are added after
//! it, so the music goes on. They come from the listener's own library: songs by the artists just heard, then liked
//! songs, then what is played most; none heard in the last few hours, none already queued, never the same artist twice
//! in a row when that can be helped. On unless the listener switches it off (Settings › Sound), as on the phone.

use std::collections::HashSet;
use std::sync::Mutex;

use stash_core::Library;
use stash_core::player::Player;

/// The setting that switches it off ("0").
pub(crate) const SETTING: &str = "autoplay";
/// How many songs are added at a time.
const BATCH: usize = 10;
/// How many of the last songs say what to play next.
const SEEDS: usize = 5;
/// Songs heard this recently are not picked again.
const RESTED_MS: i64 = 3 * 3600 * 1000;

/// Whether autoplay is on.
pub(crate) fn on(library: &Library) -> bool {
    library.setting(SETTING).ok().flatten().as_deref() != Some("0")
}

/// Remembers which last song it already added after, so a queue is not filled twice.
#[derive(Default)]
pub(crate) struct Autoplay {
    filled_after: Option<u64>,
}

impl Autoplay {
    /// Looks at the player (about once a second) and adds songs when the last one has started.
    pub fn look(&mut self, library: &Mutex<Library>, player: &Player, can_ask_addons: bool) {
        let state = player.state();
        let Some(index) = state.index else { return };
        let Some(current) = state.track.as_ref() else {
            return;
        };
        if state.status != "playing"
            || state.repeat != "off"
            || index + 1 != state.queue_len
            || self.filled_after == Some(current.qid)
        {
            return;
        }
        self.filled_after = Some(current.qid);
        if !on(&library.lock().unwrap()) {
            return;
        }
        // The last songs say what comes next; the songs around them are not picked again (a queue of the whole
        // library is not read whole for this).
        let queue = player.queue_at(state.queue_len.saturating_sub(500), 500);
        let queued: HashSet<i64> = queue.iter().filter_map(|t| t.id).collect();
        let recent: Vec<_> = queue.iter().rev().take(SEEDS).collect();
        let seed_ids: Vec<i64> = recent.iter().filter_map(|t| t.id).collect();
        let seed_artists: Vec<String> = recent.iter().map(|t| t.artist.clone()).collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let items = {
            let library = library.lock().unwrap();
            let picked = library
                .autoplay_picks(
                    &seed_ids,
                    &seed_artists,
                    &queued,
                    now - RESTED_MS,
                    !can_ask_addons,
                    BATCH,
                )
                .unwrap_or_default();
            library.queue_items(&picked).unwrap_or_default()
        };
        player.enqueue(items, false);
    }
}
