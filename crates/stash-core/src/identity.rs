//! Which song is which. A library that takes songs from files, add-ons, a phone and a web page has to know when two
//! of them are one song, so a like made on the phone lands on the file already on this computer.
//!
//! The rule is the one the Stash web player and the Android app use, so a library moved between them stays one
//! library: a song's identity is its words (title and artist, folded), or its ISRC when it has one; the same words
//! with and without an ISRC are one song; the same words with two different ISRCs are two recordings.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Text as identity compares it: accents and case gone, a bracketed "feat." credit gone, everything that is not a
/// letter or a digit a single space. "Beyoncé" is "beyonce"; "Song (feat. X) [Live]" is "song live".
///
/// Only a bracket that starts with "feat", "ft" or "featuring" as a whole word is dropped: "(Live)", "(Remastered
/// 2011)" and "(Acoustic)" are other recordings or other titles and stay.
pub fn fold(text: &str) -> String {
    let plain: String = text
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    let chars: Vec<char> = plain.chars().collect();
    let mut kept = String::with_capacity(plain.len());
    let mut at = 0;
    while at < chars.len() {
        if let Some(end) = feat_bracket(&chars, at) {
            kept.push(' ');
            at = end;
            continue;
        }
        kept.push(chars[at]);
        at += 1;
    }
    let mut out = String::with_capacity(kept.len());
    for c in kept.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// When a bracketed featuring credit opens at `at`: the place just after its closing bracket.
fn feat_bracket(chars: &[char], at: usize) -> Option<usize> {
    if !matches!(chars[at], '(' | '[') {
        return None;
    }
    let mut word = at + 1;
    while chars.get(word).is_some_and(|c| c.is_whitespace()) {
        word += 1;
    }
    let after = ["featuring", "feat", "ft"].iter().find_map(|marker| {
        let end = word + marker.chars().count();
        let same = chars.get(word..end)?.iter().copied().eq(marker.chars());
        // A whole word: "ft" in "(ftw)" or "feat" in "(feather)" is not a credit.
        let whole = !chars.get(end).is_some_and(|c| c.is_alphanumeric());
        (same && whole).then_some(end)
    })?;
    let close = chars[after..].iter().position(|c| matches!(c, ')' | ']'))?;
    Some(after + close + 1)
}

/// A song's words as one key: folded title, a bar, folded artist.
pub fn words_key(title: &str, artist: &str) -> String {
    format!("{}|{}", fold(title), fold(artist))
}

/// An ISRC as kept: twelve letters and digits, upper case. Anything else is not an ISRC.
pub fn isrc(text: &str) -> Option<String> {
    (text.len() == 12 && text.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| text.to_ascii_uppercase())
}

/// Whether two songs with the same words are one song by their ISRCs: yes, unless both have one and they differ.
pub fn isrcs_agree(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => true,
    }
}

/// Text from outside (a file someone hands over, an add-on's answer) made fit to keep and show: control characters
/// and the characters that turn text around are taken out, runs of white space become one space, and it is cut to
/// `max` characters.
pub fn clean_text(text: &str, max: usize) -> String {
    let safe: String = text
        .chars()
        .map(|c| {
            // Characters that turn text around or hide in it: bidi marks, embeddings, overrides and isolates,
            // zero-width characters, line and paragraph separators, the byte order mark.
            let turns_text = matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2028}' | '\u{2029}' | '\u{2060}' | '\u{2066}'..='\u{2069}' | '\u{061C}' | '\u{FEFF}');
            if c.is_control() || turns_text { ' ' } else { c }
        })
        .collect();
    let mut out = String::with_capacity(safe.len());
    for word in safe.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    match out.char_indices().nth(max) {
        Some((cut, _)) => out[..cut].trim_end().to_string(),
        None => out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_keeps_what_makes_a_recording_and_drops_what_does_not() {
        assert_eq!(fold("Pink + White"), "pink white");
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(fold("  HALO!! "), "halo");
        // The ligature and the full-width letters are the same letters.
        assert_eq!(fold("ﬁre Ｗalk"), "fire walk");
        // A featuring credit in brackets is decoration, whatever the bracket and the spelling.
        assert_eq!(fold("Song (feat. Someone Else)"), "song");
        assert_eq!(fold("Song [ft Someone] (Live)"), "song live");
        assert_eq!(fold("Song (Featuring A & B)"), "song");
        assert_eq!(fold("Song ( FEAT. A )"), "song");
        // Everything else in brackets is part of what the song is.
        assert_eq!(fold("Dance (Without You)"), "dance without you");
        assert_eq!(fold("Song (Remastered 2011)"), "song remastered 2011");
        // The marker must be a whole word at the start of the bracket.
        assert_eq!(fold("Bird (Feather)"), "bird feather");
        assert_eq!(fold("Win (ftw)"), "win ftw");
        assert_eq!(fold("Song (with feat. inside)"), "song with feat inside");
        // A bracket that never closes is not a credit.
        assert_eq!(fold("Song (feat. Someone"), "song feat someone");
        // Other scripts are letters too.
        assert_eq!(fold("夜に駆ける"), "夜に駆ける");
        assert_eq!(fold(""), "");
    }

    #[test]
    fn a_song_is_its_words_and_isrcs_only_part_two_recordings() {
        assert_eq!(
            words_key("Pink + White", "Frank Ocean"),
            "pink white|frank ocean"
        );
        assert_eq!(
            words_key("Halo (feat. X)", "BEYONCÉ"),
            words_key("halo", "Beyonce")
        );
        assert_eq!(isrc("usum71607009").as_deref(), Some("USUM71607009"));
        assert_eq!(isrc("USUM7160700"), None);
        assert_eq!(isrc("USUM-1607009"), None);
        assert!(isrcs_agree(Some("USUM71607009"), None));
        assert!(isrcs_agree(None, None));
        assert!(isrcs_agree(Some("usum71607009"), Some("USUM71607009")));
        assert!(!isrcs_agree(Some("USUM71607009"), Some("GBAAA0000001")));
    }

    #[test]
    fn text_from_outside_is_made_fit_to_keep() {
        assert_eq!(clean_text("  Night \t\n drive  ", 100), "Night drive");
        assert_eq!(
            clean_text("evil\u{202E}txt\u{0000}.mp3", 100),
            "evil txt .mp3"
        );
        assert_eq!(clean_text("abcdef", 3), "abc");
        assert_eq!(clean_text("ab cdef", 3), "ab");
        assert_eq!(clean_text("\u{0007}", 10), "");
    }
}
