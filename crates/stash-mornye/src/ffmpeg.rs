//! FFmpeg, for what some extensions hand over unfinished: a song sent encrypted with its key (Amazon Music's), pieces
//! to be put into one file (SoundCloud's). Stash uses its own copy (in its tools folder, beside the library) or the
//! system's; its own is fetched when the listener asks, on Windows: the newest release build of FFmpeg-Builds (LGPL)
//! on GitHub, checked against the SHA-256 the release publishes, of which only `ffmpeg.exe` is kept.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};

#[cfg(windows)]
pub const EXE: &str = "ffmpeg.exe";
#[cfg(not(windows))]
pub const EXE: &str = "ffmpeg";

const RELEASE: &str =
    "https://api.github.com/repos/FFmpeg-Builds/FFmpeg-Builds/releases/tags/latest";
/// The largest archive taken.
const LARGEST: u64 = 400 * 1024 * 1024;

/// Stash's own copy in `tools`, else the system's on the PATH.
pub fn find(tools: &Path) -> Option<PathBuf> {
    let own = tools.join(EXE);
    if own.is_file() {
        return Some(own);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(EXE))
        .find(|path| path.is_file())
}

/// The build's name for this computer, when there is one Stash can fetch.
fn arch() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("win64"),
        ("windows", "aarch64") => Some("winarm64"),
        _ => None,
    }
}

/// Whether Stash can fetch FFmpeg for this computer.
pub fn can_fetch() -> bool {
    arch().is_some()
}

/// The newest release build for `arch` among a release's assets (`ffmpeg-n<version>-latest-<arch>-lgpl-<version>.zip`),
/// else the master build: its name and address.
fn pick(assets: &[Value], arch: &str) -> Option<(String, String)> {
    let named = |a: &Value| -> Option<(String, String)> {
        Some((
            a.get("name")?.as_str()?.to_string(),
            a.get("browser_download_url")?.as_str()?.to_string(),
        ))
    };
    let version = |name: &str| -> Option<Vec<u32>> {
        let rest = name.strip_prefix("ffmpeg-n")?;
        let (version, tail) = rest.split_once("-latest-")?;
        (tail == format!("{arch}-lgpl-{version}.zip")).then_some(())?;
        version.split('.').map(|p| p.parse().ok()).collect()
    };
    let released = assets
        .iter()
        .filter_map(named)
        .filter_map(|(name, url)| Some((version(&name)?, name, url)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, name, url)| (name, url));
    released.or_else(|| {
        assets
            .iter()
            .filter_map(named)
            .find(|(name, _)| *name == format!("ffmpeg-master-latest-{arch}-lgpl.zip"))
    })
}

/// The SHA-256 a `sha256sum` list gives `file`.
fn sum_for(list: &str, file: &str) -> Option<String> {
    list.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hex = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == file && hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hex.to_ascii_lowercase())
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .user_agent(concat!("Stash/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn get(
    agent: &ureq::Agent,
    url: &str,
    what: &str,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let answer = agent
        .get(url)
        .call()
        .map_err(|e| format!("{what} could not be fetched ({e})"))?;
    if !answer.status().is_success() {
        return Err(format!(
            "{what} could not be fetched (GitHub said {})",
            answer.status().as_u16()
        ));
    }
    Ok(answer)
}

fn text(agent: &ureq::Agent, url: &str, what: &str) -> Result<String, String> {
    get(agent, url, what)?
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("{what} could not be read ({e})"))
}

