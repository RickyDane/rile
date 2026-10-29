//! Protocol-independent logic (recursion, progress, cancellation) on top of a
//! minimal per-protocol [`Backend`].

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

use crate::{Cancelled, RemoteEntry, paths};

pub(crate) const CHUNK: usize = 256 * 1024;

/// Minimal operations each protocol implements. Paths are absolute remote paths.
pub(crate) trait Backend: Send {
    /// Working directory right after login.
    fn login_dir(&mut self) -> Result<String>;
    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>>;
    /// Metadata of `path` itself (symlinks are reported with `is_symlink`; `is_dir`
    /// describes the link target when known).
    fn stat(&mut self, path: &str) -> Result<RemoteEntry>;
    fn mkdir(&mut self, path: &str) -> Result<()>;
    fn remove_file(&mut self, path: &str) -> Result<()>;
    fn remove_dir(&mut self, path: &str) -> Result<()>;
    fn rename(&mut self, from: &str, to: &str) -> Result<()>;
    /// Stream a file's contents to `sink`; `sink` returns `Ok(false)` to stop early.
    fn read_file(&mut self, path: &str, sink: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<()>;
    /// Create/truncate `path` and write everything `src` yields. `size` is the
    /// number of bytes `src` will yield (cloud uploads need it up front).
    fn write_file(&mut self, path: &str, src: &mut dyn Read, size: u64) -> Result<()>;
    /// False once the connection is known to be unusable.
    fn is_alive(&self) -> bool;
    /// Whether a failed or cancelled [`Backend::write_file`] leaves the previous
    /// file untouched (so it must not be removed afterwards).
    fn atomic_writes(&self) -> bool {
        false
    }
    /// Remove `path` and everything below it in one request, when the backend
    /// can (cloud drives move whole folders to their trash). `None` = use the
    /// generic recursive removal.
    fn remove_all(&mut self, _path: &str) -> Option<Result<()>> {
        None
    }
    /// See [`crate::RemoteFs::capacity`].
    fn capacity(&mut self, _path: &str) -> Result<Option<crate::Capacity>> {
        Ok(None)
    }
}

pub(crate) fn remove_recursive(b: &mut dyn Backend, path: &str) -> Result<()> {
    let path = paths::normalize(path);
    if path == "/" {
        bail!("refusing to remove the root directory");
    }
    if let Some(result) = b.remove_all(&path) {
        return result;
    }
    let entry = b.stat(&path)?;
    if entry.is_dir && !entry.is_symlink {
        for child in b.list(&path)? {
            remove_recursive(b, &child.path)?;
        }
        b.remove_dir(&path)
    } else {
        b.remove_file(&path)
    }
}

pub(crate) fn read_head(b: &mut dyn Backend, path: &str, max: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(max.min(CHUNK));
    if max == 0 {
        return Ok(out);
    }
    b.read_file(path, &mut |chunk| {
        let take = chunk.len().min(max - out.len());
        out.extend_from_slice(&chunk[..take]);
        Ok(out.len() < max)
    })?;
    Ok(out)
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Cancelled.into())
    } else {
        Ok(())
    }
}

struct Progress<'a> {
    done: u64,
    total: u64,
    cb: &'a mut dyn FnMut(u64, u64),
}

impl Progress<'_> {
    fn add(&mut self, n: u64) {
        self.done += n;
        (self.cb)(self.done, self.total.max(self.done));
    }
}

// ---------------------------------------------------------------- download

/// Plan: (remote path, local path, is_dir, size), in creation order.
type Plan = Vec<(String, std::path::PathBuf, bool, u64)>;

fn plan_download(
    b: &mut dyn Backend,
    entry: &RemoteEntry,
    local: &Path,
    cancel: &AtomicBool,
    plan: &mut Plan,
) -> Result<()> {
    check_cancel(cancel)?;
    if entry.is_dir {
        if entry.is_symlink {
            // Not followed, to avoid cycles.
            log::info!("skipping symlinked directory {}", entry.path);
            return Ok(());
        }
        plan.push((entry.path.clone(), local.to_path_buf(), true, 0));
        for child in b.list(&entry.path)? {
            // Names come from the server: one that isn't a plain file name
            // here (`..`, `C:\x`, `a\..\b` or `a:stream` on Windows) could
            // write outside the download folder.
            if !rile_core::ops::is_safe_component(&child.name) {
                log::warn!("skipping {}: its name isn't valid on this computer", child.path);
                continue;
            }
            plan_download(b, &child, &local.join(&child.name), cancel, plan)?;
        }
    } else {
        plan.push((entry.path.clone(), local.to_path_buf(), false, entry.size));
    }
    Ok(())
}

