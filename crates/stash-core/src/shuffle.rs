//! Shuffle that spreads artists. A fair shuffle often puts two songs by one artist side by side, and a listener hears
//! that as "not random". Here each artist's songs are laid evenly across the queue from a start chosen by chance,
//! each a little off the even step, and the queue is read in that order (the dithering Spotify described in 2014).
//! An artist's own songs take turns between its albums the same way, and the first song is not by the artist of the
//! song just before it when another artist can come first. The artist is the first one a credit names
//! ("Drake, Future" is Drake's).

use std::collections::HashMap;

use crate::identity::fold;
use crate::matching::artists;

/// Chance: xorshift64, from a seed.
pub struct Chance(u64);

impl Chance {
    pub fn new(seed: u64) -> Self {
        // A xorshift state of 0 stays 0.
        Chance(seed | 1)
    }

    /// Seeded from the clock.
    pub fn from_clock() -> Self {
        Chance::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15),
        )
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `list` in an order by chance (Fisher–Yates).
    fn mix<T>(&mut self, list: &mut [T]) {
        for i in (1..list.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            list.swap(i, j);
        }
    }
}

/// Puts `items[from..]` in a new order by chance, each artist's songs spread out. `song` gives an item's artist
/// credit and album.
pub fn spread_from<T>(
    items: &mut Vec<T>,
    from: usize,
    song: impl Fn(&T) -> (&str, Option<&str>),
    chance: &mut Chance,
) {
    if from >= items.len() {
        return;
    }
    let keys: Vec<(String, String)> = items[from..]
        .iter()
        .map(|item| {
            let (artist, album) = song(item);
            (artist_key(artist), album.map(fold).unwrap_or_default())
        })
        .collect();
    let order = spread(&keys, chance);
    let mut tail: Vec<Option<T>> = items.drain(from..).map(Some).collect();
    items.extend(order.iter().map(|&i| tail[i].take().expect("each once")));
    // The first is not by the artist just before, when another can come first.
    if from > 0 {
        let before = artist_key(song(&items[from - 1]).0);
        let same = |i: usize| keys[order[i - from]].0 == before;
        if same(from)
            && let Some(other) = (from + 1..items.len()).find(|&i| !same(i))
        {
            items[from..=other].rotate_right(1);
        }
    }
}

/// The artist a credit is counted under: the first one it names, folded.
fn artist_key(credit: &str) -> String {
    artists(credit)
        .into_iter()
        .next()
        .unwrap_or_else(|| fold(credit))
}

/// The order (places in `songs`) in which to play `songs`, given as (artist, album) keys.
fn spread(songs: &[(String, String)], chance: &mut Chance) -> Vec<usize> {
    let all: Vec<usize> = (0..songs.len()).collect();
    let mut placed = Vec::with_capacity(songs.len());
    for artist in grouped(&all, |i| &songs[i].0) {
        // The artist's songs take turns between its albums; each album's songs come in an order by chance.
        let mut inner = Vec::with_capacity(artist.len());
        for mut album in grouped(&artist, |i| &songs[i].1) {
            chance.mix(&mut album);
            lay(album, &mut inner, chance);
        }
        lay(read(inner), &mut placed, chance);
    }
    read(placed)
}

/// `members` in groups by `key`, in the order each group first appears.
fn grouped<'a>(members: &[usize], key: impl Fn(usize) -> &'a String) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut at: HashMap<&'a String, usize> = HashMap::new();
    for &i in members {
        let place = *at.entry(key(i)).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[place].push(i);
    }
    groups
}

/// Lays `members` evenly over [0, 1): from a start by chance within the first step, each next one a step on, give or
/// take a tenth of a step.
fn lay(members: Vec<usize>, placed: &mut Vec<(f64, usize)>, chance: &mut Chance) {
    let step = 1.0 / members.len() as f64;
    let start = chance.unit() * step;
    for (k, i) in members.into_iter().enumerate() {
        let off = (chance.unit() - 0.5) * 0.2 * step;
        placed.push((start + k as f64 * step + off, i));
    }
}

