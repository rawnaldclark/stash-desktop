//! Stash's mixes (`stash_core::discover`): Daily Discover and Recommended by Last.fm, made once a day while YouTube Music
//! is on and Stash is online, and their routes.
//!
//! A mix is made in four steps: its seeds (what was played lately, filled in from the Last.fm account or the longer
//! run), Last.fm's answers about them (asked as the radio asks, through the same proxy, kept answers and day's count:
//! `Radio::look_up`), each new song looked for on YouTube Music, and the day's 40 drawn from everything the mix found.
//!
//! - `GET /api/discover`: the mixes on offer, each with its songs of the day.
//! - `GET /api/discover/<mix>`: a mix's songs of the day, as YouTube Music songs (`TubeSong` in the window).
//! - `POST /api/discover/<mix>/refresh`: another draw now; new finds are looked for when the last look is an hour old.
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Datelike;
use serde_json::{Value, json};
use stash_core::discover::{self, DAILY, Find, FoundSong, LASTFM};
use stash_core::identity::{fold, words_key};
use stash_core::lastfm::{self, Keys};
use stash_core::similar::{self, Answer, Ask};
use stash_core::youtube::parse::Kind;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, now_ms, offline, refuse, reply_json};

/// The first look after Stash starts, and how often it looks whether a mix is due.
const FIRST_LOOK: Duration = Duration::from_secs(90);
const LOOK_EVERY: Duration = Duration::from_secs(30 * 60);
/// Daily Discover's similar artists per seed and Recommended by Last.fm's similar songs per seed (Android's).
const SIMILAR_ARTISTS: usize = 5;
const SIMILAR_SONGS: usize = 10;
/// The most new songs looked for on YouTube Music in one making, and the wait between two searches.
const SEARCHES: usize = 60;
const BETWEEN: Duration = Duration::from_millis(700);
/// New finds are looked for again on Refresh only when the last look is this old.
const LOOK_AGAIN_ON_REFRESH_MS: i64 = 3_600_000;
const LOOKED_SETTING: &str = "discover.looked.";

/// Today, on this computer's calendar: a mix is new each local day.
fn today() -> i64 {
    i64::from(chrono::Local::now().date_naive().num_days_from_ce())
}

/// The Last.fm account the listener connected for scrobbling, and Stash's application keys: what Recommended by
/// Last.fm needs. None without either.
fn lastfm_user(state: &State) -> Option<(Keys, String)> {
    let keys = Keys::found()?;
    let targets = state.library.lock().unwrap().scrobble_targets().ok()?;
    let user = targets
        .into_iter()
        .find(|t| t.kind == "lastfm" && t.enabled && !t.refused)?
        .user?;
    (!user.trim().is_empty()).then_some((keys, user))
}

/// The mixes on offer: Daily Discover always (with YouTube Music and Last.fm in this build), Recommended by Last.fm with
/// a Last.fm account.
fn offered(state: &State) -> Vec<&'static str> {
    if state.youtube_on().is_none() || !state.radio.available() {
        return Vec::new();
    }
    let mut mixes = vec![DAILY];
    if lastfm_user(state).is_some() {
        mixes.insert(0, LASTFM);
    }
    mixes
}

/// What Last.fm answered, as (artist, title) for songs, or names for artists; nothing when it could not say.
fn named(state: &State, ask: &Ask) -> Vec<(String, Option<String>)> {
    match state.radio.look_up(state, ask, offline(state)) {
        Ok(Answer::Found(likes)) => likes.into_iter().map(|l| (l.artist, l.title)).collect(),
        _ => Vec::new(),
    }
}

