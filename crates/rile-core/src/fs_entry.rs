use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use rayon::prelude::*;

/// Directories above this size get their entries stat'ed in parallel.
const PARALLEL_STAT_THRESHOLD: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: Arc<str>,
    /// Lowercased name, cached for sorting and type-ahead.
    pub name_lower: Arc<str>,
    pub kind: EntryKind,
    /// True when the entry (or a symlink target) is a directory.
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub hidden: bool,
}

impl Entry {
    pub fn from_path(path: PathBuf) -> Option<Self> {
        let name: Arc<str> = path.file_name()?.to_string_lossy().into();
        let meta = fs::symlink_metadata(&path).ok()?;
        let ft = meta.file_type();
        let (kind, target_meta) = if ft.is_symlink() {
            (EntryKind::Symlink, fs::metadata(&path).ok())
        } else if ft.is_dir() {
            (EntryKind::Dir, None)
        } else if ft.is_file() {
            (EntryKind::File, None)
        } else {
            (EntryKind::Other, None)
        };
        let effective = target_meta.as_ref().unwrap_or(&meta);
        let is_dir = effective.is_dir();
        Some(Self {
            name_lower: name.to_lowercase().into(),
            hidden: name.starts_with('.') || is_hidden_os(&meta),
            name,
            kind,
            is_dir,
            size: if is_dir { 0 } else { effective.len() },
            modified: effective.modified().ok(),
            path,
        })
    }

    /// An entry on a remote filesystem; `path` holds the remote path.
    pub fn remote(name: &str, path: &str, is_dir: bool, is_symlink: bool, size: u64, modified: Option<SystemTime>) -> Self {
        let name: Arc<str> = name.into();
        Self {
            path: PathBuf::from(path),
            name_lower: name.to_lowercase().into(),
            hidden: name.starts_with('.'),
            name,
            kind: if is_symlink { EntryKind::Symlink } else if is_dir { EntryKind::Dir } else { EntryKind::File },
            is_dir,
            size: if is_dir { 0 } else { size },
            modified,
        }
    }

    /// File extension in lowercase, empty for directories.
    pub fn extension(&self) -> String {
        if self.is_dir {
            return String::new();
        }
        self.path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }

    pub fn kind_label(&self) -> String {
        if self.is_dir {
            return "Folder".into();
        }
        match self.extension().as_str() {
            "" => "Document".into(),
            ext => format!("{} file", ext.to_uppercase()),
        }
    }
}

#[cfg(target_os = "macos")]
fn is_hidden_os(meta: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const UF_HIDDEN: u32 = 0x8000;
    meta.st_flags() & UF_HIDDEN != 0
}

#[cfg(not(target_os = "macos"))]
fn is_hidden_os(_meta: &fs::Metadata) -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

impl SortOrder {
    pub fn toggled(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

/// Reads a directory without recursing. Never indexes; cost is one
/// `readdir` plus one `lstat` per entry, parallelised for large dirs.
pub fn list_dir(dir: &Path, show_hidden: bool) -> std::io::Result<Vec<Entry>> {
    let paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    let mut entries: Vec<Entry> = if paths.len() > PARALLEL_STAT_THRESHOLD {
        paths.into_par_iter().filter_map(Entry::from_path).collect()
    } else {
        paths.into_iter().filter_map(Entry::from_path).collect()
    };
    if !show_hidden {
        entries.retain(|e| !e.hidden);
    }
    Ok(entries)
}

/// Sorts with folders first, then by key, using natural ordering for names.
pub fn sort_entries(entries: &mut [Entry], key: SortKey, order: SortOrder) {
    let cmp = |a: &Entry, b: &Entry| -> Ordering {
        let by_key = match key {
            SortKey::Name => Ordering::Equal,
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Kind => a.extension().cmp(&b.extension()),
        };
        let ord = by_key.then_with(|| natural_cmp(&a.name_lower, &b.name_lower));
        match order {
            SortOrder::Ascending => ord,
            SortOrder::Descending => ord.reverse(),
        }
    };
    let par = entries.len() > PARALLEL_STAT_THRESHOLD;
    let full = |a: &Entry, b: &Entry| b.is_dir.cmp(&a.is_dir).then_with(|| cmp(a, b));
    if par {
        entries.par_sort_unstable_by(full);
    } else {
        entries.sort_unstable_by(full);
    }
}

/// Compares strings treating runs of digits as numbers ("file2" < "file10").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let nb = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (da, db) = (trim_zeros(&a[..na]), trim_zeros(&b[..nb]));
                let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[na..];
                b = &b[nb..];
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let n = d.iter().take_while(|c| **c == b'0').count();
    &d[n.min(d.len().saturating_sub(1))..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "file2", "file1", "a", "file02x"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "file1", "file2", "file02x", "file10"]);
    }

    #[test]
    fn lists_and_sorts_dirs_first() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("b.txt"), "hello").unwrap();
        fs::write(tmp.path().join(".hidden"), "").unwrap();
        fs::create_dir(tmp.path().join("z_dir")).unwrap();
        let mut e = list_dir(tmp.path(), false).unwrap();
        sort_entries(&mut e, SortKey::Name, SortOrder::Ascending);
        let names: Vec<_> = e.iter().map(|e| e.name.to_string()).collect();
        assert_eq!(names, ["z_dir", "b.txt"]);
        assert_eq!(e[1].size, 5);
        assert_eq!(list_dir(tmp.path(), true).unwrap().len(), 3);
    }
}
