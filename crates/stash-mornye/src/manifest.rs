//! A package and its manifest (spec §8.1, §8.2; Lokal `morneye_packages.dart` 126-308).

use std::cmp::Ordering;
use std::io::Read;

use serde_json::Value;

use crate::net::Permissions;

/// The runtime features this host has, and the newest version of each (pkgs.dart:41-54).
const FEATURES: [(&str, u64); 12] = [
    ("signedSession", 3),
    ("sessionRefresh", 1),
    ("sessionGrant", 1),
    ("globalAction", 1),
    ("webviewAuth", 1),
    ("downloadSegments", 1),
    ("patternedFileTransform", 1),
    ("preparedContext", 1),
    ("accountForms", 1),
    ("accountDownloadMode", 1),
    ("directMedia", 1),
    ("deferredPreview", 1),
];

const MAX_SETTINGS: usize = 100;
const MAX_QUALITY_OPTIONS: usize = 30;
const MAX_PACKAGE: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 100;
const MAX_UNPACKED: u64 = 64 * 1024 * 1024;
const MAX_SCRIPT: usize = 8 * 1024 * 1024;

const INVALID_MANIFEST: &str = "Invalid Morneye package manifest";
const UNSAFE_PATH: &str = "Addon package contains an unsafe archive path";
const NOT_A_PACKAGE: &str = "This isn\u{2019}t a Morneye package";
const TOO_BIG_UNPACKED: &str = "Unpacked addon package exceeds 64 MB";

/// A checked manifest. `raw` is kept whole: settings, quality options, actions and capabilities are read from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub raw: Value,
    /// `^[a-z0-9][a-z0-9_-]{0,99}$`: the add-on's identity.
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
    /// `type`: `download_provider`, `metadata_provider`, …
    pub types: Vec<String>,
    pub permissions: Permissions,
    /// `signedSession`, when it has one.
    pub signed_session: Option<Value>,
    /// `settings`: fields `{key, type, label, description|help, default, required, secret, options, action,
    /// placeholder}`.
    pub settings: Vec<Value>,
    /// `qualityOptions`: `{id, label, name, kind, description, settings}`.
    pub quality_options: Vec<Value>,
    /// `globalActions`: `{action, label}`.
    pub global_actions: Vec<Value>,
    /// `capabilities` (`{}` when absent).
    pub capabilities: Value,
}

