//! What add-ons keep (spec §5; Lokal `morneye_storage.dart`): `storage` as plain JSON, everything else (an add-on's
//! `credentials`, `settings`, `connection`, a signed `session`, its `ratelimit`) sealed with AES-256-GCM under a key
//! drawn from the core's master key for that record alone.
//!
//! `<root>/<namespace>/storage.json` and `<root>/<namespace>/<purpose>.enc`; the namespace is the add-on's key (or a
//! signed session's), matching `^[a-f0-9]{10,64}$`, the purpose `^[a-z]+$`. A record is at most 5 MiB (`Addon
//! storage exceeds 5 MB`); writes are whole (a temporary file renamed). A sealed record that cannot be opened reads as
//! empty.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::{Map, Value};

/// The purpose kept as plain JSON.
const PLAIN: &str = "storage";
/// The most one record may hold.
const LIMIT: usize = 5 * 1024 * 1024;

pub struct Store {
    root: PathBuf,
    master: [u8; 32],
    /// Writes (and read-change-write updates) one at a time, so two runtimes of an add-on never lose a change.
    writing: Mutex<()>,
}

impl Store {
    /// The records under `root` (`<mornye>/data`), sealed with keys drawn from `master`.
    pub fn new(root: PathBuf, master: [u8; 32]) -> Store {
        Store {
            root,
            master,
            writing: Mutex::new(()),
        }
    }

    /// The record (`{}` when there is none, or it cannot be read).
    pub fn read(&self, namespace: &str, purpose: &str) -> Map<String, Value> {
        let Ok(path) = self.path(namespace, purpose) else {
            return Map::new();
        };
        let Ok(bytes) = fs::read(&path) else {
            return Map::new();
        };
        let plain = if purpose == PLAIN {
            bytes
        } else {
            match crate::crypto::open(&self.key(namespace, purpose), &bytes) {
                Ok(plain) => plain,
                Err(_) => return Map::new(),
            }
        };
        match serde_json::from_slice::<Value>(&plain) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        }
    }

    pub fn write(
        &self,
        namespace: &str,
        purpose: &str,
        value: &Map<String, Value>,
    ) -> Result<(), String> {
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        self.write_now(namespace, purpose, value)
    }

    /// Sets `key` (or removes it, for `None`) in the record.
    pub fn update(
        &self,
        namespace: &str,
        purpose: &str,
        key: &str,
        value: Option<Value>,
    ) -> Result<(), String> {
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        let mut record = self.read(namespace, purpose);
        match value {
            Some(value) => {
                record.insert(key.to_string(), value);
            }
            None => {
                record.remove(key);
            }
        }
        self.write_now(namespace, purpose, &record)
    }

    /// Everything kept for `namespace` (an add-on removed).
    pub fn remove(&self, namespace: &str) {
        if !valid_namespace(namespace) {
            return;
        }
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = fs::remove_dir_all(self.root.join(namespace));
    }

    /// Writes the record; the caller holds `writing`.
    fn write_now(
        &self,
        namespace: &str,
        purpose: &str,
        value: &Map<String, Value>,
    ) -> Result<(), String> {
        let path = self.path(namespace, purpose)?;
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        if bytes.len() > LIMIT {
            return Err("Addon storage exceeds 5 MB".to_string());
        }
        let data = if purpose == PLAIN {
            bytes
        } else {
            crate::crypto::seal(&self.key(namespace, purpose), &bytes)?
        };
        write_whole(&path, &data)
    }

    /// The record's file (store.dart:114-121).
    fn path(&self, namespace: &str, purpose: &str) -> Result<PathBuf, String> {
        if !valid_namespace(namespace) || !valid_purpose(purpose) {
            return Err("Invalid addon storage namespace".to_string());
        }
        let extension = if purpose == PLAIN { "json" } else { "enc" };
        Ok(self
            .root
            .join(namespace)
            .join(format!("{purpose}.{extension}")))
    }

    /// The record's own key: HMAC-SHA256(master, "Lokal Morneye storage v1\0<purpose>\0<namespace>").
    fn key(&self, namespace: &str, purpose: &str) -> [u8; 32] {
        let label = format!("Lokal Morneye storage v1\0{purpose}\0{namespace}");
        crate::crypto::hmac_sha256(&self.master, label.as_bytes())
    }
}

/// `^[a-f0-9]{10,64}$`.
fn valid_namespace(namespace: &str) -> bool {
    (10..=64).contains(&namespace.len())
        && namespace
            .bytes()
            .all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9'))
}

/// `^[a-z]+$`.
fn valid_purpose(purpose: &str) -> bool {
    !purpose.is_empty() && purpose.bytes().all(|b| b.is_ascii_lowercase())
}

