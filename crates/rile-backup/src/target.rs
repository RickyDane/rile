//! Backup destinations. Paths are relative to the destination root and
//! `/`-separated; writes go to a temporary name first and are renamed into
//! place, so an interrupted run never leaves a half-written file behind.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use anyhow::{Context, Result};
use rile_remote::{RemoteFs, paths};

/// Suffix of files being written.
pub const PARTIAL: &str = ".rile-partial";

/// A file found in the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestFile {
    pub path: String,
    pub size: u64,
    /// Only meaningful where [`Target::keeps_mtime`] holds.
    pub mtime: Option<SystemTime>,
}

pub trait Target: Send {
    /// Human-readable location, for messages.
    fn describe(&self) -> String;
    /// Whether copied files keep their modification time (so it can be
    /// compared when re-checking the destination).
    fn keeps_mtime(&self) -> bool;
    /// Makes sure the root folder exists.
    fn prepare(&mut self) -> Result<()>;
    /// Reads a small file, `None` when it doesn't exist.
    fn read_small(&mut self, path: &str) -> Result<Option<Vec<u8>>>;
    fn write_small(&mut self, path: &str, bytes: &[u8]) -> Result<()>;
    /// Every file below `dir` (recursively); empty when `dir` doesn't exist.
    fn walk(&mut self, dir: &str, out: &mut Vec<DestFile>) -> Result<()>;
    /// Names of the entries directly inside `dir` (empty when missing).
    fn list_names(&mut self, dir: &str) -> Result<Vec<String>>;
    /// Copies the local file `src` to `path`, replacing it.
    fn put(&mut self, src: &Path, mtime: Option<SystemTime>, path: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<()>;
    /// Copies the backed up file `path` to the local file `dest` (created or
    /// truncated; its folder must exist).
    fn get(&mut self, path: &str, dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<()>;
    /// Moves `from` to `to` (creating `to`'s parents). Ok(false) when `from` doesn't exist.
    fn move_file(&mut self, from: &str, to: &str) -> Result<bool>;
    /// Removes a file or folder recursively (no error when missing).
    fn remove_all(&mut self, path: &str) -> Result<()>;
    /// Removes `dir` if it is an empty folder (best effort).
    fn remove_dir_if_empty(&mut self, _dir: &str) {}
}

// ---------------------------------------------------------------- local

pub struct LocalTarget {
    root: PathBuf,
}

impl LocalTarget {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn abs(&self, rel: &str) -> PathBuf {
        let mut p = self.root.clone();
        for part in rel.split('/').filter(|p| !p.is_empty()) {
            p.push(part);
        }
        p
    }
}

fn walk_local(dir: &Path, rel: &str, out: &mut Vec<DestFile>) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let meta = entry.metadata()?;
        if meta.is_dir() {
            walk_local(&entry.path(), &child, out)?;
        } else if meta.is_file() && !name.ends_with(PARTIAL) {
            out.push(DestFile { path: child, size: meta.len(), mtime: meta.modified().ok() });
        }
    }
    Ok(())
}

impl Target for LocalTarget {
    fn describe(&self) -> String {
        self.root.display().to_string()
    }

    fn keeps_mtime(&self) -> bool {
        true
    }

    fn prepare(&mut self) -> Result<()> {
        // A missing parent or volume means an unplugged drive: don't create
        // the folder on the startup disk instead.
        let volume: PathBuf = self.root.components().take(3).collect();
        // Windows: the drive (`E:\`) itself.
        let drive: PathBuf = self.root.components().take(2).collect();
        let missing = match self.root.parent() {
            _ if self.root.starts_with("/Volumes") && !volume.exists() => Some(volume),
            _ if cfg!(windows) && !drive.exists() => Some(drive),
            Some(parent) if !parent.as_os_str().is_empty() && !parent.exists() => Some(parent.to_path_buf()),
            _ => None,
        };
        if let Some(missing) = missing {
            anyhow::bail!("{} is not available (is the drive connected?)", missing.display());
        }
        fs::create_dir_all(&self.root).with_context(|| format!("creating {}", self.root.display()))
    }