/// The members in the order they were laid.
fn read(mut placed: Vec<(f64, usize)>) -> Vec<usize> {
    placed.sort_by(|a, b| a.0.total_cmp(&b.0));
    placed.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    type Song = (String, Option<String>, usize);

    /// `sizes[a][b]` songs by artist `a` on its album `b`, numbered.
    fn songs(sizes: &[&[usize]]) -> Vec<Song> {
        let mut out = Vec::new();
        for (a, albums) in sizes.iter().enumerate() {
            for (b, &n) in albums.iter().enumerate() {
                for _ in 0..n {
                    let n = out.len();
                    out.push((format!("Artist {a}"), Some(format!("Album {b}")), n));
                }
            }
        }
        out
    }

    fn shuffled(list: &[Song], from: usize, seed: u64) -> Vec<Song> {
        let mut list = list.to_vec();
        spread_from(
            &mut list,
            from,
            |s| (s.0.as_str(), s.1.as_deref()),
            &mut Chance::new(seed),
        );
        list
    }

    /// How many times a song is followed by one with the same `key`.
    fn side_by_side(list: &[Song], key: impl Fn(&Song) -> String) -> usize {
        list.windows(2).filter(|w| key(&w[0]) == key(&w[1])).count()
    }

    #[test]
    fn every_song_comes_once_and_what_is_before_from_stays() {
        let list = songs(&[&[5, 2], &[3], &[1], &[1], &[4, 4, 1]]);
        for seed in 0..50 {
            for from in [0, 1, 7, list.len() - 1, list.len()] {
                let out = shuffled(&list, from, seed);
                assert_eq!(out[..from], list[..from]);
                let mut numbers: Vec<usize> = out.iter().map(|s| s.2).collect();
                numbers.sort_unstable();
                assert_eq!(numbers, (0..list.len()).collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn an_artist_is_spread_out_where_a_fair_shuffle_puts_songs_side_by_side() {
        // A library-like mix: a few artists with many songs, many with one or two.
        let list = songs(&[
            &[5, 3],
            &[6],
            &[4, 1],
            &[3],
            &[2, 2],
            &[2],
            &[1],
            &[1],
            &[1],
            &[1],
        ]);
        let artist = |s: &Song| s.0.clone();
        let (mut spread, mut fair) = (0, 0);
        for seed in 1..=300 {
            spread += side_by_side(&shuffled(&list, 0, seed), artist);
            let mut mixed = list.clone();
            Chance::new(seed).mix(&mut mixed);
            fair += side_by_side(&mixed, artist);
        }
        assert!(
            spread * 4 < fair,
            "side by side in 300 shuffles: spread {spread}, fair {fair}"
        );

        // Four artists with six songs each: hardly ever side by side.
        let even = songs(&[&[6], &[6], &[6], &[6]]);
        let total: usize = (1..=300)
            .map(|seed| side_by_side(&shuffled(&even, 0, seed), artist))
            .sum();
        assert!(total < 60, "side by side in 300 shuffles: {total}");
    }

    #[test]
    fn an_artist_takes_turns_between_its_albums() {
        let list = songs(&[&[4, 4, 4]]);
        let album = |s: &Song| s.1.clone().unwrap_or_default();
        let total: usize = (1..=300)
            .map(|seed| side_by_side(&shuffled(&list, 0, seed), album))
            .sum();
        assert!(
            total < 60,
            "same album side by side in 300 shuffles: {total}"
        );
    }

    #[test]
    fn the_first_is_not_by_the_artist_just_before() {
        // The song playing is by Artist 0, as are most of the rest.
        let list = songs(&[&[9], &[1]]);
        for seed in 1..=100 {
            let out = shuffled(&list, 1, seed);
            assert_eq!(out[1].0, "Artist 1", "seed {seed}");
        }
        // With only that artist left, the order is still by chance.
        let alone = songs(&[&[5]]);
        let out = shuffled(&alone, 1, 7);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn a_shared_credit_counts_under_its_first_artist() {
        assert_eq!(artist_key("Drake, Future"), artist_key("Drake"));
        assert_eq!(artist_key("Beyoncé feat. JAY-Z"), artist_key("BEYONCE"));
        assert_ne!(artist_key("Drake"), artist_key("Future"));
    }
}
