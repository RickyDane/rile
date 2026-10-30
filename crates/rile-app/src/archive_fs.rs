//! Browsing archives like folders. An archive opens as a read-only
//! "connection" with the id `archive:<absolute path>`, so it reuses
//! everything remote locations have: listing, breadcrumbs, previews,
//! opening files (extracted to a temp folder) and copying out by drag,
//! paste or F5 (extracted on the fly).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use rile_remote::{RemoteEntry, RemoteFs};
use rile_services::archive::{self, ListedEntry};

const PREFIX: &str = "archive:";

/// The connection id for browsing `archive`.
pub fn conn_for(archive: &Path) -> String {
    format!("{PREFIX}{}", archive.display())
}

/// The archive behind a connection id, if it is one.
pub fn archive_of(conn: &str) -> Option<PathBuf> {
    conn.strip_prefix(PREFIX).map(PathBuf::from)
}

pub fn is_archive_conn(conn: &str) -> bool {
    conn.starts_with(PREFIX)
}

/// Whether double-clicking `path` should browse it rather than hand it to
/// the system (set from settings).
pub static BROWSE_ON_OPEN: AtomicBool = AtomicBool::new(true);

pub fn browse_on_open(path: &Path) -> bool {
    BROWSE_ON_OPEN.load(std::sync::atomic::Ordering::Relaxed) && is_browsable(path)
}

/// Multi-entry archives (zip, 7z, tar…); single compressed files aren't
/// worth a folder view.
pub fn is_browsable(path: &Path) -> bool {
    archive::detect(path).is_some_and(|f| f.supports_multiple())
}

type Session = Arc<Mutex<Box<dyn RemoteFs>>>;

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static S: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The (cached) reader for an archive connection.
pub fn session(conn: &str) -> Result<Session> {
    let path = archive_of(conn).context("not an archive")?;
    let mut map = sessions().lock().unwrap();
    if let Some(s) = map.get(conn) {
        return Ok(s.clone());
    }
    let fs: Box<dyn RemoteFs> = Box::new(ArchiveFs::open(path)?);
    let s = Arc::new(Mutex::new(fs));
    map.insert(conn.to_string(), s.clone());
    Ok(s)
}

struct ArchiveFs {
    path: PathBuf,
    stamp: Option<(SystemTime, u64)>,
    entries: Vec<ListedEntry>,
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn read_only() -> anyhow::Error {
    anyhow::anyhow!("Archives are read-only here — extract it to make changes")
}

impl ArchiveFs {
    fn open(path: PathBuf) -> Result<Self> {
        let entries = archive::list(&path, &AtomicBool::new(false))?;
        Ok(Self { stamp: stamp(&path), path, entries })
    }

    /// Re-reads the archive when it changed on disk since it was listed.
    fn refresh(&mut self) -> Result<()> {
        let now = stamp(&self.path);
        if now.is_none() {
            bail!("\"{}\" no longer exists", self.path.display());
        }
        if now != self.stamp {
            self.entries = archive::list(&self.path, &AtomicBool::new(false))?;
            self.stamp = now;
        }
        Ok(())
    }

    fn to_remote(e: &ListedEntry) -> RemoteEntry {
        RemoteEntry {
            name: e.path.rsplit('/').next().unwrap_or(&e.path).to_string(),
            path: format!("/{}", e.path),
            is_dir: e.is_dir,
            is_symlink: e.is_symlink,
            size: e.size,
            modified: e.modified,
            permissions: None,
        }
    }

    fn find(&self, path: &str) -> Result<&ListedEntry> {
        let inner = inner(path);
        self.entries.iter().find(|e| e.path == inner).with_context(|| format!("\"{inner}\" isn't in the archive"))
    }
}

/// `/a/b/` → `a/b`.
fn inner(path: &str) -> &str {
    path.trim_matches('/')
}

impl RemoteFs for ArchiveFs {
    fn home_dir(&mut self) -> Result<String> {
        Ok("/".into())
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        self.refresh()?;
        let dir = inner(path);
        if !dir.is_empty() && !self.find(path)?.is_dir {
            bail!("\"{dir}\" isn't a folder");
        }
        Ok(self
            .entries
            .iter()
            .filter(|e| match e.path.rsplit_once('/') {
                Some((parent, _)) => parent == dir,
                None => dir.is_empty(),
            })
            .map(Self::to_remote)
            .collect())
    }

    fn stat(&mut self, path: &str) -> Result<RemoteEntry> {
        if inner(path).is_empty() {
            return Ok(RemoteEntry { name: String::new(), path: "/".into(), is_dir: true, is_symlink: false, size: 0, modified: None, permissions: None });
        }
        self.refresh()?;
        Ok(Self::to_remote(self.find(path)?))
    }

    fn mkdir(&mut self, _: &str) -> Result<()> {
        Err(read_only())
    }

    fn create_file(&mut self, _: &str) -> Result<()> {
        Err(read_only())
    }

    fn remove(&mut self, _: &str) -> Result<()> {
        Err(read_only())
    }

    fn rename(&mut self, _: &str, _: &str) -> Result<()> {
        Err(read_only())
    }

    fn download(&mut self, remote: &str, local: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
        // Files opened or previewed earlier are extracted again into the
        // same temp spot; replace that copy.
        let temp = std::env::temp_dir().join("rile-remote");
        if local.starts_with(&temp) && std::fs::symlink_metadata(local).is_ok() {
            rile_core::ops::remove_permanently(local)?;
        }
        archive::extract_entry(&self.path, inner(remote), local, cancel, |p| progress(p.bytes_done, p.bytes_total)).map_err(|e| {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) { anyhow::Error::new(rile_remote::Cancelled) } else { e }
        })
    }

    fn upload(&mut self, _: &Path, _: &str, _: &AtomicBool, _: &mut dyn FnMut(u64, u64)) -> Result<()> {
        Err(read_only())
    }

    fn read_head(&mut self, path: &str, max: usize) -> Result<Vec<u8>> {
        use std::io::Read;
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join("rile-remote");
        std::fs::create_dir_all(&dir)?;
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let out = dir.join(format!(".head-{}-{n}", std::process::id()));
        archive::extract_entry(&self.path, inner(path), &out, &AtomicBool::new(false), |_| {})?;
        let mut buf = Vec::new();
        let read = std::fs::File::open(&out).and_then(|f| f.take(max as u64).read_to_end(&mut buf));
        rile_core::ops::remove_permanently(&out).ok();
        read?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_levels_and_extracts() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("proj");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "A").unwrap();
        std::fs::write(src.join("sub/b.txt"), "B").unwrap();
        let zip = t.path().join("proj.zip");
        archive::compress(&[src.clone()], &zip, archive::ArchiveFormat::Zip, 6, &AtomicBool::new(false), |_| {}).unwrap();
        let conn = conn_for(&zip);
        assert_eq!(archive_of(&conn), Some(zip.clone()));
        let s = session(&conn).unwrap();
        let mut fs = s.lock().unwrap();
        let root: Vec<String> = fs.list("/").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(root, ["proj"]);
        let mut level: Vec<(String, bool)> = fs.list("/proj").unwrap().into_iter().map(|e| (e.name, e.is_dir)).collect();
        level.sort();
        assert_eq!(level, [("a.txt".to_string(), false), ("sub".to_string(), true)]);
        assert_eq!(fs.read_head("/proj/sub/b.txt", 10).unwrap(), b"B");
        let out = t.path().join("out-sub");
        fs.download("/proj/sub", &out, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("b.txt")).unwrap(), "B");
        assert!(fs.remove("/proj/a.txt").is_err());
    }
}
