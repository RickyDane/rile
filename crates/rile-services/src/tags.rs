//! Smart tags: AI-generated tags, a short description and extracted text
//! (OCR or an excerpt) for images and documents, keyed by absolute path and
//! persisted as JSON in the config dir. Records carry the file's mtime and
//! size so callers can tell when a file needs re-tagging ([`is_fresh`]).
//!
//! [`search`] answers loose natural-language queries ("dog on the beach",
//! "tax receipts pdf") with a simple weighted token match: tags count 3,
//! description and file name 2, extracted text 1.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "smart-tags.json";
static REVISION: AtomicU64 = AtomicU64::new(0);

pub fn revision() -> u64 { REVISION.load(Ordering::Relaxed) }
/// Extracted text kept per record, in bytes (cut on a char boundary).
pub const MAX_TEXT: usize = 4096;

const W_TAG: f32 = 3.0;
const W_DESC: f32 = 2.0;
const W_NAME: f32 = 2.0;
const W_TEXT: f32 = 1.0;
/// Score multiplier for records of the kind the query asks for ("images of ...").
const KIND_BOOST: f32 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TagKind {
    Image,
    Document,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagRecord {
    pub tags: Vec<String>,
    pub description: String,
    /// OCR output or a text excerpt, capped at [`MAX_TEXT`] bytes.
    pub text: String,
    /// Modification time of the file when tagged (seconds since the epoch).
    pub mtime: u64,
    pub size: u64,
    pub kind: TagKind,
}

impl TagRecord {
    fn has_content(&self) -> bool {
        self.tags.iter().any(|tag| !tag.trim().is_empty()) || !self.description.trim().is_empty() || !self.text.trim().is_empty()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TagStore {
    records: HashMap<PathBuf, TagRecord>,
    #[serde(skip)]
    dirty: bool,
}

/// Words that carry no meaning for matching.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "with", "my", "in", "on", "at", "and", "or", "for", "to", "from", "by", "is", "are", "that", "this",
    "some", "any", "all", "me", "show", "find", "containing", "contains", "file", "files", "about",
];
const IMAGE_WORDS: &[&str] = &["image", "images", "photo", "photos", "picture", "pictures", "pic", "pics", "img"];
const DOCUMENT_WORDS: &[&str] = &["document", "documents", "doc", "docs"];

/// Light stemming so "dogs"/"dog", "boxes"/"box", "running"/"run" meet.
fn stem(word: &str) -> String {
    let w = word;
    let n = w.len();
    if n > 4 && w.ends_with("ies") {
        format!("{}y", &w[..n - 3])
    } else if n > 5 && w.ends_with("ing") {
        let base = &w[..n - 3];
        let b = base.as_bytes();
        // running -> runn -> run
        if b.len() >= 3 && b[b.len() - 1].is_ascii_alphabetic() && b[b.len() - 1] == b[b.len() - 2] && !b"aeiouls".contains(&b[b.len() - 1]) {
            base[..base.len() - 1].to_string()
        } else {
            base.to_string()
        }
    } else if n > 3 && (w.ends_with("ches") || w.ends_with("shes") || w.ends_with("xes") || w.ends_with("zes") || w.ends_with("sses")) {
        w[..n - 2].to_string()
    } else if n > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") {
        w[..n - 1].to_string()
    } else {
        w.to_string()
    }
}

/// Lowercased, stemmed alphanumeric tokens of `s`.
fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| stem(&t.to_lowercase())).collect()
}

struct Query {
    terms: Vec<String>,
    /// Kind implied by words like "photos" or "documents".
    kind: Option<TagKind>,
}

fn parse_query(q: &str) -> Query {
    let mut terms = Vec::new();
    let mut kind = None;
    for raw in q.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()) {
        let w = raw.to_lowercase();
        if IMAGE_WORDS.contains(&w.as_str()) {
            kind = Some(TagKind::Image);
        } else if DOCUMENT_WORDS.contains(&w.as_str()) {
            kind = Some(TagKind::Document);
        } else if !STOPWORDS.contains(&w.as_str()) {
            let t = stem(&w);
            if !terms.contains(&t) {
                terms.push(t);
            }
        }
    }
    Query { terms, kind }
}

/// 1.0 for an exact token match, 0.5 for a prefix match of a 3+ char term.
fn field_match(term: &str, field: &[String]) -> f32 {
    let mut best = 0.0f32;
    for t in field {
        if t == term {
            return 1.0;
        }
        if term.len() >= 3 && t.starts_with(term) {
            best = 0.5;
        }
    }
    best
}

