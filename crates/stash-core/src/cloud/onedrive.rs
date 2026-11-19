//! OneDrive (Microsoft Graph): `delta` and its links for the scan, an item's `@microsoft.graph.downloadUrl` to play a
//! file.

use std::time::Duration;

use serde_json::Value;

use super::{
    ANSWER_CAP, Change, CloudError, Drive, Entry, File, Kind, Link, Me, Page, Root, TokenFn,
    unreached,
};

const GRAPH: &str = "https://graph.microsoft.com/v1.0";
/// A download address is good for a short while; one is used for less.
const LINK_GOOD_FOR: Duration = Duration::from_secs(10 * 60);
const DOWNLOAD_URL: &str = "@microsoft.graph.downloadUrl";

pub struct OneDrive {
    agent: ureq::Agent,
    token: TokenFn,
}

/// An item's id in an address (a path segment).
fn item(id: &str) -> String {
    id.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'!' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Whether `url` is Graph's own: the addresses Graph names for the next page are followed, and only those.
fn is_graph(url: &str) -> bool {
    url::Url::parse(url)
        .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("graph.microsoft.com"))
}

fn text<'v>(v: &'v Value, name: &str) -> Option<&'v str> {
    v.get(name).and_then(Value::as_str)
}

/// Where an item is, from its parent's path ("/drive/root:/Music") and its name: "/Music/Name".
fn display_path(entry: &Value, name: &str) -> String {
    let parent = entry
        .get("parentReference")
        .and_then(|p| text(p, "path"))
        .and_then(|p| p.split_once(':').map(|(_, rest)| rest.to_string()))
        .unwrap_or_default();
    format!("{}/{name}", parent.trim_end_matches('/'))
}

impl OneDrive {
    pub fn new(agent: ureq::Agent, token: TokenFn) -> Self {
        Self { agent, token }
    }

    /// GETs `url` (Graph's); a refused token is renewed once.
    fn get(&self, url: &str) -> Result<Value, CloudError> {
        if !is_graph(url) {
            return Err(CloudError::Failed(
                "OneDrive named an address that is not its own".into(),
            ));
        }
        let mut fresh = false;
        loop {
            let token = (self.token)(fresh)?;
            let mut response = self
                .agent
                .get(url)
                .header("Authorization", format!("Bearer {token}"))
                .call()
                .map_err(|e| unreached("OneDrive", e))?;
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
            // A delta link too old to take (`resyncRequired`).
            if status == 410 {
                return Err(CloudError::CursorGone);
            }
            if !(200..300).contains(&status) {
                return Err(super::trouble(status, "OneDrive"));
            }
            return serde_json::from_str(&text)
                .map_err(|_| CloudError::Failed("OneDrive's answer could not be read".into()));
        }
    }
}

/// One item of a `delta` page as a change.
pub(crate) fn change(entry: &Value) -> Option<Change> {
    let id = text(entry, "id")?.to_string();
    if entry.get("deleted").is_some() {
        return Some(Change::Removed {
            id: Some(id),
            path_lower: None,
        });
    }
    let name = text(entry, "name").unwrap_or_default().to_string();
    let parent = entry
        .get("parentReference")
        .and_then(|p| text(p, "id"))
        .map(str::to_string);
    if entry.get("folder").is_some() || entry.get("root").is_some() {
        return Some(Change::Folder { id, name, parent });
    }
    entry.get("file")?;
    Some(Change::File(File {
        id,
        name,
        size: entry.get("size").and_then(Value::as_u64).unwrap_or(0),
        version: text(entry, "cTag")
            .or_else(|| text(entry, "eTag"))
            .map(str::to_string),
        parent,
        folders: None,
        path_lower: None,
        link: text(entry, DOWNLOAD_URL)
            .filter(|l| l.starts_with("https://"))
            .map(str::to_string),
    }))
}

pub(crate) fn page_of(answer: &Value) -> Result<Page, CloudError> {
    let changes = answer
        .get("value")
        .and_then(Value::as_array)
        .ok_or_else(|| CloudError::Failed("OneDrive's list could not be read".into()))?
        .iter()
        .filter_map(change)
        .collect();
    let (cursor, more) = match (
        text(answer, "@odata.nextLink"),
        text(answer, "@odata.deltaLink"),
    ) {
        (Some(next), _) => (next, true),
        (None, Some(delta)) => (delta, false),
        (None, None) => {
            return Err(CloudError::Failed(
                "OneDrive's list did not say where it goes on".into(),
            ));
        }
    };
    if !is_graph(cursor) {
        return Err(CloudError::Failed(
            "OneDrive named an address that is not its own".into(),
        ));
    }
    Ok(Page {
        changes,
        cursor: cursor.to_string(),
        more,
    })
}