/// Fetches FFmpeg into `tools` (replacing Stash's copy there). `told` hears how much of the archive is in, and its
/// size.
pub fn fetch(tools: &Path, told: &dyn Fn(u64, Option<u64>)) -> Result<PathBuf, String> {
    let arch = arch().ok_or_else(|| "Stash cannot fetch FFmpeg for this computer".to_string())?;
    std::fs::create_dir_all(tools)
        .map_err(|e| format!("Stash's tools folder could not be made ({e})"))?;
    let agent = agent();
    let release: Value = serde_json::from_str(&text(&agent, RELEASE, "FFmpeg's release")?)
        .map_err(|_| "FFmpeg's release could not be read".to_string())?;
    let assets = release
        .get("assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (name, url) = pick(&assets, arch)
        .ok_or_else(|| "FFmpeg's release has no build for this computer".to_string())?;
    let sums_url = assets
        .iter()
        .find(|a| a.get("name").and_then(Value::as_str) == Some("checksums.sha256"))
        .and_then(|a| a.get("browser_download_url")?.as_str())
        .ok_or_else(|| "FFmpeg's release lists no checksums".to_string())?;
    let expected = sum_for(&text(&agent, sums_url, "FFmpeg's checksums")?, &name)
        .ok_or_else(|| format!("FFmpeg's release does not say what {name} should be"))?;

    // The archive, to a file of its own, hashed as it comes.
    let archive = tools.join("ffmpeg-download.zip.part");
    let fetched = (|| -> Result<(), String> {
        let mut answer = get(&agent, &url, "FFmpeg")?;
        let total = answer
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if total.is_some_and(|t| t > LARGEST) {
            return Err("FFmpeg's archive is larger than Stash takes".into());
        }
        let mut body = answer.body_mut().with_config().limit(LARGEST).reader();
        let mut out = std::fs::File::create(&archive)
            .map_err(|e| format!("FFmpeg could not be written ({e})"))?;
        let mut hash = Sha256::new();
        let mut chunk = vec![0u8; 256 * 1024];
        let (mut got, mut last) = (0u64, Instant::now());
        loop {
            let n = body
                .read(&mut chunk)
                .map_err(|e| format!("FFmpeg stopped coming in ({e})"))?;
            if n == 0 {
                break;
            }
            hash.update(&chunk[..n]);
            out.write_all(&chunk[..n])
                .map_err(|e| format!("FFmpeg could not be written ({e})"))?;
            got += n as u64;
            if last.elapsed() > Duration::from_millis(200) {
                last = Instant::now();
                told(got, total);
            }
        }
        out.flush()
            .map_err(|e| format!("FFmpeg could not be written ({e})"))?;
        told(got, total);
        if crate::hex(&hash.finalize()) != expected {
            return Err(
                "FFmpeg did not match what its release says it is: nothing was kept (try again)"
                    .into(),
            );
        }
        Ok(())
    })();
    if let Err(why) = fetched {
        let _ = std::fs::remove_file(&archive);
        return Err(why);
    }

    // Only the program is kept.
    let kept = (|| -> Result<PathBuf, String> {
        let file = std::fs::File::open(&archive)
            .map_err(|e| format!("FFmpeg's archive could not be opened ({e})"))?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|e| format!("FFmpeg's archive could not be opened ({e})"))?;
        let inside = (0..zip.len())
            .filter_map(|i| {
                let file = zip.by_index(i).ok()?;
                file.name().ok().map(|name| name.to_string())
            })
            .find(|n| n.ends_with(&format!("/bin/{EXE}")))
            .ok_or_else(|| "FFmpeg's archive holds no program".to_string())?;
        let mut program = zip
            .by_name(&inside)
            .map_err(|_| "FFmpeg's archive holds no program".to_string())?;
        let part = tools.join(format!("{EXE}.part"));
        let mut out = std::fs::File::create(&part)
            .map_err(|e| format!("FFmpeg could not be written ({e})"))?;
        std::io::copy(&mut program.by_ref().take(LARGEST), &mut out)
            .map_err(|e| format!("FFmpeg could not be unpacked ({e})"))?;
        drop(out);
        let place = tools.join(EXE);
        std::fs::rename(&part, &place).map_err(|e| {
            let _ = std::fs::remove_file(&part);
            format!("FFmpeg could not be put in place ({e})")
        })?;
        Ok(place)
    })();
    let _ = std::fs::remove_file(&archive);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_newest_release_build_is_picked_else_master() {
        let asset = |name: &str| json!({ "name": name, "browser_download_url": format!("https://example.com/{name}") });
        let assets = vec![
            asset("checksums.sha256"),
            asset("ffmpeg-master-latest-win64-lgpl.zip"),
            asset("ffmpeg-n8.1-latest-win64-lgpl-8.1.zip"),
            asset("ffmpeg-n9.0-latest-win64-lgpl-9.0.zip"),
            asset("ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip"),
            asset("ffmpeg-n9.0-latest-winarm64-lgpl-9.0.zip"),
        ];
        assert_eq!(
            pick(&assets, "win64").unwrap().0,
            "ffmpeg-n9.0-latest-win64-lgpl-9.0.zip"
        );
        assert_eq!(
            pick(&assets, "winarm64").unwrap().0,
            "ffmpeg-n9.0-latest-winarm64-lgpl-9.0.zip"
        );
        assert_eq!(
            pick(&assets[..2], "win64").unwrap().0,
            "ffmpeg-master-latest-win64-lgpl.zip"
        );
        assert!(pick(&assets[..1], "win64").is_none());
    }

    #[test]
    fn a_release_s_sums_are_read_for_the_file_asked() {
        let list = format!(
            "{}  ffmpeg-n9.0-latest-win64-lgpl-9.0.zip\n{}  other.zip\n",
            "A".repeat(64),
            "b".repeat(64)
        );
        assert_eq!(
            sum_for(&list, "ffmpeg-n9.0-latest-win64-lgpl-9.0.zip"),
            Some("a".repeat(64))
        );
        assert_eq!(sum_for(&list, "missing.zip"), None);
    }
}
