//! Discovery index: remembers the contents of folders the user has actually
//! opened, so a later search can answer from memory before (and while) the
//! disk walk runs. Nothing is crawled in the background — only listings the
//! explorer already read are recorded.
//!
//! Only names and the folder flag are stored; hits are re-checked against
//! the disk (one `stat` each) before they're shown, so the index never
//! returns files that were deleted or have changed size or date since.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rile_core::Entry;
use serde::{Deserialize, Serialize};

use crate::search::{KindFilter, NameMatcher, SearchQuery, meta_ok};

/// Remembered entries before the least recently seen folders are dropped.
const MAX_ENTRIES: usize = 500_000;
const FILE_NAME: &str = "discovered.json";
static REVISION: AtomicU64 = AtomicU64::new(0);

pub fn revision() -> u64 { REVISION.load(Ordering::Relaxed) }

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DirRecord {
    /// When the folder was last listed (seconds since the epoch).
    seen: u64,
    /// (name, is_dir) of every entry, hidden ones included.
    entries: Vec<(String, bool)>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DiscoveryIndex {
    dirs: HashMap<PathBuf, DirRecord>,
    #[serde(skip)]
    total: usize,
    #[serde(skip)]
    dirty: bool,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl DiscoveryIndex {
    pub fn load(path: &Path) -> Self {
        let mut index: Self = fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        index.total = index.dirs.values().map(|d| d.entries.len()).sum();
        index
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(self)?)?;
        fs::rename(tmp, path)?;
        self.dirty = false;
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Remembered folders and entries.
    pub fn stats(&self) -> (usize, usize) {
        (self.dirs.len(), self.total)
    }

    /// Remembered (files, folders) among all entries.
    pub fn counts(&self) -> (usize, usize) {
        let folders = self.dirs.values().flat_map(|d| &d.entries).filter(|(_, is_dir)| *is_dir).count();
        (self.total - folders, folders)
    }

    pub fn image_paths(&self) -> Vec<PathBuf> {
        self.dirs.iter().flat_map(|(dir, record)| {
            record.entries.iter().filter_map(move |(name, is_dir)| {
                let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
                (!is_dir && crate::gallery::IMAGE_EXTENSIONS.contains(&ext.as_str())).then(|| dir.join(name))
            })
        }).collect()
    }

    /// Names for semantic review, without a lexical filter or disk I/O.
    /// These paths may be stale; callers must verify model-selected hits.
    pub fn remembered_paths(&self, root: &Path) -> Vec<(PathBuf, bool)> {
        let mut paths = std::collections::BTreeMap::new();
        for (dir, record) in &self.dirs {
            let Ok(relative) = dir.strip_prefix(root) else { continue };
            if relative.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
                continue;
            }
            for (name, is_dir) in &record.entries {
                if !name.starts_with('.') {
                    paths.insert(dir.join(name), *is_dir);
                }
            }
        }
        paths.into_iter().collect()
    }

    pub fn clear(&mut self) {
        self.dirs.clear();
        self.total = 0;
        self.dirty = true;
    }

    /// Records (replaces) the listing of `dir`.
    pub fn record(&mut self, dir: &Path, entries: &[Entry]) {
        let items: Vec<(String, bool)> = entries.iter().map(|e| (e.name.to_string(), e.is_dir)).collect();
        if let Some(old) = self.dirs.get(dir)
            && old.entries == items
        {
            self.dirs.get_mut(dir).unwrap().seen = now_secs();
            return;
        }
        self.total += items.len();
        if let Some(old) = self.dirs.insert(dir.to_path_buf(), DirRecord { seen: now_secs(), entries: items }) {
            self.total -= old.entries.len();
        }
        self.dirty = true;
        self.evict();
    }

    /// Drops the least recently seen folders once over the size cap.
    fn evict(&mut self) {
        if self.total <= MAX_ENTRIES {
            return;
        }
        let mut by_age: Vec<(u64, PathBuf)> = self.dirs.iter().map(|(p, d)| (d.seen, p.clone())).collect();
        by_age.sort();
        for (_, path) in by_age {
            if self.total <= MAX_ENTRIES * 9 / 10 {
                break;
            }
            if let Some(d) = self.dirs.remove(&path) {
                self.total -= d.entries.len();
            }
        }
    }

    /// Remembered entries under `q.root` matching the query's name, kind,
    /// extension, depth, size and date filters, verified on disk. Content
    /// queries can't be answered from names, so they return nothing.
    pub fn query(&self, q: &SearchQuery) -> Vec<Entry> {
        if q.content.is_some() {
            return Vec::new();
        }
        let Ok(name) = NameMatcher::new(&q.name, q.case_sensitive) else { return Vec::new() };
        let exts: Vec<String> = q.extensions.iter().map(|e| e.trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()).collect();
        let max = if q.max_results == 0 { usize::MAX } else { q.max_results };
        let mut hits = Vec::new();
        for (dir, record) in &self.dirs {
            let Ok(rel) = dir.strip_prefix(&q.root) else { continue };
            // Entries of `dir` sit one level below it.
            let depth = rel.components().count() + 1;
            if q.max_depth.is_some_and(|m| depth > m) {
                continue;
            }
            if rel.components().any(|c| {
                let c = c.as_os_str().to_string_lossy();
                c == ".git" || (!q.include_hidden && c.starts_with('.'))
            }) {
                continue;
            }
            for (fname, is_dir) in &record.entries {
                match q.kind {
                    KindFilter::Files if *is_dir => continue,
                    KindFilter::Dirs if !is_dir => continue,
                    _ => {}
                }
                if !q.include_hidden && fname.starts_with('.') {
                    continue;
                }
                if !exts.is_empty() {
                    let ext = Path::new(fname).extension().map(|e| e.to_string_lossy().to_lowercase());
                    if *is_dir || !ext.is_some_and(|e| exts.contains(&e)) {
                        continue;
                    }
                }
                if !name.is_match(fname) {
                    continue;
                }
                // Verify on disk and refresh the metadata.
                let Some(entry) = Entry::from_path(dir.join(fname)) else { continue };
                if entry.is_dir != *is_dir || (!q.include_hidden && entry.hidden) || !meta_ok(q, &entry) {
                    continue;
                }
                hits.push(entry);
                if hits.len() >= max {
                    return hits;
                }
            }
        }
        hits
    }
}

struct Global {
    index: DiscoveryIndex,
    path: Option<PathBuf>,
    enabled: bool,
}

fn global() -> &'static Mutex<Global> {
    static GLOBAL: OnceLock<Mutex<Global>> = OnceLock::new();
    GLOBAL.get_or_init(|| {
        let path = rile_core::settings::Settings::config_dir().map(|d| d.join(FILE_NAME));
        let index = path.as_deref().map(DiscoveryIndex::load).unwrap_or_default();
        Mutex::new(Global { index, path, enabled: true })
    })
}

