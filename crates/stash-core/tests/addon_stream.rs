//! An add-on from link to sound: a small add-on server on this computer is added by its link, searched, asked for two
//! streams, and the two streams are played back to back through the engine. The join must be gapless and bit-exact,
//! as for files. Also: how the host reads an add-on's errors and refuses answers the contract does not allow.

mod common;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use common::{
    RATE, assert_same_samples, library, play_to_file, read_wav, scratch, signal, wav_bytes,
};
use stash_core::addons::{Addon, Code, Delivery, StreamRequest, Tier};
use stash_core::audio::EndReason;

struct Answer {
    status: u16,
    content_type: &'static str,
    extra_header: Option<(&'static str, String)>,
    body: Vec<u8>,
}

fn json(status: u16, body: String) -> Answer {
    Answer {
        status,
        content_type: "application/json",
        extra_header: None,
        body: body.into_bytes(),
    }
}

/// Serves `routes(base)` (path without the query, to its answer) on a free port of this computer and returns the
/// base URL. The thread lives as long as the test program.
fn serve(routes: impl FnOnce(&str) -> HashMap<String, Answer>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let routes = Arc::new(routes(&base));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let routes = Arc::clone(&routes);
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut byte) {
                        Ok(1) => head.push(byte[0]),
                        _ => return,
                    }
                }
                let head = String::from_utf8_lossy(&head);
                let target = head.split_whitespace().nth(1).unwrap_or("/");
                let path = target.split('?').next().unwrap_or("/");
                let missing = json(
                    404,
                    r#"{"error":{"code":"not_found","message":"no such route"}}"#.into(),
                );
                let answer = routes.get(path).unwrap_or(&missing);
                let mut response = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    answer.status,
                    answer.content_type,
                    answer.body.len()
                );
                if let Some((name, value)) = &answer.extra_header {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&answer.body);
            });
        }
    });
    base
}

const MANIFEST: &str = r#"{"stash":1,"id":"tone-test","name":"Tone Test","version":"1.0.0","kind":"server",
    "resources":["search","stream","something-new"],"stream":{"cors":false,"manifests":["none"],"tiers":["lossless"]}}"#;

fn stream_info(url: &str, tier: &str, codec: &str) -> String {
    format!(
        r#"{{"url":"{url}","manifest":"none","tier":"{tier}","codec":"{codec}","container":"wav","bitDepth":16,"sampleRate":44100,"cors":false}}"#
    )
}