/// A mix's candidates: (artist, title, seed), in the order they were found.
fn candidates(state: &State, mix: &str, stop: &AtomicBool) -> Vec<(String, String, String)> {
    let now = now_ms();
    let account = lastfm_user(state);
    let agent = stash_core::listenbrainz::agent();
    let mut out = Vec::new();
    if mix == DAILY {
        let (recent, longer, most) = {
            let library = state.library.lock().unwrap();
            let recent = library
                .artists_played_since(now - discover::LATELY_MS, discover::SEED_ARTISTS)
                .unwrap_or_default();
            let longer = library
                .artists_played_since(now - discover::LONG_RUN_MS, discover::SEED_ARTISTS)
                .unwrap_or_default();
            // A thin record is filled in with the artists the library has most songs by (Android's last tier).
            let most = library
                .artists_with_most_songs(discover::SEED_ARTISTS)
                .unwrap_or_default();
            (recent, longer, most)
        };
        let from_account: Vec<String> = account
            .as_ref()
            .and_then(|(keys, user)| {
                lastfm::user_top(&agent, keys, user, false, discover::SEED_ARTISTS).ok()
            })
            .map(|top| top.into_iter().map(|(artist, _)| artist).collect())
            .unwrap_or_default();
        let mut longer = if from_account.is_empty() {
            longer
        } else {
            from_account
        };
        longer.extend(most);
        for seed in discover::recent_first(&recent, &longer, discover::SEED_ARTISTS) {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let alike = named(
                state,
                &Ask::Artist {
                    artist: seed.clone(),
                },
            );
            for (artist, _) in alike.into_iter().take(SIMILAR_ARTISTS) {
                for (by, title) in named(
                    state,
                    &Ask::TopSongs {
                        artist: artist.clone(),
                    },
                ) {
                    if let Some(title) = title {
                        out.push((by, title, seed.clone()));
                    }
                }
            }
        }
    } else {
        let recent = state
            .library
            .lock()
            .unwrap()
            .songs_played_since(now - discover::LATELY_MS, discover::SEED_SONGS)
            .unwrap_or_default();
        let longer: Vec<(String, String)> = account
            .as_ref()
            .and_then(|(keys, user)| {
                lastfm::user_top(&agent, keys, user, true, discover::SEED_SONGS).ok()
            })
            .map(|top| top.into_iter().filter_map(|(a, t)| Some((a, t?))).collect())
            .unwrap_or_default();
        for (artist, title) in discover::recent_first(&recent, &longer, discover::SEED_SONGS)
            .into_iter()
            .take(discover::SONGS_ASKED)
        {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let seed = format!("{artist} - {title}");
            let ask = Ask::Song { artist, title };
            for (by, found) in named(state, &ask).into_iter().take(SIMILAR_SONGS) {
                if let Some(found) = found {
                    out.push((by, found, seed.clone()));
                }
            }
        }
    }
    out
}

/// The YouTube Music song Last.fm's (artist, title) is: the first search result by that artist with that title, the
/// released song before a video.
fn find_song(state: &State, artist: &str, title: &str) -> Option<FoundSong> {
    let youtube = state.youtube_on()?;
    let found = youtube.found(&format!("{artist} {title}")).ok()?;
    let want_artist = fold(&similar::clean_artist(artist));
    let want_title = fold(&similar::clean_title(title));
    if want_artist.is_empty() || want_title.is_empty() {
        return None;
    }
    let mut songs: Vec<stash_core::youtube::parse::Song> = Vec::new();
    if let Some(stash_core::youtube::pages::Top::Song(song)) = found.top {
        songs.push(song);
    }
    songs.extend(found.songs);
    songs.sort_by_key(|s| !matches!(s.kind, Some(Kind::Song) | None));
    let song = songs.into_iter().find(|s| {
        let by = fold(&s.artist);
        let named = fold(&similar::clean_title(&s.title));
        (by.contains(&want_artist) || want_artist.contains(&by) && !by.is_empty())
            && (named == want_title
                || named.starts_with(&want_title)
                || want_title.starts_with(&named) && !named.is_empty())
    })?;
    Some(FoundSong {
        id: song.id,
        title: song.title,
        artist: song.artist,
        album: song.album,
        duration_ms: song.duration_ms,
        art: song.art,
        explicit: song.explicit,
        artist_id: song.artist_id,
        album_id: song.album_id,
    })
}

/// Looks for `mix`'s new songs: Last.fm's candidates the library does not have, not looked for in 30 days, not another
/// mix's of today, found on YouTube Music. How many were found.
fn look(state: &State, mix: &str, stop: &AtomicBool) -> usize {
    let wanted = candidates(state, mix, stop);
    let since = now_ms() - discover::LOOK_AGAIN_MS;
    let (not_songs, not_artists) = state
        .library
        .lock()
        .unwrap()
        .not_for_mixes()
        .unwrap_or_default();
    let others: HashSet<String> = discover::MIXES
        .iter()
        .filter(|m| **m != mix)
        .flat_map(|m| {
            state
                .library
                .lock()
                .unwrap()
                .mix_songs(m)
                .unwrap_or_default()
        })
        .map(|f| words_key(&f.title, &f.artist))
        .collect();
    let mut asked: HashSet<String> = HashSet::new();
    let (mut searched, mut found) = (0, 0);
    for (artist, title, seed) in wanted {
        if searched >= SEARCHES || stop.load(Ordering::Relaxed) || offline(state) {
            break;
        }
        let key = words_key(&title, &artist);
        if !similar::lookable(&title, &artist)
            || not_songs.contains(&key)
            || not_artists.contains(&fold(&artist))
            || others.contains(&key)
            || !asked.insert(key)
            || state
                .library
                .lock()
                .unwrap()
                .looked_for(mix, &title, &artist, since)
                .unwrap_or(true)
        {
            continue;
        }
        if searched > 0 {
            std::thread::sleep(BETWEEN);
        }
        searched += 1;
        let song = find_song(state, &artist, &title);
        found += usize::from(song.is_some());
        let find = Find {
            title,
            artist,
            seed,
            found_at_ms: now_ms(),
            song,
        };
        let _ = state.library.lock().unwrap().note_find(mix, &find);
    }
    let _ = state
        .library
        .lock()
        .unwrap()
        .set_setting(&format!("{LOOKED_SETTING}{mix}"), &now_ms().to_string());
    found
}

