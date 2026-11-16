//! Autoplay: when the last song of the queue starts (and nothing repeats), songs like the last ones are added after
//! it, so the music goes on. With similar music from Last.fm (see [`crate::radio`]), a station starts from the newest
//! of the last songs Last.fm can be asked about: songs by similar artists, the listener's own first, then their
//! add-ons'. Without it, or when Last.fm cannot help, they come from the listener's own library: songs by the artists
//! just heard, then liked songs, then what is played most; none heard in the last few hours, none already queued,
//! never the same artist twice in a row when that can be helped. On unless the listener switches it off
//! (Settings › Sound), as on the phone.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use stash_core::Library;
use stash_core::player::Player;

use crate::radio::{Kind, seed_askable, song_seed};
use crate::serve::{State, can_stream, now_ms};

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

/// Looks at the player (about once a second) and adds songs when the last one has started, or when nothing comes
/// after the one playing (the songs after it could not be played). It acts once a song (`Radio::autoplay_after`),
/// and not while a station still grows the queue.
pub(crate) fn look(state: &State, player: &Player) {
    let now = player.state();
    let Some(index) = now.index else { return };
    let Some(current) = now.track.as_ref() else {
        return;
    };
    let radio = &state.radio;
    if now.status != "playing"
        || now.repeat != "off"
        || (index + 1 != now.queue_len && !now.nothing_after)
        || radio.autoplay_after.load(Ordering::Relaxed) == current.qid
        || radio.carries_on(now.queue_id)
    {
        return;
    }
    // Switched off: looked at again, in case it is switched on before the song ends.
    if !on(&state.library.lock().unwrap()) {
        return;
    }
    // The last songs say what comes next; the songs around them are not picked again (a queue of the whole library
    // is not read whole for this).
    let queue = player.queue_at(now.queue_len.saturating_sub(500), 500);
    let Some(last) = queue.last().map(|t| t.qid) else {
        return;
    };
    let recent: Vec<_> = queue.iter().rev().take(SEEDS).collect();
    let seed_ids: Vec<i64> = recent.iter().filter_map(|t| t.id).collect();
    let seed_artists: Vec<String> = recent.iter().map(|t| t.artist.clone()).collect();
    radio.autoplay_after.store(current.qid, Ordering::Relaxed);

    // Similar music: a station from the newest of the last songs Last.fm can be asked about. It adds after the last
    // song while that is still the last.
    let seed = {
        let library = state.library.lock().unwrap();
        radio
            .on(&library)
            .then(|| {
                recent
                    .iter()
                    .map(|t| song_seed(&library, t))
                    .find(seed_askable)
            })
            .flatten()
    };
    if let Some(seed) = seed {
        radio.begin(
            Kind::Autoplay,
            seed,
            (seed_ids, seed_artists),
            now.queue_id,
            Some(last),
        );
        return;
    }

    let queued: HashSet<i64> = queue.iter().filter_map(|t| t.id).collect();
    let items = {
        let library = state.library.lock().unwrap();
        // The library could not be read: nothing this time.
        let Ok(picked) = library.autoplay_picks(
            &seed_ids,
            &seed_artists,
            &queued,
            now_ms() - RESTED_MS,
            !can_stream(state),
            BATCH,
        ) else {
            return;
        };
        let Ok(items) = library.queue_items(&picked) else {
            return;
        };
        items
    };
    if let Some(qids) = player.enqueue_onto(items, now.queue_id, Some(last)) {
        radio.note_fill(qids, now.queue_id);
    }
}
