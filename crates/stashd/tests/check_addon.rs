//! `stashd check-addon` against the demo add-on, as its maker would run it: both kinds of add-on pass, each
//! resource is asked once, and what is not an add-on fails at its manifest.

use std::path::Path;
use std::time::Duration;

use stashd::check_addon::{Verdict, check};
use stashd::demo_addon::DemoAddon;

fn silence(path: &Path) {
    let data = 44_100u32 * 2 * 2;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&44_100u32.to_le_bytes());
    wav.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    wav.resize(44 + data as usize, 0);
    std::fs::write(path, wav).unwrap();
}

#[test]
fn an_add_on_is_checked_as_stash_would_ask_it() {
    let dir = std::env::temp_dir().join(format!("stash-check-addon-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    silence(&dir.join("kilo.wav"));
    silence(&dir.join("lima.wav"));

    for eclipse in [false, true] {
        let demo = DemoAddon::bind_as(&dir, 0, Duration::ZERO, eclipse).unwrap();
        let link = demo.link();
        let asked = demo.asked();
        std::thread::spawn(move || demo.run());
        let lines = check(&link, "kilo");
        let said: Vec<String> = lines
            .iter()
            .map(|l| format!("{:?} {} {}", l.verdict, l.what, l.words))
            .collect();
        assert!(
            lines.iter().all(|l| l.verdict != Verdict::Fail),
            "{said:#?}"
        );
        let verdict = |what: &str| lines.iter().find(|l| l.what == what).map(|l| l.verdict);
        assert_eq!(verdict("manifest"), Some(Verdict::Ok), "{said:#?}");
        assert_eq!(verdict("search"), Some(Verdict::Ok), "{said:#?}");
        assert_eq!(verdict("stream"), Some(Verdict::Ok), "{said:#?}");
        assert_eq!(
            verdict("download"),
            Some(if eclipse {
                Verdict::Skipped
            } else {
                Verdict::Ok
            }),
            "{said:#?}"
        );
        let stream = lines.iter().find(|l| l.what == "stream").unwrap();
        assert!(stream.words.contains("a wav file"), "{said:#?}");
        // The link is never printed: it can carry a key.
        assert!(lines.iter().all(|l| !l.words.contains(&link)), "{said:#?}");
        // Each thing is asked once.
        let asked = asked.lock().unwrap().clone();
        let searches = asked.iter().filter(|a| a.contains("/search")).count();
        assert_eq!(searches, 1, "{asked:#?}");
    }

    let lines = check("https://example.invalid/manifest.json", "kilo");
    assert_eq!(lines.len(), 1);
    assert_eq!(
        (lines[0].verdict, lines[0].what),
        (Verdict::Fail, "manifest")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