/// Writes `data` to `path` through a temporary file beside it, so a crash never leaves half a record.
fn write_whole(path: &Path, data: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid addon storage namespace".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = parent.join(format!(
        "{name}.{}.tmp",
        crate::hex(&crate::random_bytes(8))
    ));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NAMESPACE: &str = "0123456789";

    /// A folder of its own, removed when the test ends.
    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Folder {
            let path = std::env::temp_dir().join(format!(
                "stash-mornye-store-{}",
                crate::hex(&crate::random_bytes(8))
            ));
            Folder(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn record(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn storage_is_plain_json() {
        let folder = Folder::new();
        let store = Store::new(folder.0.clone(), [7; 32]);
        store
            .write(NAMESPACE, "storage", &record(json!({"a": 1, "b": "two"})))
            .unwrap();
        let file = folder.0.join(NAMESPACE).join("storage.json");
        let text = fs::read_to_string(&file).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"a": 1, "b": "two"})
        );
        assert_eq!(
            store.read(NAMESPACE, "storage"),
            record(json!({"a": 1, "b": "two"}))
        );
    }

    #[test]
    fn other_purposes_are_sealed_with_their_own_key() {
        let folder = Folder::new();
        let store = Store::new(folder.0.clone(), [7; 32]);
        let secret = record(json!({"token": "a-secret-value"}));
        store.write(NAMESPACE, "credentials", &secret).unwrap();
        let file = folder.0.join(NAMESPACE).join("credentials.enc");
        let bytes = fs::read(&file).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("a-secret-value"));
        assert_eq!(store.read(NAMESPACE, "credentials"), secret);
        // The key is the record's own: another master, another purpose's key, or a moved file read as empty.
        assert!(
            Store::new(folder.0.clone(), [8; 32])
                .read(NAMESPACE, "credentials")
                .is_empty()
        );
        fs::copy(&file, folder.0.join(NAMESPACE).join("settings.enc")).unwrap();
        assert!(store.read(NAMESPACE, "settings").is_empty());
        let other = "abcdef0123";
        fs::create_dir_all(folder.0.join(other)).unwrap();
        fs::copy(&file, folder.0.join(other).join("credentials.enc")).unwrap();
        assert!(store.read(other, "credentials").is_empty());
        // A damaged record reads as empty too.
        fs::write(&file, b"not sealed").unwrap();
        assert!(store.read(NAMESPACE, "credentials").is_empty());
    }

    #[test]
    fn updates_set_and_remove_keys() {
        let folder = Folder::new();
        let store = Store::new(folder.0.clone(), [1; 32]);
        store
            .update(NAMESPACE, "settings", "a", Some(json!(1)))
            .unwrap();
        store
            .update(NAMESPACE, "settings", "b", Some(json!(null)))
            .unwrap();
        assert_eq!(
            store.read(NAMESPACE, "settings"),
            record(json!({"a": 1, "b": null}))
        );
        store.update(NAMESPACE, "settings", "a", None).unwrap();
        assert_eq!(
            store.read(NAMESPACE, "settings"),
            record(json!({"b": null}))
        );
        // No temporary file is left behind.
        let names: Vec<String> = fs::read_dir(folder.0.join(NAMESPACE))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["settings.enc".to_string()]);
    }

    #[test]
    fn names_are_checked() {
        let folder = Folder::new();
        let store = Store::new(folder.0.clone(), [1; 32]);
        let empty = Map::new();
        for (namespace, purpose) in [
            ("012345678", "storage"),
            ("0123456789ABCDEF", "storage"),
            ("../../etc12", "storage"),
            (NAMESPACE, "Storage"),
            (NAMESPACE, "set-tings"),
            (NAMESPACE, ""),
            (&"a".repeat(65)[..], "storage"),
        ] {
            assert_eq!(
                store.write(namespace, purpose, &empty),
                Err("Invalid addon storage namespace".to_string()),
                "{namespace} {purpose}"
            );
            assert!(store.read(namespace, purpose).is_empty());
        }
        assert!(store.write(&"a".repeat(64), "storage", &empty).is_ok());
    }

    #[test]
    fn records_are_at_most_5_mib() {
        let folder = Folder::new();
        let store = Store::new(folder.0.clone(), [1; 32]);
        let big = record(json!({ "x": "y".repeat(LIMIT) }));
        assert_eq!(
            store.write(NAMESPACE, "storage", &big),
            Err("Addon storage exceeds 5 MB".to_string())
        );
        assert!(!folder.0.join(NAMESPACE).join("storage.json").exists());
    }

    #[test]
    fn removing_a_namespace_removes_only_it() {
        let folder = Folder::new();
        let store = Store::new(folder.0.clone(), [1; 32]);
        let one = record(json!({"a": 1}));
        store.write(NAMESPACE, "storage", &one).unwrap();
        store.write("abcdefabcd", "storage", &one).unwrap();
        store.remove("..");
        store.remove(NAMESPACE);
        assert!(!folder.0.join(NAMESPACE).exists());
        assert_eq!(store.read("abcdefabcd", "storage"), one);
    }
}
