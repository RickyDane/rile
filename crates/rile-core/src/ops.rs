//! File operations. Long-running ones (copy/move) report byte progress
//! through a callback and honour a cancel flag; they're meant to run on a
//! background thread.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

const COPY_BUF: usize = 1 << 20;

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub items_done: usize,
    pub items_total: usize,
    /// Name of the file being transferred right now (relative to the
    /// top-level item it belongs to), when known.
    pub current: Option<Arc<str>>,
    pub file_bytes_done: u64,
    pub file_bytes_total: u64,
    /// Individual entries (files, folders, links; recursively) finished /
    /// in total; 0 when unknown.
    pub files_done: usize,
    pub files_total: usize,
}

impl Progress {
    /// Progress of the current file (0..1), if one is being transferred.
    pub fn file_fraction(&self) -> Option<f32> {
        self.current.as_ref()?;
        Some(if self.file_bytes_total == 0 { 1.0 } else { (self.file_bytes_done as f64 / self.file_bytes_total as f64) as f32 })
    }

    pub fn fraction(&self) -> f32 {
        if self.bytes_total == 0 {
            return match (self.files_total, self.items_total) {
                (f, _) if f > 0 => self.files_done as f32 / f as f32,
                (_, i) if i > 0 => self.items_done as f32 / i as f32,
                _ => 1.0,
            };
        }
        (self.bytes_done as f64 / self.bytes_total as f64) as f32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    Copy,
    Move,
}

/// Returns a path in `dir` named like `name` that doesn't exist yet:
/// `foo.txt`, `foo copy.txt`, `foo copy 2.txt`, ...
pub fn unique_destination(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !exists_no_follow(&candidate) {
        return candidate;
    }
    let p = Path::new(name);
    let (stem, ext) = match (p.file_stem(), p.extension()) {
        (Some(s), Some(e)) if !name.starts_with('.') || s.len() > 1 => {
            (s.to_string_lossy().into_owned(), format!(".{}", e.to_string_lossy()))
        }
        _ => (name.to_string(), String::new()),
    };
    for n in 1.. {
        let suffix = if n == 1 { " copy".to_string() } else { format!(" copy {n}") };
        let candidate = dir.join(format!("{stem}{suffix}{ext}"));
        if !exists_no_follow(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn exists_no_follow(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn tree_size(path: &Path) -> (u64, usize) {
    let Ok(meta) = fs::symlink_metadata(path) else { return (0, 0) };
    if meta.is_dir() {
        let mut total = (0, 1);
        if let Ok(rd) = fs::read_dir(path) {
            for e in rd.flatten() {
                let (b, n) = tree_size(&e.path());
                total.0 += b;
                total.1 += n;
            }
        }
        total
    } else {
        (meta.len(), 1)
    }
}

/// Copies or moves `sources` into `dest_dir`. Name conflicts get a
/// " copy" suffix; copying a folder into itself is rejected.
pub fn transfer(
    sources: &[PathBuf],
    dest_dir: &Path,
    kind: TransferKind,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
) -> Result<Vec<PathBuf>> {
    for src in sources {
        if src.is_dir() && dest_dir.starts_with(src) {
            bail!("Can't {} \"{}\" into itself", if kind == TransferKind::Copy { "copy" } else { "move" }, src.display());
        }
    }

    // Moves within a filesystem are a cheap rename; only sum sizes for the rest.
    let mut progress = Progress { items_total: sources.len(), ..Default::default() };
    let mut pending = Vec::new();
    let mut created = Vec::new();
    for src in sources {
        let name = src.file_name().context("source has no file name")?.to_string_lossy();
        if kind == TransferKind::Move && src.parent() == Some(dest_dir) {
            progress.items_done += 1;
            created.push(src.clone());
            continue;
        }
        let dest = unique_destination(dest_dir, &name);
        if kind == TransferKind::Move && fs::rename(src, &dest).is_ok() {
            progress.items_done += 1;
            created.push(dest);
            continue;
        }
        pending.push((src.clone(), dest));
    }
    for (src, _) in &pending {
        let (bytes, files) = tree_size(src);
        progress.bytes_total += bytes;
        progress.files_total += files;
    }
    on_progress(progress.clone());

    for (src, dest) in pending {
        let root = src.parent().map(Path::to_path_buf).unwrap_or_default();
        copy_tree(&src, &dest, &root, cancel, &mut progress, &mut on_progress)?;
        if kind == TransferKind::Move {
            remove_permanently(&src)?;
        }
        progress.items_done += 1;
        on_progress(progress.clone());
        created.push(dest);
    }
    Ok(created)
}

fn copy_tree(
    src: &Path,
    dest: &Path,
    root: &Path,
    cancel: &AtomicBool,
    progress: &mut Progress,
    on_progress: &mut impl FnMut(Progress),
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled");
    }
    let meta = fs::symlink_metadata(src)?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        let target = fs::read_link(src)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, dest)?;
        #[cfg(windows)]
        {
            if src.is_dir() {
                std::os::windows::fs::symlink_dir(target, dest)?
            } else {
                std::os::windows::fs::symlink_file(target, dest)?
            }
        }
        progress.files_done += 1;
    } else if ft.is_dir() {
        fs::create_dir(dest).with_context(|| format!("creating {}", dest.display()))?;
        progress.files_done += 1;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dest.join(e.file_name()), root, cancel, progress, on_progress)?;
        }
        fs::set_permissions(dest, meta.permissions()).ok();
    } else {
        progress.current = Some(src.strip_prefix(root).unwrap_or(src).to_string_lossy().into());
        progress.file_bytes_done = 0;
        progress.file_bytes_total = meta.len();
        copy_file(src, dest, cancel, progress, on_progress)?;
        progress.files_done += 1;
        fs::set_permissions(dest, meta.permissions()).ok();
        if let Ok(m) = meta.modified() {
            fs::File::options().write(true).open(dest).and_then(|f| f.set_modified(m)).ok();
        }
    }
    Ok(())
}