impl Manifest {
    /// Reads and checks a manifest (pkgs.dart:181-250): the errors as Lokal words them (`Only download-capable or
    /// playlist-link Morneye addons can be installed`, `This addon requires Morneye compatibility <v>`,
    /// `Unsupported runtime feature: <x>`, `Invalid addon network permissions`, …).
    pub fn read(raw: Value) -> Result<Manifest, String> {
        let Some(map) = raw.as_object() else {
            return Err(INVALID_MANIFEST.into());
        };
        let name = map.get("name").and_then(Value::as_str).unwrap_or_default();
        let display_name = map.get("displayName").and_then(Value::as_str);
        let version = map
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(display_name) = display_name.filter(|_| is_name(name) && is_version(version))
        else {
            return Err(INVALID_MANIFEST.into());
        };

        let types: Option<Vec<String>> = map.get("type").and_then(Value::as_array).map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        });
        let links = raw.pointer("/urlHandler/enabled").and_then(Value::as_bool) == Some(true);
        let installable = types.as_ref().is_some_and(|types| {
            types.iter().any(|t| t == "download_provider")
                || (links && types.iter().any(|t| t == "metadata_provider"))
        });
        let Some(types) = types.filter(|_| installable) else {
            return Err(
                "Only download-capable or playlist-link Morneye addons can be installed".into(),
            );
        };

        if compare_versions(
            &version_text(map.get("minAppVersion")),
            crate::COMPAT_VERSION,
        ) == Ordering::Greater
        {
            return Err(format!(
                "This addon requires Morneye compatibility {}",
                text_of(map.get("minAppVersion"))
            ));
        }
        if let Some(features) = map.get("requiredRuntimeFeatures").and_then(Value::as_array) {
            for feature in features {
                let wanted = text_of(Some(feature));
                if !feature_supported(&wanted) {
                    return Err(format!("Unsupported runtime feature: {wanted}"));
                }
            }
        }

        let permissions_value = map.get("permissions").cloned().unwrap_or(Value::Null);
        Permissions::check(&permissions_value)?;
        let permissions = Permissions::of(&permissions_value);
        let signed_session = match map.get("signedSession") {
            Some(signed @ Value::Object(_)) => {
                crate::net::validate_url(&text_of(signed.get("baseUrl")), &permissions)?;
                if permissions_value.get("storage") != Some(&Value::Bool(true)) {
                    return Err("Signed sessions require storage permission".into());
                }
                Some(signed.clone())
            }
            _ => None,
        };

        let list = |key: &str| -> Vec<Value> {
            map.get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter(|item| item.is_object())
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        let too_long = |key: &str, limit: usize| {
            map.get(key)
                .and_then(Value::as_array)
                .is_some_and(|items| items.len() > limit)
        };
        if too_long("settings", MAX_SETTINGS) || too_long("qualityOptions", MAX_QUALITY_OPTIONS) {
            return Err("Addon manifest is too large".into());
        }

        Ok(Manifest {
            name: name.to_string(),
            display_name: display_name.to_string(),
            version: version.to_string(),
            description: text_of(map.get("description")),
            types,
            permissions,
            signed_session,
            settings: list("settings"),
            quality_options: list("qualityOptions"),
            global_actions: list("globalActions"),
            capabilities: map
                .get("capabilities")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default())),
            raw,
        })
    }

    /// `capabilities.downloadTransfer` (`{}` when absent).
    pub fn download_transfer(&self) -> Value {
        self.capabilities
            .get("downloadTransfer")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()))
    }

    /// Whether it is a download provider.
    pub fn downloads(&self) -> bool {
        self.types.iter().any(|t| t == "download_provider")
    }

    /// Whether it handles links (`urlHandler.enabled`).
    pub fn handles_links(&self) -> bool {
        self.raw
            .pointer("/urlHandler/enabled")
            .and_then(Value::as_bool)
            == Some(true)
    }

    /// The provider it is (its name without `-web`).
    pub fn provider(&self) -> &str {
        self.name.strip_suffix("-web").unwrap_or(&self.name)
    }
}

/// The add-on's key: the first 10 hex characters of SHA-1("spotiflac:" + name) (pkgs.dart:126-127).
pub fn key_of(name: &str) -> String {
    let digest = crate::crypto::sha1(format!("spotiflac:{name}").as_bytes());
    crate::hex(&digest)[..10].to_string()
}