impl Drive for OneDrive {
    fn kind(&self) -> Kind {
        Kind::OneDrive
    }

    fn me(&self) -> Result<Me, CloudError> {
        let me = self.get(&format!(
            "{GRAPH}/me?$select=id,displayName,mail,userPrincipalName"
        ))?;
        Ok(Me {
            id: text(&me, "id")
                .ok_or_else(|| CloudError::Failed("OneDrive did not say who signed in".into()))?
                .to_string(),
            name: text(&me, "displayName").unwrap_or("OneDrive").to_string(),
            email: text(&me, "mail")
                .or_else(|| text(&me, "userPrincipalName"))
                .map(str::to_string),
        })
    }

    fn browse(&self, folder: Option<&str>) -> Result<Vec<Entry>, CloudError> {
        let first = match folder {
            None => format!("{GRAPH}/me/drive/root/children?$top=200"),
            Some(id) => format!("{GRAPH}/me/drive/items/{}/children?$top=200", item(id)),
        };
        let mut out = Vec::new();
        let mut next = Some(first);
        while let Some(url) = next.take() {
            let answer = self.get(&url)?;
            for e in answer
                .get("value")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let (Some(id), Some(name)) = (text(e, "id"), text(e, "name")) else {
                    continue;
                };
                let is_folder = e.get("folder").is_some();
                if !is_folder && e.get("file").is_none() {
                    continue;
                }
                out.push(Entry {
                    id: id.to_string(),
                    name: name.to_string(),
                    folder: is_folder,
                    path: display_path(e, name),
                });
            }
            next = text(&answer, "@odata.nextLink").map(str::to_string);
        }
        Ok(out)
    }

    fn path_of(&self, folder: &str) -> Result<String, CloudError> {
        let meta = self.get(&format!("{GRAPH}/me/drive/items/{}", item(folder)))?;
        let name = text(&meta, "name").unwrap_or_default();
        Ok(if meta.get("root").is_some() {
            String::new()
        } else {
            display_path(&meta, name).to_lowercase()
        })
    }

    fn page(&self, root: &Root, cursor: Option<&str>) -> Result<Page, CloudError> {
        let url = match cursor {
            Some(cursor) => cursor.to_string(),
            None if root.id.is_empty() => format!("{GRAPH}/me/drive/root/delta"),
            None => format!("{GRAPH}/me/drive/items/{}/delta", item(&root.id)),
        };
        page_of(&self.get(&url)?)
    }

    fn link(&self, id: &str) -> Result<Link, CloudError> {
        let meta = self.get(&format!("{GRAPH}/me/drive/items/{}", item(id)))?;
        let url = text(&meta, DOWNLOAD_URL)
            .filter(|l| l.starts_with("https://"))
            .ok_or_else(|| CloudError::Failed("OneDrive gave no link to the file".into()))?;
        Ok(Link {
            url: url.to_string(),
            good_for: LINK_GOOD_FOR,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_page_of_onedrive_changes_is_read_and_goes_on_only_at_graph() {
        let answer = json!({
            "value": [
                { "id": "R!1", "name": "root", "root": {}, "folder": { "childCount": 1 } },
                { "id": "F!2", "name": "Album", "folder": {}, "parentReference": { "id": "R!1" } },
                { "id": "S!3", "name": "01.flac", "file": { "mimeType": "audio/flac" }, "size": 99, "cTag": "c1",
                  "parentReference": { "id": "F!2" }, "@microsoft.graph.downloadUrl": "https://public.dm.files.1drv.com/x" },
                { "id": "S!4", "deleted": { "state": "deleted" } }
            ],
            "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/drive/root/delta?token=abc"
        });
        let page = page_of(&answer).unwrap();
        assert!(!page.more);
        assert_eq!(page.changes.len(), 4);
        let Change::File(song) = &page.changes[2] else {
            panic!("a file");
        };
        assert_eq!(song.parent.as_deref(), Some("F!2"));
        assert_eq!(song.version.as_deref(), Some("c1"));
        assert_eq!(
            song.link.as_deref(),
            Some("https://public.dm.files.1drv.com/x")
        );
        assert_eq!(
            page.changes[3],
            Change::Removed {
                id: Some("S!4".into()),
                path_lower: None
            }
        );
        let elsewhere = json!({ "value": [], "@odata.nextLink": "https://evil.example/next" });
        assert!(page_of(&elsewhere).is_err());
        assert_eq!(
            display_path(
                &json!({ "parentReference": { "path": "/drive/root:/Music/Rips" } }),
                "A"
            ),
            "/Music/Rips/A"
        );
        assert_eq!(
            display_path(
                &json!({ "parentReference": { "path": "/drive/root:" } }),
                "Music"
            ),
            "/Music"
        );
        assert_eq!(item("D46!549 27/x"), "D46!549%2027%2Fx");
    }
}