    fn read_small(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(self.abs(path)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn write_small(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        crate::state::write_atomic(&self.abs(path), bytes)
    }

    fn walk(&mut self, dir: &str, out: &mut Vec<DestFile>) -> Result<()> {
        walk_local(&self.abs(dir), dir.trim_matches('/'), out)
    }

    fn list_names(&mut self, dir: &str) -> Result<Vec<String>> {
        match fs::read_dir(self.abs(dir)) {
            Ok(entries) => Ok(entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn put(&mut self, src: &Path, mtime: Option<SystemTime>, path: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<()> {
        use std::io::Read;
        let dest = self.abs(path);
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let mut tmp_name = dest.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(PARTIAL);
        let tmp = dest.with_file_name(tmp_name);
        let result = (|| -> Result<()> {
            let mut input = fs::File::open(src).with_context(|| format!("opening {}", src.display()))?;
            let mut output = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(rile_remote::Cancelled.into());
                }
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                output.write_all(&buf[..n])?;
                progress(n as u64);
            }
            if let Ok(meta) = input.metadata() {
                output.set_permissions(meta.permissions()).ok();
            }
            if let Some(t) = mtime {
                output.set_modified(t).ok();
            }
            output.sync_all().ok();
            Ok(())
        })();
        if let Err(e) = result {
            fs::remove_file(&tmp).ok();
            return Err(e);
        }
        // Windows won't replace a read-only file (the copy of a read-only
        // source from an earlier run).
        #[cfg(windows)]
        if let Ok(meta) = fs::metadata(&dest) {
            let mut perms = meta.permissions();
            if perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                fs::set_permissions(&dest, perms).ok();
            }
        }
        fs::rename(&tmp, &dest).with_context(|| format!("replacing {}", dest.display()))
    }

    fn get(&mut self, path: &str, dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<()> {
        use std::io::Read;
        let src = self.abs(path);
        let mut input = fs::File::open(&src).with_context(|| format!("opening {}", src.display()))?;
        let mut output = fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(rile_remote::Cancelled.into());
            }
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n])?;
            progress(n as u64);
        }
        if let Ok(meta) = input.metadata() {
            output.set_permissions(meta.permissions()).ok();
        }
        Ok(())
    }

    fn move_file(&mut self, from: &str, to: &str) -> Result<bool> {
        let (from, to) = (self.abs(from), self.abs(to));
        if fs::symlink_metadata(&from).is_err() {
            return Ok(false);
        }
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::rename(&from, &to).with_context(|| format!("moving {} to {}", from.display(), to.display()))?;
        Ok(true)
    }

    fn remove_all(&mut self, path: &str) -> Result<()> {
        let p = self.abs(path);
        if fs::symlink_metadata(&p).is_ok() {
            // Also clears read-only attributes, which block deletes on Windows.
            rile_core::ops::remove_permanently(&p)?;
        }
        Ok(())
    }

    fn remove_dir_if_empty(&mut self, dir: &str) {
        fs::remove_dir(self.abs(dir)).ok();
    }
}

// ---------------------------------------------------------------- remote

/// A folder on an SFTP/FTP/SMB server, over a connection of its own.
pub struct RemoteTarget {
    fs: Box<dyn RemoteFs>,
    root: String,
    label: String,
    /// Folders known to exist (avoids a round trip per file).
    made: HashSet<String>,
    /// Folders that couldn't be created, with the reason (not retried per file).
    unmakeable: std::collections::HashMap<String, String>,
}

impl RemoteTarget {
    pub fn new(fs: Box<dyn RemoteFs>, root: &str, label: impl Into<String>) -> Self {
        Self { fs, root: paths::normalize(root), label: label.into(), made: HashSet::new(), unmakeable: Default::default() }
    }

    fn abs(&self, rel: &str) -> String {
        let rel = rel.trim_matches('/');
        if rel.is_empty() { self.root.clone() } else { paths::join(&self.root, rel) }
    }

    fn exists(&mut self, abs: &str) -> bool {
        self.fs.stat(abs).is_ok()
    }

    fn mkdirs(&mut self, abs: &str) -> Result<()> {
        if abs == "/" || self.made.contains(abs) {
            return Ok(());
        }
        if let Some(reason) = self.unmakeable.get(abs) {
            anyhow::bail!("{reason}");
        }
        match self.fs.stat(abs) {
            Ok(e) if e.is_dir => {}
            Ok(_) => anyhow::bail!("{abs} exists and is not a folder"),
            Err(_) => {
                if let Some(parent) = paths::parent(abs) {
                    self.mkdirs(&parent)?;
                }
                if let Err(e) = self.fs.mkdir(abs) {
                    // It may exist after all, merely hidden from listings.
                    if self.fs.list(abs).is_err() {
                        let reason = format!("{:#}", e.context(format!("creating folder {abs}")));
                        self.unmakeable.insert(abs.to_string(), reason.clone());
                        anyhow::bail!("{reason}");
                    }
                }
            }
        }
        self.made.insert(abs.to_string());
        Ok(())
    }