/// Returns the number of bytes transferred before the error/success
/// (used by the reconnect logic to decide whether a retry is safe).
pub(crate) fn download(
    b: &mut dyn Backend,
    remote: &str,
    local: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
    transferred: &mut u64,
) -> Result<()> {
    let root = b.stat(remote)?;
    let mut plan = Plan::new();
    plan_download(b, &root, local, cancel, &mut plan)?;
    let total = plan.iter().map(|p| p.3).sum();
    let mut prog = Progress {
        done: 0,
        total,
        cb: progress,
    };
    (prog.cb)(0, total);
    for (rpath, lpath, is_dir, _) in plan {
        check_cancel(cancel)?;
        if is_dir {
            fs::create_dir_all(&lpath)
                .with_context(|| format!("creating directory {}", lpath.display()))?;
            continue;
        }
        if let Some(parent) = lpath.parent() {
            fs::create_dir_all(parent).ok();
        }
        let mut file =
            fs::File::create(&lpath).with_context(|| format!("creating {}", lpath.display()))?;
        let res = b.read_file(&rpath, &mut |chunk| {
            check_cancel(cancel)?;
            file.write_all(chunk)
                .with_context(|| format!("writing {}", lpath.display()))?;
            *transferred += chunk.len() as u64;
            prog.add(chunk.len() as u64);
            Ok(true)
        });
        if let Err(e) = res {
            drop(file);
            let _ = fs::remove_file(&lpath);
            return Err(e).with_context(|| format!("downloading {rpath}"));
        }
        file.flush()?;
    }
    Ok(())
}

// ---------------------------------------------------------------- upload

/// A `Read` adapter that reports progress and honours cancellation.
struct ProgressReader<'a, 'b> {
    inner: fs::File,
    cancel: &'a AtomicBool,
    prog: &'a mut Progress<'b>,
    transferred: &'a mut u64,
}

impl Read for ProgressReader<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other(Cancelled));
        }
        let n = self.inner.read(buf)?;
        *self.transferred += n as u64;
        self.prog.add(n as u64);
        Ok(n)
    }
}

type LocalPlan = Vec<(std::path::PathBuf, String, bool, u64)>;

fn plan_upload(
    local: &Path,
    remote: &str,
    cancel: &AtomicBool,
    plan: &mut LocalPlan,
) -> Result<()> {
    check_cancel(cancel)?;
    let lmeta =
        fs::symlink_metadata(local).with_context(|| format!("reading {}", local.display()))?;
    let meta = if lmeta.file_type().is_symlink() {
        match fs::metadata(local) {
            Ok(m) if m.is_dir() => {
                log::info!("skipping symlinked directory {}", local.display());
                return Ok(());
            }
            Ok(m) => m,
            Err(_) => {
                log::info!("skipping broken symlink {}", local.display());
                return Ok(());
            }
        }
    } else {
        lmeta
    };
    if meta.is_dir() {
        plan.push((local.to_path_buf(), remote.to_string(), true, 0));
        let mut children: Vec<_> = fs::read_dir(local)
            .with_context(|| format!("reading directory {}", local.display()))?
            .collect::<io::Result<_>>()?;
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            plan_upload(&child.path(), &paths::join(remote, &name), cancel, plan)?;
        }
    } else if meta.is_file() {
        plan.push((local.to_path_buf(), remote.to_string(), false, meta.len()));
    } else {
        log::info!("skipping special file {}", local.display());
    }
    Ok(())
}

pub(crate) fn upload(
    b: &mut dyn Backend,
    local: &Path,
    remote: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
    transferred: &mut u64,
) -> Result<()> {
    let mut plan = LocalPlan::new();
    plan_upload(local, &paths::normalize(remote), cancel, &mut plan)?;
    let total = plan.iter().map(|p| p.3).sum();
    let mut prog = Progress {
        done: 0,
        total,
        cb: progress,
    };
    (prog.cb)(0, total);
    for (lpath, rpath, is_dir, _) in plan {
        check_cancel(cancel)?;
        if is_dir {
            match b.stat(&rpath) {
                Ok(e) if e.is_dir => {}
                Ok(_) => bail!("{rpath} exists and is not a directory"),
                Err(_) => b
                    .mkdir(&rpath)
                    .with_context(|| format!("creating directory {rpath}"))?,
            }
            continue;
        }
        let file =
            fs::File::open(&lpath).with_context(|| format!("opening {}", lpath.display()))?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let mut reader = ProgressReader {
            inner: file,
            cancel,
            prog: &mut prog,
            transferred,
        };
        if let Err(e) = b.write_file(&rpath, &mut reader, size) {
            if crate::is_cancelled(&e) {
                if b.is_alive() && !b.atomic_writes() {
                    let _ = b.remove_file(&rpath);
                }
                return Err(Cancelled.into());
            }
            return Err(e).with_context(|| format!("uploading {}", lpath.display()));
        }
    }
    Ok(())
}