fn lock() -> MutexGuard<'static, Global> {
    global().lock().unwrap_or_else(|e| e.into_inner())
}

/// Turns recording and lookups on or off (the user's setting).
pub fn set_enabled(enabled: bool) {
    lock().enabled = enabled;
}

pub fn is_enabled() -> bool {
    lock().enabled
}

/// Records a local folder listing the explorer just read.
pub fn record(dir: &Path, entries: &[Entry]) {
    let mut g = lock();
    if g.enabled {
        g.index.record(dir, entries);
        REVISION.fetch_add(1, Ordering::Relaxed);
    }
}

/// Remembered matches for a query (see [`DiscoveryIndex::query`]). The
/// lock is held only to copy out candidates, not while verifying them.
pub fn query(q: &SearchQuery) -> Vec<Entry> {
    let snapshot: Vec<(PathBuf, DirRecord)> = {
        let g = lock();
        if !g.enabled {
            return Vec::new();
        }
        g.index.dirs.iter().filter(|(p, _)| p.starts_with(&q.root)).map(|(p, d)| (p.clone(), d.clone())).collect()
    };
    let mut scoped = DiscoveryIndex::default();
    scoped.dirs = snapshot.into_iter().collect();
    scoped.query(q)
}

/// A name-only snapshot for AI review; disabled memory stays inaccessible.
/// The index lock is released before any model calls or disk verification.
pub fn remembered_paths(root: &Path) -> Vec<(PathBuf, bool)> {
    let g = lock();
    if !g.enabled {
        return Vec::new();
    }
    g.index.remembered_paths(root)
}

/// Writes the index to disk if it changed.
pub fn save() -> Result<()> {
    let mut g = lock();
    let Some(path) = g.path.clone() else { return Ok(()) };
    if g.index.is_dirty() { g.index.save(&path) } else { Ok(()) }
}

/// Forgets everything (and removes the file).
pub fn clear() {
    let mut g = lock();
    g.index.clear();
    REVISION.fetch_add(1, Ordering::Relaxed);
    if let Some(path) = g.path.clone() {
        fs::remove_file(path).ok();
    }
    g.index.dirty = false;
}

/// (folders, entries) remembered.
pub fn stats() -> (usize, usize) {
    lock().index.stats()
}