/// Makes `mix` for today: new finds looked for, then the day's songs drawn. `seed` draws them (a day's own, or a
/// Refresh's).
fn make(state: &State, mix: &str, seed: u64, look_too: bool, stop: &AtomicBool) {
    let found = if look_too { look(state, mix, stop) } else { 0 };
    let drawn = state
        .library
        .lock()
        .unwrap()
        .draw_mix(mix, today(), seed, now_ms());
    if let Ok(n) = drawn {
        eprintln!("{}: {n} songs ({found} new finds)", discover::title(mix));
    }
    state.edited();
}

/// A day's own draw: the same all day (Android's day bucket), another tomorrow.
fn day_seed(mix: &str) -> u64 {
    let salt: u64 = mix.bytes().map(u64::from).sum();
    (today() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt
}

/// Makes each mix on offer that has not been made today, while YouTube Music is on and Stash is online.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let wait = |for_: Duration| {
        let until = Instant::now() + for_;
        while Instant::now() < until {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        true
    };
    if !wait(FIRST_LOOK) {
        return;
    }
    loop {
        if !offline(state) {
            for mix in offered(state) {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let made_today = state
                    .library
                    .lock()
                    .unwrap()
                    .mix_state(mix)
                    .ok()
                    .and_then(|s| s.day)
                    == Some(today());
                if !made_today {
                    make(state, mix, day_seed(mix), true, stop);
                }
            }
        }
        if !wait(LOOK_EVERY) {
            return;
        }
    }
}

/// A find as the window shows a YouTube Music song.
fn song_json(find: &Find) -> Option<Value> {
    let s = find.song.as_ref()?;
    Some(json!({
        "id": s.id,
        "title": s.title,
        "artist": s.artist,
        "album": s.album,
        "duration_ms": s.duration_ms,
        "art": s.art,
        "explicit": s.explicit,
        "artist_id": s.artist_id,
        "album_id": s.album_id,
    }))
}

/// A mix with its songs of the day (40 at most, so Home's cards have their covers too).
fn mix_json(state: &State, mix: &str) -> Result<Value> {
    let library = state.library.lock().unwrap();
    let songs = library.mix_songs(mix)?;
    let made = library.mix_state(mix)?;
    Ok(json!({
        "id": mix,
        "title": discover::title(mix),
        "description": discover::description(mix),
        "made_ms": made.drawn_ms,
        "songs": songs.iter().filter_map(song_json).collect::<Vec<_>>(),
    }))
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    let Some(rest) = path.strip_prefix("/api/discover") else {
        return Ok(None);
    };
    match (request.method(), rest) {
        (Method::Get, "") => {
            let mixes: Vec<Value> = offered(state)
                .into_iter()
                .filter_map(|m| mix_json(state, m).ok())
                .collect();
            Ok(Some(reply_json(200, json!({ "mixes": mixes }))))
        }
        (method, rest) => {
            let rest = rest.trim_start_matches('/');
            let (mix, refresh) = match rest.split_once('/') {
                Some((mix, "refresh")) => (mix, true),
                Some(_) => return Ok(Some(refuse(404, "No such mix"))),
                None => (rest, false),
            };
            let Some(mix) = discover::MIXES.into_iter().find(|m| *m == mix) else {
                return Ok(Some(refuse(404, "No such mix")));
            };
            if !offered(state).contains(&mix) {
                return Ok(Some(refuse(
                    409,
                    if mix == LASTFM {
                        "Connect Last.fm in Settings › Scrobbling first"
                    } else {
                        "YouTube Music is off"
                    },
                )));
            }
            match (method, refresh) {
                (Method::Get, false) => Ok(Some(reply_json(200, mix_json(state, mix)?))),
                (Method::Post, true) => {
                    if offline(state) {
                        return Ok(Some(refuse(409, "Offline")));
                    }
                    let looked: i64 = state
                        .library
                        .lock()
                        .unwrap()
                        .setting(&format!("{LOOKED_SETTING}{mix}"))
                        .ok()
                        .flatten()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    let look_too = now_ms() - looked >= LOOK_AGAIN_ON_REFRESH_MS;
                    make(
                        state,
                        mix,
                        now_ms() as u64,
                        look_too,
                        &AtomicBool::new(false),
                    );
                    Ok(Some(reply_json(200, mix_json(state, mix)?)))
                }
                _ => Ok(Some(refuse(405, "Not here"))),
            }
        }
    }
}
