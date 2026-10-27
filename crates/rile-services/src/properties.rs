//! File and folder properties ("Get Info").
//!
//! [`basic`] is a handful of syscalls and never recurses; folder totals are
//! computed separately by [`dir_totals`], which callers run on a background
//! thread and can cancel.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use ignore::{WalkBuilder, WalkState};

/// How often `dir_totals` reports progress.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Bytes read from the start of a file for magic-number sniffing.
const SNIFF_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Properties {
    pub path: PathBuf,
    pub name: String,
    pub kind_label: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub symlink_target: Option<PathBuf>,
    /// File size; for directories 0 from [`basic`] until filled in from
    /// [`dir_totals`] (see [`Properties::apply_totals`]).
    pub size: u64,
    pub file_count: u64,
    pub dir_count: u64,
    /// Bytes allocated on disk (unix only).
    pub allocated: Option<u64>,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub readonly: bool,
    /// Unix permissions, e.g. `rwxr-xr-x (755)`.
    pub permissions: Option<String>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub mime: Option<String>,
}

impl Properties {
    /// Stores the result of [`dir_totals`].
    pub fn apply_totals(&mut self, (bytes, files, dirs): (u64, u64, u64)) {
        self.size = bytes;
        self.file_count = files;
        self.dir_count = dirs;
    }
}

/// Fast, non-recursive properties for `path`. Symlinks are described as
/// links, with size/kind/dates taken from the target when it exists.
pub fn basic(path: &Path) -> Result<Properties> {
    let lmeta = fs::symlink_metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let is_symlink = lmeta.file_type().is_symlink();
    let (symlink_target, meta) = if is_symlink {
        (fs::read_link(path).ok(), fs::metadata(path).unwrap_or_else(|_| lmeta.clone()))
    } else {
        (None, lmeta.clone())
    };
    let is_dir = meta.is_dir();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let mime = if is_dir {
        Some("inode/directory".to_string())
    } else if meta.is_file() {
        guess_mime(path)
    } else {
        None
    };

    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut p = Properties {
        path: path.to_path_buf(),
        kind_label: kind_label(symlink_target.as_deref().unwrap_or(path), is_dir, is_symlink, &meta),
        name,
        is_dir,
        is_symlink,
        symlink_target,
        size: if is_dir { 0 } else { meta.len() },
        file_count: 0,
        dir_count: 0,
        allocated: None,
        created: meta.created().ok(),
        modified: meta.modified().ok(),
        accessed: meta.accessed().ok(),
        readonly: meta.permissions().readonly(),
        permissions: None,
        owner: None,
        group: None,
        mime,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        p.allocated = Some(lmeta.blocks() * 512);
        p.permissions = Some(format_mode(meta.mode()));
        p.owner = Some(
            uzers::get_user_by_uid(meta.uid())
                .map(|u| u.name().to_string_lossy().into_owned())
                .unwrap_or_else(|| meta.uid().to_string()),
        );
        p.group = Some(
            uzers::get_group_by_gid(meta.gid())
                .map(|g| g.name().to_string_lossy().into_owned())
                .unwrap_or_else(|| meta.gid().to_string()),
        );
    }
    Ok(p)
}

/// `path` is the entry itself, or the link target for symlinks (only its
/// extension is used).
fn kind_label(path: &Path, is_dir: bool, is_symlink: bool, meta: &fs::Metadata) -> String {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let base = if is_dir {
        match ext.as_str() {
            "app" => "Application".to_string(),
            "bundle" | "framework" | "plugin" => format!("{} bundle", ext.to_uppercase()),
            _ => "Folder".to_string(),
        }
    } else if !meta.is_file() {
        "Special file".to_string()
    } else if ext.is_empty() {
        if is_executable(meta) { "Executable".to_string() } else { "Document".to_string() }
    } else {
        format!("{} file", ext.to_uppercase())
    };
    if is_symlink && meta.file_type().is_symlink() {
        // metadata() failed so we only have the link itself.
        "Broken link".to_string()
    } else if is_symlink {
        format!("Alias to {base}")
    } else {
        base
    }
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

/// MIME type from magic bytes (first 8 KB) with an extension fallback.
pub fn guess_mime(path: &Path) -> Option<String> {
    let mut buf = [0u8; SNIFF_BYTES];
    let sniffed = fs::File::open(path).ok().and_then(|mut f| {
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => n += k,
            }
        }
        infer::get(&buf[..n]).map(|t| t.mime_type().to_string())
    });
    sniffed.or_else(|| mime_guess::from_path(path).first().map(|m| m.essence_str().to_string()))
}