    fn upload(&mut self, src: &Path, abs: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<()> {
        if let Some(parent) = paths::parent(abs) {
            self.mkdirs(&parent)?;
        }
        let tmp = format!("{abs}{PARTIAL}");
        let mut last = 0;
        let result = self.fs.upload(src, &tmp, cancel, &mut |done, _| {
            progress(done.saturating_sub(last));
            last = done;
        });
        if let Err(e) = result {
            self.fs.remove(&tmp).ok();
            return Err(e);
        }
        // Not every server lets a rename replace an existing file.
        if self.exists(abs) {
            self.fs.remove(abs).with_context(|| format!("replacing {abs}"))?;
        }
        self.fs.rename(&tmp, abs).with_context(|| format!("renaming {tmp}"))
    }
}

impl Target for RemoteTarget {
    fn describe(&self) -> String {
        format!("{}:{}", self.label, self.root)
    }

    fn keeps_mtime(&self) -> bool {
        false
    }

    fn prepare(&mut self) -> Result<()> {
        let root = self.root.clone();
        self.mkdirs(&root)
    }

    fn read_small(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        let abs = self.abs(path);
        if !self.exists(&abs) {
            return Ok(None);
        }
        Ok(Some(self.fs.read_head(&abs, 4 * 1024 * 1024)?))
    }

    fn write_small(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        let mut tmp = tempfile_in_temp()?;
        tmp.1.write_all(bytes)?;
        tmp.1.flush()?;
        let abs = self.abs(path);
        let result = self.upload(&tmp.0, &abs, &AtomicBool::new(false), &mut |_| {});
        fs::remove_file(&tmp.0).ok();
        result
    }

    fn walk(&mut self, dir: &str, out: &mut Vec<DestFile>) -> Result<()> {
        let abs = self.abs(dir);
        let Ok(entries) = self.fs.list(&abs) else {
            return Ok(());
        };
        let rel = dir.trim_matches('/');
        for e in entries {
            let child = if rel.is_empty() { e.name.clone() } else { format!("{rel}/{}", e.name) };
            if e.is_dir {
                if !e.is_symlink {
                    self.made.insert(e.path.clone());
                    self.walk(&child, out)?;
                }
            } else if !e.name.ends_with(PARTIAL) {
                out.push(DestFile { path: child, size: e.size, mtime: e.modified });
            }
        }
        Ok(())
    }

    fn list_names(&mut self, dir: &str) -> Result<Vec<String>> {
        let abs = self.abs(dir);
        Ok(self.fs.list(&abs).map(|l| l.into_iter().map(|e| e.name).collect()).unwrap_or_default())
    }

    fn put(&mut self, src: &Path, _mtime: Option<SystemTime>, path: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<()> {
        let abs = self.abs(path);
        self.upload(src, &abs, cancel, progress)
    }

    fn get(&mut self, path: &str, dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<()> {
        let abs = self.abs(path);
        let mut last = 0;
        self.fs.download(&abs, dest, cancel, &mut |done, _| {
            progress(done.saturating_sub(last));
            last = done;
        })
    }

    fn move_file(&mut self, from: &str, to: &str) -> Result<bool> {
        let (from, to) = (self.abs(from), self.abs(to));
        if !self.exists(&from) {
            return Ok(false);
        }
        if let Some(parent) = paths::parent(&to) {
            self.mkdirs(&parent)?;
        }
        self.fs.rename(&from, &to).with_context(|| format!("moving {from} to {to}"))?;
        Ok(true)
    }

    fn remove_all(&mut self, path: &str) -> Result<()> {
        let abs = self.abs(path);
        if self.exists(&abs) {
            self.fs.remove(&abs)?;
        }
        self.made.retain(|d| !(d == &abs || d.starts_with(&format!("{abs}/"))));
        Ok(())
    }
}

/// A uniquely named file in the temp dir (removed by the caller).
fn tempfile_in_temp() -> Result<(PathBuf, fs::File)> {
    let path = std::env::temp_dir().join(format!("rile-backup-{}-{}", std::process::id(), crate::config::new_id()));
    let file = fs::File::create(&path)?;
    Ok((path, file))
}