#[test]
fn an_add_on_added_from_a_link_is_searched_and_its_streams_play_gapless() {
    let (first, second) = (30_011usize, 40_007usize);
    let whole = signal(first + second);
    let (a, b) = (
        wav_bytes(RATE, &whole[..first * 2]),
        wav_bytes(RATE, &whole[first * 2..]),
    );
    let base = serve(|base| {
        let mut routes = HashMap::new();
        routes.insert("/manifest.json".into(), json(200, MANIFEST.into()));
        routes.insert(
            "/search".into(),
            json(200, r#"{"tracks":[{"id":"a","title":"Tone, part one","artist":"Sine","durationMs":680,"maxTier":"lossless"},
                                    {"id":"b","title":"Tone, part two","artist":"Sine","album":"Waves","isrc":"XX0000000001"}]}"#.into()),
        );
        routes.insert(
            "/stream/a".into(),
            json(
                200,
                stream_info(&format!("{base}/media/a.wav"), "lossless", "pcm"),
            ),
        );
        routes.insert(
            "/stream/b".into(),
            json(
                200,
                stream_info(&format!("{base}/media/b.wav"), "lossless", "pcm"),
            ),
        );
        routes.insert(
            "/media/a.wav".into(),
            Answer {
                status: 200,
                content_type: "audio/wav",
                extra_header: None,
                body: a,
            },
        );
        routes.insert(
            "/media/b.wav".into(),
            Answer {
                status: 200,
                content_type: "audio/wav",
                extra_header: None,
                body: b,
            },
        );
        routes
    });

    let addon = Addon::from_link(&format!("{base}/manifest.json")).unwrap();
    assert_eq!(
        (addon.manifest.id.as_str(), addon.manifest.name.as_str()),
        ("tone-test", "Tone Test")
    );

    let tracks = addon.search("tone", 10).unwrap();
    assert_eq!(
        tracks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(
        (
            tracks[0].duration_ms,
            tracks[0].max_tier,
            tracks[1].album.as_deref()
        ),
        (Some(680), Some(Tier::Lossless), Some("Waves"))
    );

    let want = StreamRequest {
        tier: Tier::Hires,
        min_tier: Tier::Lossless,
        deliveries: vec![Delivery::None],
    };
    let streams: Vec<String> = tracks
        .iter()
        .map(|t| addon.stream(&t.id, &want).unwrap().url)
        .collect();

    // From here it needs the engine.
    let Some(library) = library() else { return };
    let dir = scratch("addon-stream");
    let out = dir.join("out.wav");
    let played = play_to_file(&library, &[&streams[0], &streams[1]], &out);
    assert_eq!(
        played.ended,
        [EndReason::Eof, EndReason::Eof],
        "log:\n{}",
        played.log_text()
    );
    assert_eq!(
        played.output_opened,
        1,
        "one output for both streams; log:\n{}",
        played.log_text()
    );
    assert_same_samples(&read_wav(&out).samples(), &whole, first);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_add_ons_errors_are_read_as_the_contract_says_and_bad_answers_are_refused() {
    let base = serve(|base| {
        let mut routes = HashMap::new();
        // This add-on offers streams only.
        routes.insert(
            "/manifest.json".into(),
            json(200, MANIFEST.replace("\"search\",", "")),
        );
        routes.insert("/search".into(), json(200, r#"{"tracks":[]}"#.into()));
        routes.insert(
            "/stream/gone".into(),
            json(
                404,
                r#"{"error":{"code":"not_found","message":"no such track"}}"#.into(),
            ),
        );
        routes.insert(
            "/stream/lossy-only".into(),
            json(
                404,
                r#"{"error":{"code":"quality_unavailable","message":"only AAC"}}"#.into(),
            ),
        );
        routes.insert(
            "/stream/busy".into(),
            json(
                503,
                r#"{"error":{"code":"unavailable","message":"busy","retryAfter":7}}"#.into(),
            ),
        );
        routes.insert(
            "/stream/limited".into(),
            Answer {
                status: 429,
                content_type: "text/plain",
                extra_header: Some(("Retry-After", "30".into())),
                body: b"slow down".to_vec(),
            },
        );
        routes.insert("/stream/broken".into(), json(500, "oops".into()));
        routes.insert(
            "/stream/lies".into(),
            json(
                200,
                stream_info("https://cdn.example.org/a.m4a", "high", "aac"),
            ),
        );
        routes.insert(
            "/stream/plain-http".into(),
            json(
                200,
                stream_info("http://cdn.example.org/a.wav", "lossless", "pcm"),
            ),
        );
        routes.insert(
            "/stream/moved".into(),
            Answer {
                status: 302,
                content_type: "text/plain",
                extra_header: Some(("Location", format!("{base}/stream/a"))),
                body: Vec::new(),
            },
        );
        routes.insert(
            "/stream/not-json".into(),
            json(200, "<html>hello</html>".into()),
        );
        routes
    });
    let addon = Addon::from_link(&format!("{base}/manifest.json")).unwrap();
    let want = StreamRequest {
        tier: Tier::Hires,
        min_tier: Tier::Lossless,
        deliveries: vec![Delivery::None],
    };
    let fail = |id: &str| addon.stream(id, &want).unwrap_err();

    let gone = fail("gone");
    assert_eq!(
        (gone.code, gone.is_miss(), gone.message.as_str()),
        (Code::NotFound, true, "no such track")
    );
    assert_eq!(
        (fail("lossy-only").code, fail("lossy-only").is_miss()),
        (Code::QualityUnavailable, true)
    );
    // Trouble with the add-on is not a miss, and its wait is taken from the body, else from the header.
    let busy = fail("busy");
    assert_eq!(
        (busy.code, busy.is_miss(), busy.retry_after),
        (Code::Unavailable, false, Some(7))
    );
    let limited = fail("limited");
    assert_eq!(
        (limited.code, limited.retry_after),
        (Code::RateLimited, Some(30))
    );
    assert_eq!(
        (fail("broken").code, fail("broken").retry_after),
        (Code::Unavailable, None)
    );
    // Answers the contract does not allow: a tier below what was asked, a stream that is not https, a redirect, not JSON.
    for id in ["lies", "plain-http", "moved", "not-json"] {
        assert_eq!(fail(id).code, Code::InvalidResponse, "{id}");
    }
    // A resource the manifest does not list is never asked for, though the server would answer it.
    let unlisted = addon.search("x", 5).unwrap_err();
    assert!(
        unlisted.message.contains("does not offer search"),
        "{unlisted}"
    );
}

#[test]
fn a_link_that_is_not_an_https_manifest_is_refused_before_any_request() {
    for link in [
        "http://addon.example.org/manifest.json",
        "https://addon.example.org/",
        "ftp://addon.example.org/manifest.json",
        "not a url",
    ] {
        assert!(Addon::from_link(link).is_err(), "{link}");
    }
}