/// Compares two versions part by part, numerically (a part that is not a number counts 0) (pkgs.dart:129-143).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    fn parts(version: &str) -> Vec<i64> {
        version
            .split('.')
            .map(|part| part.trim().parse::<i64>().unwrap_or(0))
            .collect()
    }
    let (left, right) = (parts(a), parts(b));
    for i in 0..left.len().max(right.len()) {
        let order = left
            .get(i)
            .copied()
            .unwrap_or(0)
            .cmp(&right.get(i).copied().unwrap_or(0));
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

/// A package's files, checked (pkgs.dart:260-308): a ZIP of at most 32 MiB, 100 entries, 64 MiB unpacked, no unsafe
/// paths, `manifest.json` and `index.js` (at most 8 MiB) at its root.
pub struct Package {
    pub manifest: Manifest,
    /// Every file, by its path in the archive.
    pub files: Vec<(String, Vec<u8>)>,
}

impl Package {
    pub fn read(bytes: &[u8]) -> Result<Package, String> {
        if bytes.len() > MAX_PACKAGE {
            return Err("Addon package exceeds 32 MB".into());
        }
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|_| NOT_A_PACKAGE.to_string())?;
        if archive.len() > MAX_ENTRIES {
            return Err("Addon package contains too many files".into());
        }
        // The reader keeps one entry per name: a name given twice is refused, as Lokal refuses it.
        if archive.has_duplicate_names() {
            return Err(UNSAFE_PATH.into());
        }
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut declared: u64 = 0;
        let mut unpacked: u64 = 0;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|_| NOT_A_PACKAGE.to_string())?;
            let name = entry
                .name()
                .map_err(|_| UNSAFE_PATH.to_string())?
                .into_owned();
            if !safe_archive_path(&name)
                || entry.is_symlink()
                || files.iter().any(|(known, _)| *known == name)
            {
                return Err(UNSAFE_PATH.into());
            }
            declared = declared.saturating_add(entry.size());
            if declared > MAX_UNPACKED {
                return Err(TOO_BIG_UNPACKED.into());
            }
            if entry.is_file() {
                // What the entry says it holds is not trusted: read at most what is left, plus one byte to tell.
                let room = MAX_UNPACKED - unpacked;
                let mut content = Vec::new();
                (&mut entry)
                    .take(room + 1)
                    .read_to_end(&mut content)
                    .map_err(|_| NOT_A_PACKAGE.to_string())?;
                if content.len() as u64 > room {
                    return Err(TOO_BIG_UNPACKED.into());
                }
                unpacked += content.len() as u64;
                files.push((name, content));
            }
        }

        let file = |name: &str| {
            files
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, bytes)| bytes)
        };
        let (Some(manifest_bytes), Some(code)) = (file("manifest.json"), file("index.js")) else {
            return Err("A Morneye package needs manifest.json and index.js at its root".into());
        };
        let text = std::str::from_utf8(manifest_bytes).map_err(|_| INVALID_MANIFEST.to_string())?;
        let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|_| INVALID_MANIFEST.to_string())?;
        let manifest = Manifest::read(value)?;
        if code.len() > MAX_SCRIPT {
            return Err("Addon script exceeds 8 MB".into());
        }
        if std::str::from_utf8(code).is_err() {
            return Err("Addon script is not UTF-8 text".into());
        }
        Ok(Package { manifest, files })
    }

    /// `index.js`, as text.
    pub fn code(&self) -> String {
        self.files
            .iter()
            .find(|(name, _)| name == "index.js")
            .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default()
    }
}

/// Whether a file name is a package's (`.sflx`, `.morneye-ext`, `.spotiflac-ext`, any case).
pub fn is_package_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [".sflx", ".morneye-ext", ".spotiflac-ext"]
        .iter()
        .any(|end| name.ends_with(end))
}

/// `^[a-z0-9][a-z0-9_-]{0,99}$`: a package name (manifests and registries).
pub(crate) fn is_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=100).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// `^\d+\.\d+\.\d+(?:[-+][\w.-]+)?$`.
fn is_version(version: &str) -> bool {
    let (numbers, suffix) = match version.find(['-', '+']) {
        Some(at) => (&version[..at], Some(&version[at + 1..])),
        None => (version, None),
    };
    let parts: Vec<&str> = numbers.split('.').collect();
    let numbers_ok = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    let suffix_ok = suffix.is_none_or(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
    });
    numbers_ok && suffix_ok
}

/// `^([A-Za-z]+)@(\d+)$`, and this host has that feature at that version or newer.
fn feature_supported(wanted: &str) -> bool {
    let Some((name, version)) = wanted.split_once('@') else {
        return false;
    };
    if name.is_empty()
        || !name.bytes().all(|b| b.is_ascii_alphabetic())
        || version.is_empty()
        || !version.bytes().all(|b| b.is_ascii_digit())
    {
        return false;
    }
    let Ok(version) = version.parse::<u64>() else {
        return false;
    };
    FEATURES
        .iter()
        .any(|(feature, have)| *feature == name && *have >= version)
}

/// Lokal's archive path checks: not empty, no `\`, not absolute (`/…` or `X:…`), no `.` or `..` segment.
fn safe_archive_path(name: &str) -> bool {
    let bytes = name.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !name.is_empty()
        && !name.contains('\\')
        && !name.starts_with('/')
        && !drive
        && !name.split('/').any(|part| part == "." || part == "..")
}