fn score(q: &Query, path: &Path, r: &TagRecord) -> f32 {
    let kind_boost = if q.kind == Some(r.kind) { KIND_BOOST } else { 1.0 };
    if q.terms.is_empty() {
        // Only kind words (e.g. "photos"): everything of that kind.
        return if q.kind == Some(r.kind) { 1.0 } else { 0.0 };
    }
    let tags: Vec<String> = r.tags.iter().flat_map(|t| tokens(t)).collect();
    let desc = tokens(&r.description);
    let text = tokens(&r.text);
    let name = path.file_stem().map(|n| tokens(&n.to_string_lossy())).unwrap_or_default();
    let mut total = 0.0;
    let mut matched = 0;
    for term in &q.terms {
        let s = W_TAG * field_match(term, &tags)
            + W_DESC * field_match(term, &desc)
            + W_NAME * field_match(term, &name)
            + W_TEXT * field_match(term, &text);
        if s > 0.0 {
            matched += 1;
            total += s;
        }
    }
    if matched == 0 {
        return 0.0;
    }
    // Favor records that cover more of the query.
    total * matched as f32 / q.terms.len() as f32 * kind_boost
}

fn cap_text(mut s: String) -> String {
    if s.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

impl TagStore {
    pub fn load(path: &Path) -> Self {
        fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
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

    /// Number of tagged files.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Successful smart-tag records only; empty retry/failure markers do not
    /// count toward trophies. Count both kinds in one pass without cloning paths.
    pub fn reward_counts(&self) -> (usize, usize) {
        let mut counts = (0, 0);
        for record in self.records.values() {
            if !record.has_content() {
                continue;
            }
            match record.kind {
                TagKind::Image => counts.0 += 1,
                TagKind::Document => counts.1 += 1,
            }
        }
        counts
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.dirty = true;
    }

    pub fn get(&self, path: &Path) -> Option<&TagRecord> {
        self.records.get(path)
    }

    /// Stores (replaces) the record for `path`, capping its text.
    pub fn put(&mut self, path: &Path, mut record: TagRecord) {
        record.text = cap_text(record.text);
        self.records.insert(path.to_path_buf(), record);
        self.dirty = true;
    }

    /// True if `path` has a record taken at this mtime and size.
    pub fn is_fresh(&self, path: &Path, mtime: u64, size: u64) -> bool {
        self.records.get(path).is_some_and(|r| r.mtime == mtime && r.size == size)
    }

    pub fn remove(&mut self, path: &Path) -> Option<TagRecord> {
        let old = self.records.remove(path);
        self.dirty |= old.is_some();
        old
    }

    /// Paths whose records match `query`, best first. Kind words in the
    /// query ("photos", "documents") boost that kind; `kind` filters to one.
    /// Scores are relative (higher is better), not normalized.
    pub fn search(&self, query: &str, kind: Option<TagKind>, limit: usize) -> Vec<(PathBuf, f32)> {
        let q = parse_query(query);
        if q.terms.is_empty() && q.kind.is_none() {
            return Vec::new();
        }
        let mut hits: Vec<(PathBuf, f32)> = self
            .records
            .iter()
            .filter(|(_, r)| kind.is_none_or(|k| r.kind == k))
            .map(|(p, r)| (p.clone(), score(&q, p, r)))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if limit > 0 {
            hits.truncate(limit);
        }
        hits
    }

    pub fn paths_with_kind(&self, kind: TagKind) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.records.iter().filter(|(_, r)| r.kind == kind).map(|(p, _)| p.clone()).collect();
        paths.sort();
        paths
    }
}

struct Global {
    store: TagStore,
    path: Option<PathBuf>,
}

fn global() -> &'static Mutex<Global> {
    static GLOBAL: OnceLock<Mutex<Global>> = OnceLock::new();
    GLOBAL.get_or_init(|| {
        let path = rile_core::settings::Settings::config_dir().map(|d| d.join(FILE_NAME));
        let store = path.as_deref().map(TagStore::load).unwrap_or_default();
        Mutex::new(Global { store, path })
    })
}

fn lock() -> MutexGuard<'static, Global> {
    global().lock().unwrap_or_else(|e| e.into_inner())
}

pub fn get(path: &Path) -> Option<TagRecord> {
    lock().store.get(path).cloned()
}