fn copy_file(
    src: &Path,
    dest: &Path,
    cancel: &AtomicBool,
    progress: &mut Progress,
    on_progress: &mut impl FnMut(Progress),
) -> Result<()> {
    let mut input = fs::File::open(src).with_context(|| format!("opening {}", src.display()))?;
    let mut output = fs::File::create_new(dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(output);
            fs::remove_file(dest).ok();
            bail!("Cancelled");
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n])?;
        progress.bytes_done += n as u64;
        progress.file_bytes_done += n as u64;
        on_progress(progress.clone());
    }
    Ok(())
}

pub fn move_to_trash(paths: &[PathBuf]) -> Result<()> {
    #[allow(unused_mut)]
    let mut ctx = trash::TrashContext::default();
    // NSFileManager is much faster than scripting Finder and makes no sound.
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx.delete_all(paths).context("moving to trash")
}

pub fn remove_permanently(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .with_context(|| format!("deleting {}", path.display()))
}

pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    validate_name(new_name)?;
    let dest = path.with_file_name(new_name);
    if dest != path && exists_no_follow(&dest) {
        bail!("\"{new_name}\" already exists");
    }
    fs::rename(path, &dest)?;
    Ok(dest)
}

pub fn create_folder(dir: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let dest = unique_destination(dir, name);
    fs::create_dir(&dest)?;
    Ok(dest)
}

/// Creates an empty file named like `name`, suffixed if it already exists.
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let dest = unique_destination(dir, name);
    fs::File::create_new(&dest).with_context(|| format!("creating {}", dest.display()))?;
    Ok(dest)
}

/// Duplicates each path next to itself ("foo copy").
pub fn duplicate(paths: &[PathBuf], cancel: &AtomicBool) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        let dir = p.parent().context("no parent")?;
        out.extend(transfer(std::slice::from_ref(p), dir, TransferKind::Copy, cancel, |_| {})?);
    }
    Ok(out)
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        bail!("Invalid name \"{name}\"");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert_eq!(unique_destination(d, "a.txt"), d.join("a.txt"));
        fs::write(d.join("a.txt"), "").unwrap();
        assert_eq!(unique_destination(d, "a.txt"), d.join("a copy.txt"));
        fs::write(d.join("a copy.txt"), "").unwrap();
        assert_eq!(unique_destination(d, "a.txt"), d.join("a copy 2.txt"));
        fs::write(d.join(".env"), "").unwrap();
        assert_eq!(unique_destination(d, ".env"), d.join(".env copy"));
    }

    #[test]
    fn copy_and_move_tree() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/f.bin"), vec![7u8; 3 * COPY_BUF + 5]).unwrap();
        let dst = t.path().join("dst");
        fs::create_dir(&dst).unwrap();
        let cancel = AtomicBool::new(false);
        let mut last = Progress::default();
        let out = transfer(std::slice::from_ref(&src), &dst, TransferKind::Copy, &cancel, |p| last = p).unwrap();
        assert_eq!(out, [dst.join("src")]);
        assert_eq!(last.bytes_done, last.bytes_total);
        // src, src/sub and src/sub/f.bin
        assert_eq!((last.files_done, last.files_total), (3, 3));
        assert_eq!(last.current.as_deref(), Some("src/sub/f.bin"));
        assert_eq!(last.file_fraction(), Some(1.0));
        assert_eq!(fs::read(dst.join("src/sub/f.bin")).unwrap().len(), 3 * COPY_BUF + 5);

        // Second copy into same dir gets suffixed.
        let out = transfer(std::slice::from_ref(&src), &dst, TransferKind::Copy, &cancel, |_| {}).unwrap();
        assert_eq!(out, [dst.join("src copy")]);

        let moved = transfer(std::slice::from_ref(&src), &dst.join("src"), TransferKind::Move, &cancel, |_| {}).unwrap();
        assert!(!src.exists());
        assert_eq!(moved, [dst.join("src/src")]);
    }

    #[test]
    fn refuses_copy_into_self() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a");
        fs::create_dir_all(src.join("b")).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(transfer(std::slice::from_ref(&src), &src.join("b"), TransferKind::Copy, &cancel, |_| {}).is_err());
    }

    #[test]
    fn rename_and_mkdir() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("x");
        fs::write(&f, "").unwrap();
        let r = rename(&f, "y").unwrap();
        assert!(r.exists() && !f.exists());
        assert!(rename(&r, "a/b").is_err());
        let d1 = create_folder(t.path(), "untitled folder").unwrap();
        let d2 = create_folder(t.path(), "untitled folder").unwrap();
        assert_ne!(d1, d2);
        let cancel = AtomicBool::new(false);
        assert_eq!(duplicate(&[r], &cancel).unwrap(), [t.path().join("y copy")]);
        let f1 = create_file(t.path(), "untitled.txt").unwrap();
        let f2 = create_file(t.path(), "untitled.txt").unwrap();
        assert_eq!((f1.file_name().unwrap(), f2.file_name().unwrap()), ("untitled.txt".as_ref(), "untitled copy.txt".as_ref()));
    }
}
