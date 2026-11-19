//! Dropbox (API v2): `list_folder` and its cursor for the scan, `get_temporary_link` (four hours) to play a file.

use std::time::Duration;

use serde_json::{Value, json};

use super::{
    ANSWER_CAP, Change, CloudError, Drive, Entry, File, Kind, Link, Me, Page, Root, TokenFn,
    unreached,
};

const API: &str = "https://api.dropboxapi.com/2";
/// Dropbox's links live four hours; one is used for less.
const LINK_GOOD_FOR: Duration = Duration::from_secs(210 * 60);

pub struct Dropbox {
    agent: ureq::Agent,
    token: TokenFn,
}

impl Dropbox {
    pub fn new(agent: ureq::Agent, token: TokenFn) -> Self {
        Self { agent, token }
    }

    /// Calls the API's `path` with `body`; a refused token is renewed once.
    fn call(&self, path: &str, body: Value) -> Result<Value, CloudError> {
        let mut fresh = false;
        loop {
            let token = (self.token)(fresh)?;
            let mut response = self
                .agent
                .post(format!("{API}{path}"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .send(body.to_string())
                .map_err(|e| unreached("Dropbox", e))?;
            let status = response.status().as_u16();
            let text = response
                .body_mut()
                .with_config()
                .limit(ANSWER_CAP)
                .read_to_string()
                .unwrap_or_default();
            if status == 401 && !fresh {
                fresh = true;
                continue;
            }
            if status == 409 {
                // The endpoint's own error: what it says is in `error_summary` ("path/not_found/..", "reset/..").
                let summary = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.get("error_summary")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                return Err(if summary.starts_with("reset") {
                    CloudError::CursorGone
                } else if summary.contains("not_found") {
                    CloudError::Gone
                } else {
                    CloudError::Failed(format!(
                        "Dropbox refused it ({})",
                        summary.trim_end_matches('/')
                    ))
                });
            }
            if !(200..300).contains(&status) {
                return Err(super::trouble(status, "Dropbox"));
            }
            return serde_json::from_str(&text)
                .map_err(|_| CloudError::Failed("Dropbox's answer could not be read".into()));
        }
    }
}

fn text<'v>(v: &'v Value, name: &str) -> Option<&'v str> {
    v.get(name).and_then(Value::as_str)
}

/// The folders between the picked folder (`root`, lower case) and the file at `path_lower`, as the file's
/// `path_display` spells them. None when the file is not below the root as it was picked (it was moved since).
pub(crate) fn folders_below(
    root: &str,
    path_lower: &str,
    path_display: &str,
) -> Option<Vec<String>> {
    let root = root.trim_end_matches('/');
    let rest_lower = path_lower.strip_prefix(root)?.strip_prefix('/')?;
    // The display path has the same shape as the lower-case one.
    let rest = path_display.get(path_display.len().checked_sub(rest_lower.len())?..)?;
    let mut parts: Vec<String> = rest.split('/').map(str::to_string).collect();
    parts.pop(); // the file's own name
    Some(parts)
}

/// One entry of a `list_folder` page as a change.
pub(crate) fn change(root: &Root, entry: &Value) -> Option<Change> {
    let path_lower = text(entry, "path_lower").map(str::to_string);
    match text(entry, ".tag")? {
        "deleted" => Some(Change::Removed {
            id: None,
            path_lower,
        }),
        "folder" => Some(Change::Folder {
            id: text(entry, "id")?.to_string(),
            name: text(entry, "name")?.to_string(),
            parent: None,
        }),
        "file" => {
            let display = text(entry, "path_display").unwrap_or_default();
            Some(Change::File(File {
                id: text(entry, "id")?.to_string(),
                name: text(entry, "name")?.to_string(),
                size: entry.get("size").and_then(Value::as_u64).unwrap_or(0),
                version: text(entry, "rev")
                    .or_else(|| text(entry, "content_hash"))
                    .map(str::to_string),
                parent: None,
                folders: path_lower
                    .as_deref()
                    .and_then(|p| folders_below(&root.path_lower, p, display)),
                path_lower,
                link: None,
            }))
        }
        _ => None,
    }
}

