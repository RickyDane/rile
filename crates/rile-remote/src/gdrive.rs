//! Google Drive (API v3) backend. "My Drive" is mapped to `/`.
//!
//! Drive addresses files by id and allows several files with the same name in
//! one folder, so paths are resolved through listings: each listing caches
//! `path → id` for its children, and duplicate names get a ` (2)` suffix.
//! Google Docs, Sheets, Slides and Drawings are shown with an Office/PDF
//! extension and exported on read; they can't be overwritten. Deleting moves
//! items to the Drive trash. Shortcuts appear as symlinks to their target.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::time::SystemTime;

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::json;

use crate::backend::Backend;
use crate::cloud::{self, Api, HttpError};
use crate::{ConnectionInfo, RemoteEntry, paths};

const API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3";
const FOLDER: &str = "application/vnd.google-apps.folder";
const SHORTCUT: &str = "application/vnd.google-apps.shortcut";
const FIELDS: &str = "id,name,mimeType,size,modifiedTime,createdTime,shortcutDetails(targetId,targetMimeType)";
/// Resumable upload chunk; must be a multiple of 256 KiB.
const UPLOAD_CHUNK: usize = 32 * 256 * 1024;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct File {
    id: String,
    name: String,
    mime_type: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    modified_time: Option<String>,
    #[serde(default)]
    created_time: Option<String>,
    #[serde(default)]
    shortcut_details: Option<ShortcutDetails>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct ShortcutDetails {
    target_id: String,
    #[serde(default)]
    target_mime_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList {
    #[serde(default)]
    files: Vec<File>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    /// The item itself (the shortcut, for shortcuts).
    id: String,
    /// Whose children or bytes are used (the target, for shortcuts).
    content: String,
    /// MIME type of `content`.
    mime: String,
    shortcut: bool,
    size: u64,
    modified: Option<SystemTime>,
}

impl Node {
    fn from_file(f: &File) -> Self {
        let (content, mime, shortcut) = match (&f.shortcut_details, f.mime_type.as_str()) {
            (Some(s), SHORTCUT) => (s.target_id.clone(), s.target_mime_type.clone().unwrap_or_default(), true),
            _ => (f.id.clone(), f.mime_type.clone(), false),
        };
        Node {
            id: f.id.clone(),
            content,
            mime,
            shortcut,
            size: f.size.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0),
            modified: cloud::parse_time(f.modified_time.as_deref()),
        }
    }

    fn is_dir(&self) -> bool {
        self.mime == FOLDER
    }

    fn entry(&self, path: &str) -> RemoteEntry {
        RemoteEntry {
            name: paths::file_name(path).to_string(),
            path: path.to_string(),
            is_dir: self.is_dir(),
            is_symlink: self.shortcut,
            size: self.size,
            modified: self.modified,
            permissions: None,
        }
    }
}

/// Export format (extension, MIME type) for Google's own document types.
fn export_format(mime: &str) -> Option<(&'static str, &'static str)> {
    Some(match mime {
        "application/vnd.google-apps.document" => (".docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "application/vnd.google-apps.spreadsheet" => (".xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "application/vnd.google-apps.presentation" => (".pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        "application/vnd.google-apps.drawing" => (".pdf", "application/pdf"),
        _ => return None,
    })
}

/// Google-native types without downloadable content (Forms, Sites, …) are hidden.
fn listable(node: &Node) -> bool {
    !node.mime.starts_with("application/vnd.google-apps.") || node.is_dir() || export_format(&node.mime).is_some()
}

/// Display names for a folder's children: export extensions added, `/`
/// replaced, duplicates suffixed (oldest file keeps the plain name).
fn display_names(mut files: Vec<File>) -> Vec<(String, Node)> {
    files.sort_by(|a, b| a.created_time.cmp(&b.created_time).then_with(|| a.id.cmp(&b.id)));
    let named: Vec<(String, Node)> = files
        .iter()
        .map(|f| (f, Node::from_file(f)))
        .filter(|(_, n)| listable(n))
        .map(|(f, n)| {
            let mut name = f.name.replace('/', "\u{2215}");
            if let Some((ext, _)) = export_format(&n.mime)
                && !name.to_lowercase().ends_with(ext)
            {
                name.push_str(ext);
            }
            if name.is_empty() || name == "." || name == ".." {
                name = format!("Untitled {}", n.id);
            }
            (name, n)
        })
        .collect();
    let all: HashSet<String> = named.iter().map(|(n, _)| n.clone()).collect();
    let mut used = HashSet::new();
    named
        .into_iter()
        .map(|(name, node)| {
            let mut unique = name.clone();
            let mut n = 2;
            while used.contains(&unique) || (unique != name && all.contains(&unique)) {
                unique = numbered(&name, n);
                n += 1;
            }
            used.insert(unique.clone());
            (unique, node)
        })
        .collect()
}

/// `report.pdf` → `report (2).pdf`.
fn numbered(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

fn not_found(path: &str) -> anyhow::Error {
    HttpError { status: 404, message: format!("{path} doesn't exist") }.into()
}

fn is_under(path: &str, root: &str) -> bool {
    path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/') || root == "/")
}

pub(crate) struct DriveBackend {
    api: Api,
    root: Node,
    nodes: HashMap<String, Node>,
}

impl DriveBackend {
    pub(crate) fn connect(info: &ConnectionInfo, secret: Option<&str>) -> Result<Self> {
        let api = Api::new(info, secret, false)?;
        let root: File = api.json("GET", &format!("{API}/files/root?fields={FIELDS}"), None)?;
        Ok(Self { api, root: Node::from_file(&root), nodes: HashMap::new() })
    }

    fn children(&self, folder: &str) -> Result<Vec<File>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut url = url::Url::parse(&format!("{API}/files")).expect("static url");
            {
                let mut q = url.query_pairs_mut();
                q.append_pair("q", &format!("'{}' in parents and trashed = false", folder.replace('\\', "\\\\").replace('\'', "\\'")))
                    .append_pair("fields", &format!("nextPageToken,files({FIELDS})"))
                    .append_pair("pageSize", "1000")
                    .append_pair("supportsAllDrives", "true")
                    .append_pair("includeItemsFromAllDrives", "true");
                if let Some(p) = &page {
                    q.append_pair("pageToken", p);
                }
            }
            let list: FileList = self.api.json("GET", url.as_str(), None)?;
            out.extend(list.files);
            match list.next_page_token {
                Some(p) if !p.is_empty() => page = Some(p),
                _ => return Ok(out),
            }
        }
    }

    /// List `path` (fresh from the API) and cache its children.
    fn load(&mut self, path: &str) -> Result<Vec<(String, Node)>> {
        let dir = self.resolve(path)?;
        if !dir.is_dir() {
            bail!("{path} is not a folder");
        }
        let children = display_names(self.children(&dir.content)?);
        self.nodes.retain(|k, _| paths::parent(k).as_deref() != Some(path));
        for (name, node) in &children {
            self.nodes.insert(paths::join(path, name), node.clone());
        }
        Ok(children)
    }

    fn resolve(&mut self, path: &str) -> Result<Node> {
        if path == "/" {
            return Ok(self.root.clone());
        }
        if let Some(node) = self.nodes.get(path) {
            return Ok(node.clone());
        }
        let parent = paths::parent(path).unwrap_or_else(|| "/".into());
        self.load(&parent)?;
        self.nodes.get(path).cloned().ok_or_else(|| not_found(path))
    }

    /// Run `op` on the node at `path`; if the cached id has gone stale (404),
    /// forget the cache and try once more.
    fn with_node<R>(&mut self, path: &str, mut op: impl FnMut(&mut Self, Node) -> Result<R>) -> Result<R> {
        let cached = path == "/" || self.nodes.contains_key(path);
        let node = self.resolve(path)?;
        match op(self, node) {
            Err(e) if cached && cloud::status_of(&e) == Some(404) => {
                self.nodes.clear();
                let node = self.resolve(path)?;
                op(self, node)
            }
            r => r,
        }
    }

    fn forget(&mut self, path: &str) {
        self.nodes.retain(|k, _| !is_under(k, path));
    }

    fn folder(&mut self, path: &str) -> Result<Node> {
        let node = self.resolve(path)?;
        if !node.is_dir() {
            bail!("{path} is not a folder");
        }
        Ok(node)
    }

    fn trash(&mut self, path: &str) -> Result<()> {
        self.with_node(path, |this, node| {
            let _: serde_json::Value = this.api.json("PATCH", &format!("{API}/files/{}?supportsAllDrives=true&fields=id", node.id), Some(&json!({ "trashed": true })))?;
            Ok(())
        })?;
        self.forget(path);
        Ok(())
    }

    /// Start a resumable upload; returns the session URL.
    fn start_upload(&self, existing: Option<&Node>, parent: &Node, name: &str, size: u64) -> Result<String> {
        let size = size.to_string();
        let headers = [("X-Upload-Content-Length", size.as_str())];
        let (method, url, meta) = match existing {
            Some(n) => ("PATCH", format!("{UPLOAD}/files/{}?uploadType=resumable&supportsAllDrives=true&fields={FIELDS}", n.id), json!({})),
            None => ("POST", format!("{UPLOAD}/files?uploadType=resumable&supportsAllDrives=true&fields={FIELDS}"), json!({ "name": name, "parents": [parent.content] })),
        };
        let body = meta.to_string();
        let resp = self.api.call(method, &url, &headers, Some((body.as_bytes(), "application/json; charset=UTF-8")), true)?;
        match resp.headers().get("location").and_then(|v| v.to_str().ok()) {
            Some(location) => Ok(location.to_string()),
            None => bail!("Google Drive didn't start the upload"),
        }
    }
}

impl Backend for DriveBackend {
    fn login_dir(&mut self) -> Result<String> {
        Ok("/".into())
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        let children = match self.load(path) {
            Err(e) if cloud::status_of(&e) == Some(404) && path != "/" => {
                self.nodes.clear();
                self.load(path)?
            }
            r => r?,
        };
        Ok(children.iter().map(|(name, node)| node.entry(&paths::join(path, name))).collect())
    }

    fn stat(&mut self, path: &str) -> Result<RemoteEntry> {
        Ok(self.resolve(path)?.entry(path))
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        let parent_path = paths::parent(path).unwrap_or_else(|| "/".into());
        let parent = self.folder(&parent_path)?;
        if self.resolve(path).is_ok() {
            bail!("{path} already exists");
        }
        let created: File = self.api.json(
            "POST",
            &format!("{API}/files?supportsAllDrives=true&fields={FIELDS}"),
            Some(&json!({ "name": paths::file_name(path), "mimeType": FOLDER, "parents": [parent.content] })),
        )?;
        self.nodes.insert(path.to_string(), Node::from_file(&created));
        Ok(())
    }

    fn remove_file(&mut self, path: &str) -> Result<()> {
        self.trash(path)
    }

    fn remove_dir(&mut self, path: &str) -> Result<()> {
        self.trash(path)
    }

    fn remove_all(&mut self, path: &str) -> Option<Result<()>> {
        Some(self.trash(path))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let node = self.resolve(from)?;
        let old_parent = self.folder(&paths::parent(from).unwrap_or_else(|| "/".into()))?;
        let new_parent = self.folder(&paths::parent(to).unwrap_or_else(|| "/".into()))?;
        if self.resolve(to).is_ok_and(|n| n.id != node.id) {
            bail!("{to} already exists");
        }
        let mut name = paths::file_name(to).to_string();
        if !node.shortcut
            && let Some((ext, _)) = export_format(&node.mime)
            && name.to_lowercase().ends_with(ext)
            && name.len() > ext.len()
        {
            name.truncate(name.len() - ext.len());
        }
        let mut url = format!("{API}/files/{}?supportsAllDrives=true&fields={FIELDS}", node.id);
        if old_parent.content != new_parent.content {
            url.push_str(&format!("&addParents={}&removeParents={}", new_parent.content, old_parent.content));
        }
        let updated: File = self.api.json("PATCH", &url, Some(&json!({ "name": name })))?;
        self.forget(from);
        self.nodes.insert(to.to_string(), Node::from_file(&updated));
        Ok(())
    }

    fn read_file(&mut self, path: &str, sink: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<()> {
        self.with_node(path, |this, node| {
            if node.is_dir() {
                bail!("{path} is a folder");
            }
            let url = match export_format(&node.mime) {
                Some((_, mime)) => format!("{API}/files/{}/export?mimeType={}", node.content, percent_encoding::utf8_percent_encode(mime, percent_encoding::NON_ALPHANUMERIC)),
                None => format!("{API}/files/{}?alt=media&supportsAllDrives=true", node.content),
            };
            let resp = this.api.call("GET", &url, &[], None, true)?;
            cloud::stream(resp, sink)
        })
    }

    fn write_file(&mut self, path: &str, src: &mut dyn Read, size: u64) -> Result<()> {
        let parent = self.folder(&paths::parent(path).unwrap_or_else(|| "/".into()))?;
        let existing = match self.resolve(path) {
            Ok(node) => Some(node),
            Err(e) if cloud::status_of(&e) == Some(404) => None,
            Err(e) => return Err(e),
        };
        if let Some(node) = &existing {
            if node.is_dir() {
                bail!("{path} is a folder");
            }
            if node.shortcut {
                bail!("{path} is a Google Drive shortcut; replace the original file instead");
            }
            if export_format(&node.mime).is_some() {
                bail!("{path} is a Google Docs file and can't be overwritten");
            }
        }
        let session = self.start_upload(existing.as_ref(), &parent, paths::file_name(path), size)?;
        let mut done: Option<File> = None;
        cloud::upload_chunks(src, size, UPLOAD_CHUNK, &mut |offset, bytes| {
            let range = if size == 0 {
                "bytes */0".to_string()
            } else {
                format!("bytes {}-{}/{size}", offset, offset + bytes.len() as u64 - 1)
            };
            let resp = self.api.call("PUT", &session, &[("Content-Range", &range)], Some((bytes, "application/octet-stream")), true)?;
            match resp.status().as_u16() {
                308 => {
                    // Everything sent so far must have arrived ("Range: bytes=0-N").
                    let received = resp
                        .headers()
                        .get("range")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.rsplit('-').next())
                        .and_then(|n| n.parse::<u64>().ok())
                        .map_or(0, |n| n + 1);
                    if received != offset + bytes.len() as u64 {
                        bail!("Google Drive received only part of the upload");
                    }
                }
                _ => done = Some(cloud::read_json(resp)?),
            }
            Ok(())
        })?;
        let Some(file) = done else {
            bail!("Google Drive didn't finish the upload");
        };
        self.nodes.insert(path.to_string(), Node::from_file(&file));
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

    fn file(id: &str, name: &str, mime: &str, created: &str) -> File {
        File {
            id: id.into(),
            name: name.into(),
            mime_type: mime.into(),
            size: Some("5".into()),
            modified_time: None,
            created_time: Some(created.into()),
            shortcut_details: None,
        }
    }

    #[test]
    fn names_are_unique_and_exported() {
        let names: Vec<String> = display_names(vec![
            file("b", "a.txt", "text/plain", "2024-02"),
            file("a", "a.txt", "text/plain", "2024-01"),
            file("c", "a (2).txt", "text/plain", "2024-03"),
            file("d", "Notes", "application/vnd.google-apps.document", "2024-04"),
            file("e", "Sheet.xlsx", "application/vnd.google-apps.spreadsheet", "2024-05"),
            file("f", "Poll", "application/vnd.google-apps.form", "2024-06"),
            file("g", "a/b", FOLDER, "2024-07"),
        ])
        .into_iter()
        .map(|(n, _)| n)
        .collect();
        assert_eq!(names, ["a.txt", "a (3).txt", "a (2).txt", "Notes.docx", "Sheet.xlsx", "a\u{2215}b"]);
    }

    #[test]
    fn shortcuts_point_at_their_target() {
        let mut f = file("s", "Link", SHORTCUT, "2024");
        f.shortcut_details = Some(ShortcutDetails { target_id: "t".into(), target_mime_type: Some(FOLDER.into()) });
        let n = Node::from_file(&f);
        assert!(n.shortcut && n.is_dir());
        assert_eq!((n.id.as_str(), n.content.as_str()), ("s", "t"));
    }

    #[test]
    fn subtree_matching() {
        assert!(is_under("/a/b", "/a"));
        assert!(is_under("/a", "/a"));
        assert!(!is_under("/ab", "/a"));
        assert!(is_under("/x", "/"));
        assert_eq!(numbered("x.tar.gz", 2), "x.tar (2).gz");
        assert_eq!(numbered(".env", 2), ".env (2)");
    }
}