/// Formats a unix mode as `rwxr-xr-x (755)`, including setuid/setgid/sticky.
pub fn format_mode(mode: u32) -> String {
    let bit = |m: u32, c: char| if mode & m != 0 { c } else { '-' };
    let special = |exec: bool, flag: bool, lower: char, upper: char| match (exec, flag) {
        (true, true) => lower,
        (false, true) => upper,
        (true, false) => 'x',
        (false, false) => '-',
    };
    let s: String = [
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        special(mode & 0o100 != 0, mode & 0o4000 != 0, 's', 'S'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        special(mode & 0o010 != 0, mode & 0o2000 != 0, 's', 'S'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        special(mode & 0o001 != 0, mode & 0o1000 != 0, 't', 'T'),
    ]
    .into_iter()
    .collect();
    let perm = mode & 0o7777;
    if perm > 0o777 { format!("{s} ({perm:04o})") } else { format!("{s} ({perm:03o})") }
}

/// Recursively totals a directory: `(bytes, files, dirs)`, not counting
/// `path` itself. Symlinks are counted as files and never followed; hard
/// links are counted each time they appear. `on_progress(bytes, files, dirs)`
/// is called on the calling thread at most every 100 ms. Returns an error if
/// cancelled.
pub fn dir_totals(
    path: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64, u64),
) -> Result<(u64, u64, u64)> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    if !meta.is_dir() {
        return Ok((meta.len(), 1, 0));
    }
    let bytes = AtomicU64::new(0);
    let files = AtomicU64::new(0);
    let dirs = AtomicU64::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(12);

    let mut wb = WalkBuilder::new(path);
    wb.standard_filters(false).follow_links(false).threads(threads);
    let walker = wb.build_parallel();

    let (done_tx, done_rx) = mpsc::channel::<()>();
    let snapshot = || {
        (bytes.load(Ordering::Relaxed), files.load(Ordering::Relaxed), dirs.load(Ordering::Relaxed))
    };
    std::thread::scope(|s| {
        s.spawn(|| {
            walker.run(|| {
                Box::new(|res| {
                    if cancel.load(Ordering::Relaxed) {
                        return WalkState::Quit;
                    }
                    let Ok(dent) = res else { return WalkState::Continue };
                    if dent.depth() == 0 {
                        return WalkState::Continue;
                    }
                    if dent.file_type().is_some_and(|t| t.is_dir()) {
                        dirs.fetch_add(1, Ordering::Relaxed);
                    } else {
                        files.fetch_add(1, Ordering::Relaxed);
                        // On unix this is an lstat (links not followed).
                        if let Ok(m) = dent.metadata() {
                            bytes.fetch_add(m.len(), Ordering::Relaxed);
                        }
                    }
                    WalkState::Continue
                })
            });
            let _ = done_tx.send(());
        });
        while let Err(mpsc::RecvTimeoutError::Timeout) = done_rx.recv_timeout(PROGRESS_INTERVAL) {
            let (b, f, d) = snapshot();
            on_progress(b, f, d);
        }
    });
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled");
    }
    let totals = snapshot();
    on_progress(totals.0, totals.1, totals.2);
    Ok(totals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_file_props() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("hello.txt");
        fs::write(&f, "hello world").unwrap();
        let p = basic(&f).unwrap();
        assert_eq!(p.name, "hello.txt");
        assert_eq!(p.size, 11);
        assert!(!p.is_dir && !p.is_symlink);
        assert_eq!(p.kind_label, "TXT file");
        assert_eq!(p.mime.as_deref(), Some("text/plain"));
        assert!(p.modified.is_some());
        #[cfg(unix)]
        {
            assert!(p.permissions.as_ref().unwrap().ends_with(')'));
            assert!(p.owner.is_some() && p.group.is_some());
            assert!(p.allocated.is_some());
        }
    }

    #[test]
    fn mime_sniffing_beats_extension() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("image.txt");
        fs::write(&f, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        assert_eq!(basic(&f).unwrap().mime.as_deref(), Some("image/png"));
    }

    #[test]
    fn basic_dir_and_totals() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("Stuff");
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::write(d.join("one"), vec![0u8; 100]).unwrap();
        fs::write(d.join("a/two"), vec![0u8; 50]).unwrap();
        fs::write(d.join("a/b/three"), vec![0u8; 25]).unwrap();
        let mut p = basic(&d).unwrap();
        assert!(p.is_dir);
        assert_eq!(p.kind_label, "Folder");
        assert_eq!(p.mime.as_deref(), Some("inode/directory"));
        let mut calls = 0;
        let totals = dir_totals(&d, &AtomicBool::new(false), |_, _, _| calls += 1).unwrap();
        assert_eq!(totals, (175, 3, 2));
        assert!(calls >= 1);
        p.apply_totals(totals);
        assert_eq!(p.size, 175);
        assert!(dir_totals(&d, &AtomicBool::new(true), |_, _, _| {}).is_err());
        assert_eq!(dir_totals(&d.join("one"), &AtomicBool::new(false), |_, _, _| {}).unwrap(), (100, 1, 0));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_not_followed() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("target.txt");
        fs::write(&target, "12345").unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let p = basic(&link).unwrap();
        assert!(p.is_symlink);
        assert_eq!(p.symlink_target.as_deref(), Some(target.as_path()));
        assert_eq!(p.size, 5);
        assert_eq!(p.kind_label, "Alias to TXT file");
        std::os::unix::fs::symlink(t.path().join("nope"), t.path().join("broken")).unwrap();
        assert_eq!(basic(&t.path().join("broken")).unwrap().kind_label, "Broken link");

        // A link to a big directory must not be followed by dir_totals.
        let d = t.path().join("d");
        fs::create_dir(&d).unwrap();
        std::os::unix::fs::symlink(t.path(), d.join("loop")).unwrap();
        let (_, files, dirs) = dir_totals(&d, &AtomicBool::new(false), |_, _, _| {}).unwrap();
        assert_eq!((files, dirs), (1, 0));
    }

    #[test]
    fn mode_formatting() {
        assert_eq!(format_mode(0o755), "rwxr-xr-x (755)");
        assert_eq!(format_mode(0o644), "rw-r--r-- (644)");
        assert_eq!(format_mode(0o4755), "rwsr-xr-x (4755)");
        assert_eq!(format_mode(0o1777), "rwxrwxrwt (1777)");
        assert_eq!(format_mode(0o1666), "rw-rw-rwT (1666)");
    }
}