pub fn put(path: &Path, record: TagRecord) {
    lock().store.put(path, record);
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// True if `path` was tagged at this mtime (seconds) and size.
pub fn is_fresh(path: &Path, mtime: u64, size: u64) -> bool {
    lock().store.is_fresh(path, mtime, size)
}

pub fn remove(path: &Path) {
    lock().store.remove(path);
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// Number of tagged files.
pub fn stats() -> usize {
    lock().store.len()
}

pub fn reward_counts() -> (usize, usize) {
    lock().store.reward_counts()
}

/// Coverage uses the intersection, not all tagged images divided by an
/// unrelated total. An empty discovered library earns no percentage badges.
pub fn photo_coverage(paths: &[PathBuf]) -> usize {
    let g = lock();
    coverage(&g.store, paths)
}

fn coverage(store: &TagStore, paths: &[PathBuf]) -> usize {
    if paths.is_empty() { return 0; }
    let tagged = paths.iter().filter(|path| store.get(path).is_some_and(|record| record.kind == TagKind::Image && record.has_content())).count();
    ((tagged as u128 * 100) / paths.len() as u128) as usize
}

/// Matches for a natural-language query (see [`TagStore::search`]), any kind.
/// Files that no longer exist are skipped; `limit` 0 means unlimited.
pub fn search(query: &str, limit: usize) -> Vec<(PathBuf, f32)> {
    search_kind(query, None, limit)
}

/// Like [`search`], restricted to one kind when `kind` is set.
pub fn search_kind(query: &str, kind: Option<TagKind>, limit: usize) -> Vec<(PathBuf, f32)> {
    let hits = lock().store.search(query, kind, 0);
    let mut out: Vec<(PathBuf, f32)> = hits.into_iter().filter(|(p, _)| p.exists()).collect();
    if limit > 0 {
        out.truncate(limit);
    }
    out
}

/// Number of records of `kind` (cheap: no paths are collected).
pub fn count_kind(kind: TagKind) -> usize {
    lock().store.records.values().filter(|r| r.kind == kind).count()
}

pub fn paths_with_kind(kind: TagKind) -> Vec<PathBuf> {
    lock().store.paths_with_kind(kind)
}

/// Writes the store to disk if it changed.
pub fn save() -> Result<()> {
    let mut g = lock();
    let Some(path) = g.path.clone() else { return Ok(()) };
    if g.store.is_dirty() { g.store.save(&path) } else { Ok(()) }
}

/// Forgets everything (and removes the file).
pub fn clear() {
    let mut g = lock();
    g.store.clear();
    REVISION.fetch_add(1, Ordering::Relaxed);
    if let Some(path) = g.path.clone() {
        fs::remove_file(path).ok();
    }
    g.store.dirty = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: TagKind, tags: &[&str], description: &str, text: &str) -> TagRecord {
        TagRecord {
            tags: tags.iter().map(|t| t.to_string()).collect(),
            description: description.into(),
            text: text.into(),
            mtime: 100,
            size: 10,
            kind,
        }
    }

    fn store() -> TagStore {
        let mut s = TagStore::default();
        s.put(Path::new("/p/IMG_001.jpg"), rec(TagKind::Image, &["dog", "beach", "golden retriever"], "A dog running on a sandy beach", ""));
        s.put(Path::new("/p/IMG_002.jpg"), rec(TagKind::Image, &["cat", "sofa"], "A cat sleeping on a sofa", ""));
        s.put(Path::new("/p/receipt.png"), rec(TagKind::Image, &["receipt", "text"], "A store receipt", "TOTAL 12.99 coffee shop"));
        s.put(Path::new("/d/taxes-2024.pdf"), rec(TagKind::Document, &["tax", "invoice"], "Tax return form", "Income tax return for 2024"));
        s.put(Path::new("/d/beach-trip.pdf"), rec(TagKind::Document, &["travel", "itinerary"], "Itinerary for a beach trip", "Flights and hotels"));
        s
    }

    #[test]
    fn reward_counts_ignore_empty_failure_markers() {
        let mut s = store();
        s.put(Path::new("/p/failed.jpg"), rec(TagKind::Image, &[], "", ""));
        s.put(Path::new("/d/failed.pdf"), rec(TagKind::Document, &[], " ", "\n"));
        assert_eq!(s.reward_counts(), (3, 2));
        s.put(Path::new("/p/failed.jpg"), rec(TagKind::Image, &[], "A landscape", ""));
        assert_eq!(s.reward_counts(), (4, 2));
        s.put(Path::new("/d/ocr.pdf"), rec(TagKind::Document, &[], "", "Recognized text"));
        assert_eq!(s.reward_counts(), (4, 3));
        s.clear();
        assert_eq!(s.reward_counts(), (0, 0));
    }

    #[test]
    fn photo_coverage_counts_only_successful_discovered_images() {
        let mut s = store();
        assert_eq!(coverage(&s, &[]), 0);
        let paths = ["/p/IMG_001.jpg", "/p/missing.jpg", "/p/failed.jpg", "/p/other.jpg"].map(PathBuf::from);
        s.put(&paths[2], rec(TagKind::Image, &[], "", ""));
        // A document stored at an image path must not count as a tagged photo.
        s.put(&paths[3], rec(TagKind::Document, &["document"], "", ""));
        assert_eq!(coverage(&s, &paths), 25);
        s.put(&paths[2], rec(TagKind::Image, &[], "Landscape", ""));
        assert_eq!(coverage(&s, &paths), 50);
        assert_eq!(coverage(&s, &paths[..1]), 100);
    }

    #[test]
    fn stems_and_parses() {
        assert_eq!(stem("dogs"), "dog");
        assert_eq!(stem("boxes"), "box");
        assert_eq!(stem("berries"), "berry");
        assert_eq!(stem("running"), "run");
        assert_eq!(stem("glass"), "glass");
        let q = parse_query("Photos of my dogs at the beach");
        assert_eq!(q.terms, ["dog", "beach"]);
        assert_eq!(q.kind, Some(TagKind::Image));
        assert_eq!(parse_query("the files").terms.len(), 0);
    }

    #[test]
    fn searches_by_weighted_fields() {
        let s = store();
        let hits = s.search("photos of dogs at the beach", None, 10);
        assert_eq!(hits[0].0, Path::new("/p/IMG_001.jpg"));
        // The beach itinerary still matches, but ranks below the photo.
        assert!(hits.iter().any(|(p, _)| p == Path::new("/d/beach-trip.pdf")));

        // Text (OCR) and file-name matches.
        assert_eq!(s.search("coffee", None, 10)[0].0, Path::new("/p/receipt.png"));
        assert_eq!(s.search("taxes", None, 10)[0].0, Path::new("/d/taxes-2024.pdf"));

        // Kind word boosts that kind; the explicit filter restricts.
        assert_eq!(s.search("beach documents", None, 10)[0].0, Path::new("/d/beach-trip.pdf"));
        let docs = s.search("beach", Some(TagKind::Document), 10);
        assert_eq!(docs.len(), 1);
        assert_eq!(s.search("images", None, 0).len(), 3);
        assert!(s.search("zebra", None, 10).is_empty());
        assert!(s.search("the of", None, 10).is_empty());
        assert_eq!(s.search("a", None, 1).len(), 0);
        assert_eq!(s.search("sleeping", None, 1)[0].0, Path::new("/p/IMG_002.jpg"));
    }

    #[test]
    fn photo_search_matches_secondary_tags_and_license_plate_text() {
        let mut s = TagStore::default();
        let path = Path::new("/p/06db9039-45a1-42dd-9542-c73f88f8d249.jpg");
        s.put(path, rec(TagKind::Image, &["outdoor", "sky", "cloudy", "automobile", "vehicle", "car"], "Photo of outdoor, sky and cloudy", "WAF&RA 777"));
        for query in ["outdoor", "car", "777"] {
            let hits = s.search(query, Some(TagKind::Image), 0);
            assert_eq!(hits.len(), 1, "{query}");
            assert_eq!(hits[0].0, path, "{query}");
        }
    }

    #[test]
    fn freshness_remove_kind_and_persist() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store();
        assert!(s.is_dirty());
        assert!(s.is_fresh(Path::new("/p/IMG_001.jpg"), 100, 10));
        assert!(!s.is_fresh(Path::new("/p/IMG_001.jpg"), 101, 10));
        assert!(!s.is_fresh(Path::new("/nope"), 100, 10));
        assert_eq!(s.paths_with_kind(TagKind::Document).len(), 2);

        let file = tmp.path().join("cfg/smart-tags.json");
        s.save(&file).unwrap();
        assert!(!s.is_dirty());
        let mut loaded = TagStore::load(&file);
        assert_eq!(loaded.len(), 5);
        assert_eq!(loaded.get(Path::new("/p/IMG_002.jpg")).unwrap().tags, ["cat", "sofa"]);

        assert!(loaded.remove(Path::new("/p/IMG_002.jpg")).is_some());
        assert!(loaded.is_dirty());
        assert_eq!(loaded.len(), 4);
        loaded.clear();
        assert!(loaded.is_empty());

        // Text is capped on a char boundary.
        let mut big = rec(TagKind::Document, &[], "", &"é".repeat(MAX_TEXT));
        big.size = 1;
        loaded.put(Path::new("/x"), big);
        assert!(loaded.get(Path::new("/x")).unwrap().text.len() <= MAX_TEXT);
    }
}
