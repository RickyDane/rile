//! OneDrive backend (Microsoft Graph v1.0). The signed-in user's drive root
//! is `/`; Graph addresses items by path (`/me/drive/root:/a/b:`), so no id
//! cache is needed. Deleting moves items to the OneDrive recycle bin.

use std::io::Read;

use anyhow::{Result, bail};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use serde_json::json;

use crate::backend::Backend;
use crate::cloud::{self, Api};
use crate::{ConnectionInfo, RemoteEntry, paths};

const BASE: &str = "https://graph.microsoft.com/v1.0/me/drive";
const SELECT: &str = "id,name,size,lastModifiedDateTime,folder,file,package,remoteItem";
/// Files up to this size are uploaded in one request (Graph's limit is 250 MB,
/// but upload sessions are resumable and more reliable for larger files).
const SIMPLE_UPLOAD: u64 = 4 * 1024 * 1024;
/// Upload session fragment; must be a multiple of 320 KiB.
const UPLOAD_CHUNK: usize = 32 * 320 * 1024;

const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Item {
    name: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    last_modified_date_time: Option<String>,
    #[serde(default)]
    folder: Option<serde_json::Value>,
    #[serde(default)]
    remote_item: Option<RemoteItem>,
}

#[derive(Deserialize, Debug)]
struct RemoteItem {
    #[serde(default)]
    folder: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    value: Vec<Item>,
    #[serde(rename = "@odata.nextLink", default)]
    next: Option<String>,
}

impl Item {
    fn entry(&self, path: &str) -> RemoteEntry {
        let shared = self.remote_item.as_ref();
        RemoteEntry {
            name: paths::file_name(path).to_string(),
            path: path.to_string(),
            is_dir: self.folder.is_some() || shared.is_some_and(|r| r.folder.is_some()),
            // Items shared with the user and added to their drive.
            is_symlink: shared.is_some(),
            size: self.size.unwrap_or(0),
            modified: cloud::parse_time(self.last_modified_date_time.as_deref()),
            permissions: None,
        }
    }
}

/// `/a b/c` → `/a%20b/c` (each segment encoded, `/` kept).
fn encode_path(path: &str) -> String {
    path.split('/').map(|s| utf8_percent_encode(s, SEGMENT).to_string()).collect::<Vec<_>>().join("/")
}

/// Item address for `path`; append `/children`, `/content`, … with [`sub`].
fn item(path: &str) -> String {
    if path == "/" { format!("{BASE}/root") } else { format!("{BASE}/root:{}:", encode_path(path)) }
}

fn sub(path: &str, action: &str) -> String {
    format!("{}/{action}", item(path))
}

pub(crate) struct OneDriveBackend {
    api: Api,
}

impl OneDriveBackend {
    pub(crate) fn connect(info: &ConnectionInfo, secret: Option<&str>) -> Result<Self> {
        let api = Api::new(info, secret, true)?;
        // Checks the sign-in and that the account has a drive.
        let _: serde_json::Value = api.json("GET", &format!("{BASE}?$select=id"), None)?;
        Ok(Self { api })
    }

    fn delete(&mut self, path: &str) -> Result<()> {
        self.api.call("DELETE", &item(path), &[], None, true)?;
        Ok(())
    }

    fn upload_session(&self, path: &str, src: &mut dyn Read, size: u64) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Session {
            upload_url: String,
        }
        let session: Session = self.api.json(
            "POST",
            &sub(path, "createUploadSession"),
            Some(&json!({ "item": { "@microsoft.graph.conflictBehavior": "replace" } })),
        )?;
        let result = cloud::upload_chunks(src, size, UPLOAD_CHUNK, &mut |offset, bytes| {
            let range = format!("bytes {}-{}/{size}", offset, offset + bytes.len() as u64 - 1);
            // The upload URL is pre-authorized and must not get the bearer token.
            self.api.call("PUT", &session.upload_url, &[("Content-Range", &range)], Some((bytes, "application/octet-stream")), false)?;
            Ok(())
        });
        if result.is_err() {
            _ = self.api.call("DELETE", &session.upload_url, &[], None, false);
        }
        result
    }
}