fn page_of(root: &Root, answer: &Value) -> Result<Page, CloudError> {
    let unreadable = || CloudError::Failed("Dropbox's list could not be read".into());
    let entries = answer
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(unreadable)?;
    Ok(Page {
        changes: entries.iter().filter_map(|e| change(root, e)).collect(),
        cursor: text(answer, "cursor").ok_or_else(unreadable)?.to_string(),
        more: answer
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

impl Drive for Dropbox {
    fn kind(&self) -> Kind {
        Kind::Dropbox
    }

    fn me(&self) -> Result<Me, CloudError> {
        let me = self.call("/users/get_current_account", Value::Null)?;
        Ok(Me {
            id: text(&me, "account_id")
                .ok_or_else(|| CloudError::Failed("Dropbox did not say who signed in".into()))?
                .to_string(),
            name: me
                .get("name")
                .and_then(|n| text(n, "display_name"))
                .unwrap_or("Dropbox")
                .to_string(),
            email: text(&me, "email").map(str::to_string),
        })
    }

    fn browse(&self, folder: Option<&str>) -> Result<Vec<Entry>, CloudError> {
        let mut out = Vec::new();
        let mut answer = self.call(
            "/files/list_folder",
            json!({ "path": folder.unwrap_or(""), "recursive": false, "limit": 2000 }),
        )?;
        loop {
            for e in answer
                .get("entries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let tag = text(e, ".tag").unwrap_or_default();
                if tag != "folder" && tag != "file" {
                    continue;
                }
                if let (Some(id), Some(name)) = (text(e, "id"), text(e, "name")) {
                    out.push(Entry {
                        id: id.to_string(),
                        name: name.to_string(),
                        folder: tag == "folder",
                        path: text(e, "path_display").unwrap_or(name).to_string(),
                    });
                }
            }
            if !answer
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                break;
            }
            let Some(cursor) = text(&answer, "cursor").map(str::to_string) else {
                break;
            };
            answer = self.call("/files/list_folder/continue", json!({ "cursor": cursor }))?;
        }
        Ok(out)
    }

    fn path_of(&self, folder: &str) -> Result<String, CloudError> {
        if folder.is_empty() {
            return Ok(String::new());
        }
        let meta = self.call("/files/get_metadata", json!({ "path": folder }))?;
        Ok(text(&meta, "path_lower").unwrap_or_default().to_string())
    }

    fn page(&self, root: &Root, cursor: Option<&str>) -> Result<Page, CloudError> {
        let answer = match cursor {
            None => self.call(
                "/files/list_folder",
                json!({ "path": root.id, "recursive": true, "limit": 2000, "include_deleted": false }),
            )?,
            Some(cursor) => self.call("/files/list_folder/continue", json!({ "cursor": cursor }))?,
        };
        page_of(root, &answer)
    }

    fn link(&self, item: &str) -> Result<Link, CloudError> {
        let answer = self.call("/files/get_temporary_link", json!({ "path": item }))?;
        let url = text(&answer, "link")
            .filter(|l| l.starts_with("https://"))
            .ok_or_else(|| CloudError::Failed("Dropbox gave no link to the file".into()))?;
        Ok(Link {
            url: url.to_string(),
            good_for: LINK_GOOD_FOR,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_of_dropbox_changes_is_read_with_each_files_folders() {
        let root = Root {
            id: "id:root".into(),
            path_lower: "/music".into(),
        };
        let answer = json!({
            "entries": [
                { ".tag": "folder", "id": "id:f1", "name": "Rips", "path_lower": "/music/rips", "path_display": "/Music/Rips" },
                { ".tag": "file", "id": "id:a", "name": "01 Song.flac", "path_lower": "/music/rips/artist/album/01 song.flac",
                  "path_display": "/Music/Rips/Artist/Album/01 Song.flac", "size": 12345, "rev": "015f" },
                { ".tag": "file", "id": "id:b", "name": "Top.mp3", "path_lower": "/music/top.mp3", "path_display": "/Music/Top.mp3", "size": 9 },
                { ".tag": "deleted", "name": "old", "path_lower": "/music/old" }
            ],
            "cursor": "AAA", "has_more": true
        });
        let page = page_of(&root, &answer).unwrap();
        assert!(page.more);
        assert_eq!(page.cursor, "AAA");
        assert_eq!(page.changes.len(), 4);
        let Change::File(song) = &page.changes[1] else {
            panic!("a file");
        };
        assert_eq!(song.id, "id:a");
        assert_eq!(song.size, 12345);
        assert_eq!(song.version.as_deref(), Some("015f"));
        assert_eq!(
            song.folders,
            Some(vec!["Rips".to_string(), "Artist".into(), "Album".into()])
        );
        let Change::File(top) = &page.changes[2] else {
            panic!("a file");
        };
        assert_eq!(top.folders, Some(Vec::new()));
        assert_eq!(
            page.changes[3],
            Change::Removed {
                id: None,
                path_lower: Some("/music/old".into())
            }
        );
        // A file no longer below the folder as it was picked has no folders known.
        assert_eq!(
            folders_below("/music", "/elsewhere/a.mp3", "/Elsewhere/a.mp3"),
            None
        );
        // The whole Dropbox picked.
        assert_eq!(
            folders_below("", "/a/b.mp3", "/A/b.mp3"),
            Some(vec!["A".to_string()])
        );
    }
}