/// (files, folders) remembered, counted over every entry (not instant on
/// a large index: call from a background thread).
pub fn counts() -> (usize, usize) {
    lock().index.counts()
}

pub fn image_paths() -> Vec<PathBuf> {
    lock().index.image_paths()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::NameMatch;

    #[test]
    fn phrase_search_matches_the_same_names_in_memory_and_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("professor_layton.nds"), "").unwrap();
        fs::write(tmp.path().join("unrelated.pdf"), "").unwrap();
        let mut index = DiscoveryIndex::default();
        index.record(tmp.path(), &rile_core::list_dir(tmp.path(), false).unwrap());
        let mut query = SearchQuery::new(tmp.path());
        query.name = NameMatch::Substring("Professor Layton".into());
        let hits = index.query(&query);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name.as_ref(), "professor_layton.nds");
    }

    #[test]
    fn discovered_photo_paths_exclude_folders_and_non_images() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("folder.png")).unwrap();
        for name in ["photo.JPG", "raw.dng", "notes.txt", "no-extension"] {
            fs::write(root.join(name), "x").unwrap();
        }
        let mut index = DiscoveryIndex::default();
        index.record(root, &rile_core::list_dir(root, true).unwrap());
        let mut paths = index.image_paths();
        paths.sort();
        assert_eq!(paths, [root.join("photo.JPG"), root.join("raw.dng")]);
    }

    #[test]
    fn semantic_snapshot_includes_nonliteral_names_without_crawling() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("Business/Ausgangsrechnungen")).unwrap();
        fs::create_dir(root.join(".private")).unwrap();
        fs::write(root.join("Business/Rechnung.pdf"), "").unwrap();
        fs::write(root.join("Business/.secret"), "").unwrap();
        fs::write(root.join(".private/hidden.txt"), "").unwrap();
        let mut index = DiscoveryIndex::default();
        for dir in [root.to_path_buf(), root.join("Business"), root.join(".private")] {
            index.record(&dir, &rile_core::list_dir(&dir, true).unwrap());
        }
        // An unrecorded listing must not be discovered by snapshotting.
        fs::write(root.join("Business/Ausgangsrechnungen/unseen.pdf"), "").unwrap();
        fs::remove_file(root.join("Business/Rechnung.pdf")).unwrap();
        let paths = index.remembered_paths(&root.join("Business"));
        assert_eq!(paths, [
            (root.join("Business/Ausgangsrechnungen"), true),
            (root.join("Business/Rechnung.pdf"), false),
        ]);
        assert!(!index.remembered_paths(root).iter().any(|(p, _)| p.components().any(|c| c.as_os_str() == ".private")));
        let mut literal = SearchQuery::new(root);
        literal.name = NameMatch::Substring("invoice".into());
        assert!(index.query(&literal).is_empty());
    }

    #[test]
    fn records_queries_verifies_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/report.pdf"), "x").unwrap();
        fs::write(root.join("a/b/report-old.pdf"), "xy").unwrap();
        fs::write(root.join("a/b/.hidden-report"), "").unwrap();
        let mut index = DiscoveryIndex::default();
        for dir in [root.clone(), root.join("a"), root.join("a/b")] {
            index.record(&dir, &rile_core::list_dir(&dir, true).unwrap());
        }
        assert_eq!(index.stats(), (3, 5));

        let mut q = SearchQuery::new(&root);
        q.name = NameMatch::Substring("REPORT".into());
        let mut names: Vec<String> = index.query(&q).into_iter().map(|e| e.name.to_string()).collect();
        names.sort();
        assert_eq!(names, ["report-old.pdf", "report.pdf"]);

        // Depth, kind and hidden filters.
        q.max_depth = Some(2);
        assert_eq!(index.query(&q).len(), 1);
        q.max_depth = None;
        q.include_hidden = true;
        assert_eq!(index.query(&q).len(), 3);
        q.kind = KindFilter::Dirs;
        assert!(index.query(&q).is_empty());

        // Deleted files are never returned; content queries are not answered.
        fs::remove_file(root.join("a/report.pdf")).unwrap();
        q.kind = KindFilter::Any;
        q.include_hidden = false;
        assert_eq!(index.query(&q).len(), 1);
        q.content = Some(crate::search::ContentMatch { pattern: "x".into(), regex: false, case_sensitive: false });
        assert!(index.query(&q).is_empty());

        // Round-trips through disk.
        let file = root.join("idx/discovered.json");
        index.save(&file).unwrap();
        assert_eq!(DiscoveryIndex::load(&file).stats(), (3, 5));
    }
}