/// A JSON value as Lokal's `'$value'` writes it: a string as it is, nothing as `""`, anything else as JSON.
pub(crate) fn text_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// A version as Lokal compares it: nothing counts as `0`.
pub(crate) fn version_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "0".to_string(),
        other => text_of(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn minimal() -> Value {
        json!({
            "name": "demo-addon",
            "displayName": "Demo",
            "version": "1.2.3",
            "description": "A test add-on",
            "type": ["download_provider"],
            "permissions": { "network": ["api.example.com", "*.cdn.example.com"], "storage": true, "file": true }
        })
    }

    fn with(key: &str, value: Value) -> Value {
        let mut manifest = minimal();
        manifest[key] = value;
        manifest
    }

    #[test]
    fn a_valid_manifest_is_read() {
        let manifest = Manifest::read(minimal()).unwrap();
        assert_eq!(manifest.name, "demo-addon");
        assert_eq!(manifest.display_name, "Demo");
        assert_eq!(manifest.version, "1.2.3");
        assert_eq!(manifest.description, "A test add-on");
        assert!(manifest.downloads());
        assert!(!manifest.handles_links());
        assert!(manifest.permissions.storage);
        assert_eq!(manifest.capabilities, json!({}));
        assert!(manifest.settings.is_empty());
        assert_eq!(manifest.provider(), "demo-addon");
    }

    #[test]
    fn a_web_provider_drops_its_suffix() {
        let manifest = Manifest::read(with("name", json!("qobuz-web"))).unwrap();
        assert_eq!(manifest.provider(), "qobuz");
    }

    #[test]
    fn names_are_checked() {
        for name in [
            json!("Demo"),
            json!("-demo"),
            json!(""),
            json!("a b"),
            json!(5),
            json!("x".repeat(101)),
        ] {
            assert_eq!(
                Manifest::read(with("name", name.clone())).unwrap_err(),
                INVALID_MANIFEST,
                "{name}"
            );
        }
        assert!(Manifest::read(with("name", json!("x".repeat(100)))).is_ok());
        assert!(Manifest::read(with("name", json!("0_a-b"))).is_ok());
        assert!(Manifest::read(with("displayName", json!(3))).is_err());
    }

    #[test]
    fn versions_are_checked() {
        for version in ["1.2", "1.2.3.4", "v1.2.3", "1.2.3-", "1..3", "1.2.3 "] {
            assert!(
                Manifest::read(with("version", json!(version))).is_err(),
                "{version}"
            );
        }
        for version in [
            "0.0.1",
            "10.20.30",
            "1.2.3-beta.1",
            "1.2.3+build_5",
            "1.2.3-rc-2",
        ] {
            assert!(
                Manifest::read(with("version", json!(version))).is_ok(),
                "{version}"
            );
        }
    }

    #[test]
    fn only_downloads_or_link_readers_install() {
        let refused = "Only download-capable or playlist-link Morneye addons can be installed";
        assert_eq!(
            Manifest::read(with("type", json!(["metadata_provider"]))).unwrap_err(),
            refused
        );
        assert_eq!(
            Manifest::read(with("type", json!("download_provider"))).unwrap_err(),
            refused
        );
        let mut reader = with("type", json!(["metadata_provider"]));
        reader["urlHandler"] = json!({ "enabled": true, "patterns": ["example.com"] });
        let manifest = Manifest::read(reader).unwrap();
        assert!(!manifest.downloads());
        assert!(manifest.handles_links());
    }

    #[test]
    fn the_compatibility_version_is_checked() {
        assert_eq!(
            Manifest::read(with("minAppVersion", json!("5.3.1"))).unwrap_err(),
            "This addon requires Morneye compatibility 5.3.1"
        );
        assert!(Manifest::read(with("minAppVersion", json!("5.3.0"))).is_ok());
        assert!(Manifest::read(with("minAppVersion", json!("4.9.1"))).is_ok());
        assert!(Manifest::read(with("minAppVersion", json!(6))).is_err());
    }

    #[test]
    fn runtime_features_are_checked() {
        let ok = json!([
            "signedSession@1",
            "signedSession@3",
            "sessionGrant@1",
            "preparedContext@1",
            "downloadSegments@1",
            "patternedFileTransform@1",
            "deferredPreview@1"
        ]);
        let mut signed = with("requiredRuntimeFeatures", ok);
        signed["signedSession"] = json!({ "baseUrl": "https://api.example.com/v2" });
        assert!(Manifest::read(signed).is_ok());
        for feature in [
            "signedSession@4",
            "teleport@1",
            "signedSession",
            "@1",
            "sessionGrant@x",
        ] {
            assert_eq!(
                Manifest::read(with("requiredRuntimeFeatures", json!([feature]))).unwrap_err(),
                format!("Unsupported runtime feature: {feature}")
            );
        }
    }

    #[test]
    fn network_permissions_are_checked() {
        let error = "Invalid addon network permissions";
        assert_eq!(
            Manifest::read(with("permissions", json!({ "network": ["*"] }))).unwrap_err(),
            error
        );
        assert_eq!(
            Manifest::read(with("permissions", json!({ "storage": true }))).unwrap_err(),
            error
        );
        assert_eq!(
            Manifest::read(with("permissions", json!(null))).unwrap_err(),
            error
        );
    }

    #[test]
    fn a_signed_session_needs_its_host_and_storage() {
        let mut manifest = with(
            "signedSession",
            json!({ "baseUrl": "https://gateway.example.org/v2" }),
        );
        assert!(
            Manifest::read(manifest.clone())
                .unwrap_err()
                .contains("gateway.example.org")
        );
        manifest["signedSession"] = json!({ "baseUrl": "https://api.example.com/v2" });
        manifest["permissions"]["storage"] = json!(false);
        assert_eq!(
            Manifest::read(manifest).unwrap_err(),
            "Signed sessions require storage permission"
        );
    }

    #[test]
    fn a_manifest_may_not_be_too_large() {
        let settings: Vec<Value> = (0..101)
            .map(|i| json!({ "key": format!("k{i}"), "type": "string" }))
            .collect();
        assert_eq!(
            Manifest::read(with("settings", json!(settings))).unwrap_err(),
            "Addon manifest is too large"
        );
        let options: Vec<Value> = (0..31).map(|i| json!({ "id": format!("q{i}") })).collect();
        assert!(Manifest::read(with("qualityOptions", json!(options))).is_err());
    }

    /// Shaped like the official signed-session add-ons (hosts swapped for example ones).
    #[test]
    fn a_manifest_shaped_like_the_official_ones_is_read() {
        let manifest = json!({
            "name": "tidal-web",
            "displayName": "Tidal",
            "version": "1.2.14",
            "type": ["metadata_provider", "download_provider"],
            "minAppVersion": "4.9.0",
            "requiredRuntimeFeatures": ["signedSession@3", "sessionGrant@1", "downloadSegments@1"],
            "signedSession": {
                "namespace": "shared-v2",
                "baseUrl": "https://api.example.com/v2",
                "appVersion": "tidal-web@1.2.5",
                "platform": "extension",
                "endpoints": { "bootstrap": "/bootstrap", "refresh": "/session/refresh" }
            },
            "permissions": {
                "network": ["tidal.example.com", "api.example.com", "*.audio.example.com"],
                "storage": true,
                "file": true
            },
            "qualityOptions": [
                { "id": "DOLBY_ATMOS", "label": "Dolby Atmos", "kind": "spatial" },
                { "id": "HI_RES_LOSSLESS", "label": "HiRes FLAC", "kind": "lossless" }
            ],
            "capabilities": { "downloadTransfer": { "maxAttempts": 4, "maxParallelSegments": 4 } },
            "urlHandler": { "enabled": true, "patterns": ["tidal.example.com", "tidal:"] },
            "settings": [{ "key": "countryCode", "label": "Country Code", "type": "string", "default": "US" }]
        });
        let manifest = Manifest::read(manifest).unwrap();
        assert!(manifest.signed_session.is_some());
        assert_eq!(manifest.quality_options.len(), 2);
        assert_eq!(manifest.download_transfer()["maxAttempts"], 4);
        assert_eq!(manifest.provider(), "tidal");
    }

    #[test]
    fn keys_are_lokals() {
        assert_eq!(key_of("deezer"), "23df0a49ce");
        assert_eq!(key_of("qobuz-web"), "32e75aaa94");
        assert_eq!(key_of("tidal-web"), "ff9e6812d4");
        assert_eq!(key_of("amazon").len(), 10);
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(compare_versions("1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("0", "0.0.1"), Ordering::Less);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("2", "1.9.9"), Ordering::Greater);
        // A part that is not a number counts 0.
        assert_eq!(compare_versions("1.2.3-beta", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("", "0.0.0"), Ordering::Equal);
        assert_eq!(
            compare_versions(&version_text(None), crate::COMPAT_VERSION),
            Ordering::Less
        );
    }

    #[test]
    fn package_names() {
        assert!(is_package_name("Qobuz.SFLX"));
        assert!(is_package_name("a.morneye-ext"));
        assert!(is_package_name("a.spotiflac-ext"));
        assert!(!is_package_name("a.zip"));
    }

    fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn manifest_bytes() -> Vec<u8> {
        minimal().to_string().into_bytes()
    }

    const CODE: &[u8] = b"registerExtension({ download() { return { success: false } } })";

    #[test]
    fn a_package_is_read() {
        let manifest = manifest_bytes();
        let bytes = zip(&[
            ("manifest.json", &manifest),
            ("index.js", CODE),
            ("lib/extra.txt", b"x"),
        ]);
        let package = Package::read(&bytes).unwrap();
        assert_eq!(package.manifest.name, "demo-addon");
        assert_eq!(package.files.len(), 3);
        assert_eq!(package.code().as_bytes(), CODE);
    }

    #[test]
    fn unsafe_paths_are_refused() {
        let manifest = manifest_bytes();
        for name in [
            "../evil.js",
            "/abs.js",
            "C:/x.js",
            "a\\b.js",
            "./x.js",
            "a/./b.js",
            "a/../b.js",
        ] {
            let bytes = zip(&[
                ("manifest.json", &manifest),
                ("index.js", CODE),
                (name, b"x"),
            ]);
            assert_eq!(Package::read(&bytes).err().unwrap(), UNSAFE_PATH, "{name}");
        }
    }

    #[test]
    fn links_are_refused() {
        let manifest = manifest_bytes();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).unwrap();
        writer.write_all(&manifest).unwrap();
        writer.start_file("index.js", options).unwrap();
        writer.write_all(CODE).unwrap();
        writer
            .add_symlink("link.js", "index.js", options.system(zip::System::Unix))
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert_eq!(Package::read(&bytes).err().unwrap(), UNSAFE_PATH);
    }

    #[test]
    fn missing_files_are_refused() {
        let manifest = manifest_bytes();
        let needs = "A Morneye package needs manifest.json and index.js at its root";
        assert_eq!(
            Package::read(&zip(&[("manifest.json", &manifest)]))
                .err()
                .unwrap(),
            needs
        );
        assert_eq!(
            Package::read(&zip(&[("index.js", CODE)])).err().unwrap(),
            needs
        );
        // Not at the root.
        let nested = zip(&[("pkg/manifest.json", &manifest), ("pkg/index.js", CODE)]);
        assert_eq!(Package::read(&nested).err().unwrap(), needs);
    }

    #[test]
    fn other_bytes_are_not_a_package() {
        assert_eq!(Package::read(b"not a zip").err().unwrap(), NOT_A_PACKAGE);
        let big = vec![0u8; MAX_PACKAGE + 1];
        assert_eq!(
            Package::read(&big).err().unwrap(),
            "Addon package exceeds 32 MB"
        );
    }

    #[test]
    fn too_many_files_are_refused() {
        let names: Vec<String> = (0..101).map(|i| format!("f{i}.txt")).collect();
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
        assert_eq!(
            Package::read(&zip(&entries)).err().unwrap(),
            "Addon package contains too many files"
        );
    }

    #[test]
    fn a_bad_manifest_or_a_large_script_is_refused() {
        let bytes = zip(&[("manifest.json", b"{ nope"), ("index.js", CODE)]);
        assert_eq!(Package::read(&bytes).err().unwrap(), INVALID_MANIFEST);
        let manifest = manifest_bytes();
        let script = vec![b'a'; MAX_SCRIPT + 1];
        let bytes = zip(&[("manifest.json", &manifest), ("index.js", &script)]);
        assert_eq!(
            Package::read(&bytes).err().unwrap(),
            "Addon script exceeds 8 MB"
        );
    }
}
