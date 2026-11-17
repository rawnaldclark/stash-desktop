//! An add-on as BitChord's are made: added by the address it is under (no "/manifest.json"), listing nothing of what
//! it offers, declaring settings it counts on getting back, and saying how good a stream is only in its own words.
//! The add-on is a small server of the test's own, which notes what it was asked.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use serde_json::json;
use stash_core::addons::{Addon, Code, Delivery, StreamRequest, Tier};
use stash_core::reach::Reach;

fn addon() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let noted = Arc::clone(&asked);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
            noted.lock().unwrap().push(path.clone());
            let route = path.split('?').next().unwrap_or_default();
            let (status, answer) = match route {
                "/key123/manifest.json" => (
                    200,
                    json!({
                        "id": "Nebula_Tidal", "name": "Nebula", "version": "2.1",
                        "bitchordAuth": "off", "allowDownloads": "no",
                        "settings": [
                            { "key": "quality", "type": "dropdown", "options": [{ "value": "HI_RES_LOSSLESS" }, { "value": "HIGH" }] },
                            { "key": "region", "default": "US" },
                            { "key": "debrid", "default": true }
                        ]
                    }),
                ),
                "/key123/search" => (
                    200,
                    json!({ "tracks": [
                        { "id": 77, "title": "Song", "artist": "Band", "duration": 200, "audioQuality": "HI_RES_LOSSLESS" },
                        { "id": 78, "title": "No artist here" }
                    ] }),
                ),
                "/key123/stream/77" => (
                    200,
                    json!({ "url": "https://cdn.example.org/manifest.mpd", "format": "dash",
                            "quality": "Tidal · Hi-Res FLAC (DASH)", "streamQuality": "[Tidal] HI_RES_LOSSLESS" }),
                ),
                "/key123/stream/78" => (
                    200,
                    json!({ "error": "Still fetching it", "retryAfter": 12 }),
                ),
                "/key123/stream/79" => (
                    200,
                    json!({ "error": "Add your key in the add-on's settings" }),
                ),
                _ => (404, json!({})),
            };
            let answer = answer.to_string();
            let mut out = stream;
            let _ = write!(
                out,
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    (base, asked)
}

#[test]
fn an_add_on_made_for_bitchord_is_added_and_asked_as_bitchord_asks_it() {
    let (base, asked) = addon();
    // Added by the address it is under, as its maker hands it out.
    let added = Addon::from_link(&format!("{base}/key123")).unwrap();
    assert_eq!(asked.lock().unwrap().as_slice(), ["/key123/manifest.json"]);
    assert_eq!(added.link(), format!("{base}/key123/manifest.json"));
    assert_eq!(added.reach(), Reach::Here);
    let m = &added.manifest;
    assert_eq!(
        (m.id.as_str(), m.name.as_str(), m.version.as_str()),
        ("nebula-tidal", "Nebula", "2.1")
    );
    // It lists nothing: it searches and plays. "no" is a no: its songs are not saved.
    assert_eq!(m.resources, ["search", "stream"]);
    assert!(!added.offers_download());
    // Kept and read again, it is the same add-on.
    let kept = Addon::from_stored(added.link(), &added.manifest_json()).unwrap();
    assert_eq!(kept.manifest.sent, added.manifest.sent);

    // Every request carries the defaults of its settings, and the quality it calls its best.
    asked.lock().unwrap().clear();
    let found = kept.search("song band", 10).unwrap();
    assert_eq!(
        found.len(),
        1,
        "a row without an artist is left out: {found:?}"
    );
    assert_eq!(
        (found[0].id.as_str(), found[0].duration_ms),
        ("77", Some(200_000))
    );
    let stream = kept.stream("77", &StreamRequest::default()).unwrap();
    assert_eq!(
        (stream.tier, stream.codec.as_deref(), stream.manifest),
        (Tier::Hires, Some("flac"), Delivery::Dash)
    );
    // Said to be hi-res lossless in its own words: it is one where only lossless is asked for.
    let lossless_only = StreamRequest {
        min_tier: Tier::Lossless,
        ..StreamRequest::default()
    };
    assert!(kept.stream("77", &lossless_only).is_ok());
    {
        let asked = asked.lock().unwrap();
        assert_eq!(
            asked[0],
            "/key123/search?q=song+band&region=US&debrid=true&quality=HI_RES_LOSSLESS"
        );
        assert_eq!(
            asked[1],
            "/key123/stream/77?region=US&debrid=true&quality=HI_RES_LOSSLESS"
        );
    }

    // "Not yet" is trouble to ask again about, not "it is not there"; a plain error is, in the add-on's own words.
    let not_yet = kept.stream("78", &StreamRequest::default()).unwrap_err();
    assert_eq!(
        (not_yet.code, not_yet.retry_after, not_yet.is_miss()),
        (Code::Unavailable, Some(12), false)
    );
    let missing = kept.stream("79", &StreamRequest::default()).unwrap_err();
    assert_eq!(missing.code, Code::NotFound);
    assert!(
        missing
            .in_words()
            .contains("Add your key in the add-on's settings")
    );
}