impl Backend for OneDriveBackend {
    fn login_dir(&mut self) -> Result<String> {
        Ok("/".into())
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        let mut url = format!("{}?$top=1000&$select={SELECT}", sub(path, "children"));
        let mut out = Vec::new();
        loop {
            let page: Page = self.api.json("GET", &url, None)?;
            out.extend(page.value.iter().filter(|i| !i.name.contains('/')).map(|i| i.entry(&paths::join(path, &i.name))));
            match page.next {
                Some(next) => url = next,
                None => return Ok(out),
            }
        }
    }

    fn stat(&mut self, path: &str) -> Result<RemoteEntry> {
        let item: Item = self.api.json("GET", &format!("{}?$select={SELECT}", item(path)), None)?;
        Ok(item.entry(path))
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        let parent = paths::parent(path).unwrap_or_else(|| "/".into());
        let _: serde_json::Value = self.api.json(
            "POST",
            &sub(&parent, "children"),
            Some(&json!({ "name": paths::file_name(path), "folder": {}, "@microsoft.graph.conflictBehavior": "fail" })),
        )?;
        Ok(())
    }

    fn remove_file(&mut self, path: &str) -> Result<()> {
        self.delete(path)
    }

    fn remove_dir(&mut self, path: &str) -> Result<()> {
        self.delete(path)
    }

    fn remove_all(&mut self, path: &str) -> Option<Result<()>> {
        Some(self.delete(path))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let parent = paths::parent(to).unwrap_or_else(|| "/".into());
        let parent_ref = if parent == "/" { "/drive/root:".to_string() } else { format!("/drive/root:{}", encode_path(&parent)) };
        let _: serde_json::Value = self.api.json(
            "PATCH",
            &item(from),
            Some(&json!({ "name": paths::file_name(to), "parentReference": { "path": parent_ref } })),
        )?;
        Ok(())
    }

    fn read_file(&mut self, path: &str, sink: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<()> {
        // Answers with a redirect to a pre-authenticated download URL.
        let resp = self.api.call("GET", &sub(path, "content"), &[], None, true)?;
        cloud::stream(resp, sink)
    }

    fn write_file(&mut self, path: &str, src: &mut dyn Read, size: u64) -> Result<()> {
        if path == "/" {
            bail!("can't write to the drive root");
        }
        if size > SIMPLE_UPLOAD {
            return self.upload_session(path, src, size);
        }
        let mut bytes = vec![0u8; size as usize];
        if cloud::read_full(src, &mut bytes)? != bytes.len() {
            bail!("the file changed while it was being uploaded");
        }
        let url = format!("{}?@microsoft.graph.conflictBehavior=replace", sub(path, "content"));
        self.api.call("PUT", &url, &[], Some((&bytes, "application/octet-stream")), true)?;
        Ok(())
    }

    fn is_alive(&self) -> bool {
        true
    }

    fn atomic_writes(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_items_by_path() {
        assert_eq!(item("/"), format!("{BASE}/root"));
        assert_eq!(sub("/", "children"), format!("{BASE}/root/children"));
        assert_eq!(item("/My Files/a#b%c?.txt"), format!("{BASE}/root:/My%20Files/a%23b%25c%3F.txt:"));
        assert_eq!(sub("/x", "content"), format!("{BASE}/root:/x:/content"));
        assert_eq!(encode_path("/ü:"), "/%C3%BC%3A");
    }

    #[test]
    fn entries_from_items() {
        let item: Item = serde_json::from_str(r#"{"name":"Docs","size":10,"folder":{"childCount":1},"lastModifiedDateTime":"2024-05-01T12:00:00Z"}"#).unwrap();
        let e = item.entry("/Docs");
        assert!(e.is_dir && !e.is_symlink);
        assert_eq!((e.size, e.name.as_str()), (10, "Docs"));
        let shared: Item = serde_json::from_str(r#"{"name":"Team","remoteItem":{"folder":{}}}"#).unwrap();
        let e = shared.entry("/Team");
        assert!(e.is_dir && e.is_symlink);
    }
}
