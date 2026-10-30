//! The operation journal behind Undo/Redo and the searchable history.
//!
//! Every change Rile makes to local files is recorded as a [`Record`]: a
//! label plus the reversible steps it took ([`Change`]). Undo applies the
//! inverse of each step in reverse order; redo applies them again. Steps are
//! deliberately few and always reversible without keeping copies of data:
//! renames/moves are moved back, created items go to the trash (and come
//! back from it on redo), trashed items are put back from the trash.
//! Permanent deletions are recorded for the history but can't be undone.
//!
//! The journal persists in `<config>/journal.json` (newest first, bounded).
//! File system work runs on the caller's thread: call [`undo`]/[`redo`]
//! from a background thread.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rile_core::ops;
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "journal.json";
const MAX_RECORDS: usize = 400;

/// One reversible step of an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    /// `from` was renamed or moved to `to`.
    Moved { from: PathBuf, to: PathBuf },
    /// `path` was created (copied, extracted, new folder…). While undone it
    /// sits in the trash at `trashed` (when the platform reports it).
    Created {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trashed: Option<PathBuf>,
    },
    /// `path` went to the trash, which put it at `trashed` (when known).
    Trashed {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trashed: Option<PathBuf>,
    },
    /// `path` was deleted permanently; recorded only for the history.
    Deleted { path: PathBuf },
}

impl Change {
    /// The path this step is about, as the user knows it.
    pub fn path(&self) -> &Path {
        match self {
            Change::Moved { to, .. } => to,
            Change::Created { path, .. } | Change::Trashed { path, .. } | Change::Deleted { path } => path,
        }
    }
}

/// What kind of operation a record is (for icons and filtering).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    Copy,
    Move,
    Rename,
    BatchRename,
    Duplicate,
    NewFolder,
    NewFile,
    Trash,
    Delete,
    Compress,
    Extract,
    Sync,
    Other,
}

impl OpKind {
    pub fn label(self) -> &'static str {
        match self {
            OpKind::Copy => "Copy",
            OpKind::Move => "Move",
            OpKind::Rename => "Rename",
            OpKind::BatchRename => "Batch Rename",
            OpKind::Duplicate => "Duplicate",
            OpKind::NewFolder => "New Folder",
            OpKind::NewFile => "New File",
            OpKind::Trash => "Move to Trash",
            OpKind::Delete => "Delete",
            OpKind::Compress => "Compress",
            OpKind::Extract => "Extract",
            OpKind::Sync => "Sync",
            OpKind::Other => "Change",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Done,
    Undone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: u64,
    /// Unix seconds.
    pub time: i64,
    pub kind: OpKind,
    /// Past tense, e.g. "Copied 3 items to Documents".
    pub label: String,
    #[serde(default)]
    pub changes: Vec<Change>,
    #[serde(default)]
    pub state: State,
}

impl Record {
    /// Whether every step can be reversed.
    pub fn undoable(&self) -> bool {
        !self.changes.is_empty() && !self.changes.iter().any(|c| matches!(c, Change::Deleted { .. }))
    }

    /// Whether this record matches a history search (label, kind or any path).
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        q.split_whitespace().all(|word| {
            self.label.to_lowercase().contains(word)
                || self.kind.label().to_lowercase().contains(word)
                || self.changes.iter().any(|c| {
                    let hit = |p: &Path| p.to_string_lossy().to_lowercase().contains(word);
                    match c {
                        Change::Moved { from, to } => hit(from) || hit(to),
                        other => hit(other.path()),
                    }
                })
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Journal {
    #[serde(default)]
    next_id: u64,
    /// Newest first.
    #[serde(default)]
    records: Vec<Record>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Journal {
    fn load() -> Self {
        let path = rile_core::settings::Settings::config_dir().map(|d| d.join(FILE_NAME));
        let mut j: Journal = path
            .as_ref()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        j.path = path;
        j
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).ok();
        }
        if let Ok(bytes) = serde_json::to_vec(self) {
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, bytes).is_ok() {
                fs::rename(&tmp, path).ok();
            }
        }
    }
}

fn journal() -> &'static Mutex<Journal> {
    static J: OnceLock<Mutex<Journal>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(Journal::load()))
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Records a finished operation; returns its id (None when nothing changed).
pub fn record(kind: OpKind, label: impl Into<String>, changes: Vec<Change>) -> Option<u64> {
    if changes.is_empty() {
        return None;
    }
    let mut j = journal().lock().unwrap();
    j.next_id += 1;
    let id = j.next_id;
    j.records.insert(0, Record { id, time: now(), kind, label: label.into(), changes, state: State::Done });
    j.records.truncate(MAX_RECORDS);
    j.save();
    Some(id)
}

/// All records, newest first.
pub fn list() -> Vec<Record> {
    journal().lock().unwrap().records.clone()
}

pub fn get(id: u64) -> Option<Record> {
    journal().lock().unwrap().records.iter().find(|r| r.id == id).cloned()
}

/// Forgets the whole history.
pub fn clear() {
    let mut j = journal().lock().unwrap();
    j.records.clear();
    j.save();
}

/// The record ⌘Z would undo: the newest one that is done and undoable.
pub fn undo_candidate() -> Option<Record> {
    journal().lock().unwrap().records.iter().find(|r| r.state == State::Done && r.undoable()).cloned()
}

/// The record ⌘⇧Z would redo: of the undone records on top of the history
/// (no newer operation done since), the one undone last — the oldest.
pub fn redo_candidate() -> Option<Record> {
    let j = journal().lock().unwrap();
    j.records.iter().take_while(|r| r.state == State::Undone || !r.undoable()).filter(|r| r.state == State::Undone).last().cloned()
}

/// Reverses record `id`. Returns the paths that are back (for selecting).
pub fn undo(id: u64) -> Result<Vec<PathBuf>> {
    let mut rec = get(id).context("That operation is no longer in the history")?;
    if rec.state == State::Undone {
        bail!("\"{}\" was already undone", rec.label);
    }
    if !rec.undoable() {
        bail!("\"{}\" can't be undone", rec.label);
    }
    let mut back = Vec::new();
    let mut done = Vec::new();
    for i in (0..rec.changes.len()).rev() {
        match revert(&mut rec.changes[i]) {
            Ok(p) => {
                back.extend(p);
                done.push(i);
            }
            Err(e) => {
                // Put already reverted steps back so the files stay consistent.
                for &j in done.iter().rev() {
                    apply(&mut rec.changes[j]).ok();
                }
                store(&rec);
                return Err(e).with_context(|| format!("Couldn't undo \"{}\"", rec.label));
            }
        }
    }
    rec.state = State::Undone;
    store(&rec);
    Ok(back)
}

/// Applies an undone record `id` again. Returns the affected paths.
pub fn redo(id: u64) -> Result<Vec<PathBuf>> {
    let mut rec = get(id).context("That operation is no longer in the history")?;
    if rec.state == State::Done {
        bail!("\"{}\" isn't undone", rec.label);
    }
    let mut out = Vec::new();
    let mut done = Vec::new();
    for i in 0..rec.changes.len() {
        match apply(&mut rec.changes[i]) {
            Ok(p) => {
                out.extend(p);
                done.push(i);
            }
            Err(e) => {
                for &j in done.iter().rev() {
                    revert(&mut rec.changes[j]).ok();
                }
                store(&rec);
                return Err(e).with_context(|| format!("Couldn't redo \"{}\"", rec.label));
            }
        }
    }
    rec.state = State::Done;
    store(&rec);
    Ok(out)
}

fn store(rec: &Record) {
    let mut j = journal().lock().unwrap();
    if let Some(r) = j.records.iter_mut().find(|r| r.id == rec.id) {
        *r = rec.clone();
        j.save();
    }
}

/// Undoes one step; returns the path that exists again, if any.
fn revert(change: &mut Change) -> Result<Option<PathBuf>> {
    match change {
        Change::Moved { from, to } => {
            move_path(to, from)?;
            Ok(Some(from.clone()))
        }
        Change::Created { path, trashed } => {
            *trashed = trash_one(path)?;
            Ok(None)
        }
        Change::Trashed { path, trashed } => {
            restore(path, trashed.as_deref())?;
            *trashed = None;
            Ok(Some(path.clone()))
        }
        Change::Deleted { path } => bail!("\"{}\" was deleted permanently", path.display()),
    }
}

/// Redoes one step.
fn apply(change: &mut Change) -> Result<Option<PathBuf>> {
    match change {
        Change::Moved { from, to } => {
            move_path(from, to)?;
            Ok(Some(to.clone()))
        }
        Change::Created { path, trashed } => {
            restore(path, trashed.as_deref())?;
            *trashed = None;
            Ok(Some(path.clone()))
        }
        Change::Trashed { path, trashed } => {
            *trashed = trash_one(path)?;
            Ok(None)
        }
        Change::Deleted { path } => bail!("\"{}\" was deleted permanently", path.display()),
    }
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// Moves `src` to exactly `dest` (which must not exist), across volumes too.
pub fn move_path(src: &Path, dest: &Path) -> Result<()> {
    if !exists(src) {
        bail!("\"{}\" no longer exists", src.display());
    }
    if exists(dest) && !same_file_case_only(src, dest) {
        bail!("\"{}\" already exists", dest.display());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            let parent = dest.parent().context("no parent folder")?;
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let created = ops::transfer(&[src.to_path_buf()], parent, ops::TransferKind::Copy, &cancel, |_| {})?;
            let copy = created.into_iter().next().context("nothing copied")?;
            if copy != dest {
                fs::rename(&copy, dest)?;
            }
            ops::remove_permanently(src)
        }
        Err(e) => Err(e).with_context(|| format!("moving \"{}\"", src.display())),
    }
}

/// Renaming `a.txt` to `A.txt` on a case-insensitive volume: `dest` "exists"
/// but is the same file.
fn same_file_case_only(a: &Path, b: &Path) -> bool {
    a.parent() == b.parent()
        && a.file_name().map(|n| n.to_string_lossy().to_lowercase()) == b.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

/// Moves `paths` to the trash, returning a [`Change::Trashed`] per item.
/// Stops at the first failure, returning what was trashed so far with it.
pub fn trash(paths: &[PathBuf]) -> (Vec<Change>, Result<()>) {
    let mut changes = Vec::new();
    for p in paths {
        match trash_one(p) {
            Ok(trashed) => changes.push(Change::Trashed { path: p.clone(), trashed }),
            Err(e) => return (changes, Err(e)),
        }
    }
    (changes, Ok(()))
}

/// Moves one item to the trash; returns where it ended up when the platform
/// says so (macOS), else None (it's found again by its original path).
#[cfg(target_os = "macos")]
pub fn trash_one(path: &Path) -> Result<Option<PathBuf>> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    if !exists(path) {
        bail!("\"{}\" no longer exists", path.display());
    }
    let s = path.to_str().context("path isn't valid UTF-8")?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(s));
    let mut out = None;
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, Some(&mut out))
        .map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))
        .with_context(|| format!("moving \"{}\" to the Trash", path.display()))?;
    Ok(out.and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string())))
}

#[cfg(not(target_os = "macos"))]
pub fn trash_one(path: &Path) -> Result<Option<PathBuf>> {
    if !exists(path) {
        bail!("\"{}\" no longer exists", path.display());
    }
    trash::delete(path).with_context(|| format!("moving \"{}\" to the trash", path.display()))?;
    Ok(None)
}

/// Puts an item back from the trash to `path`.
fn restore(path: &Path, trashed: Option<&Path>) -> Result<()> {
    if exists(path) {
        bail!("\"{}\" already exists", path.display());
    }
    if let Some(t) = trashed {
        if !exists(t) {
            bail!("\"{}\" is no longer in the {}", file_name(path), trash_name());
        }
        return move_path(t, path);
    }
    restore_by_original(path)
}

#[cfg(any(target_os = "windows", all(unix, not(target_os = "macos"))))]
fn restore_by_original(path: &Path) -> Result<()> {
    let items = trash::os_limited::list().context("reading the trash")?;
    let item = items
        .into_iter()
        .filter(|i| i.original_path() == path)
        .max_by_key(|i| i.time_deleted)
        .with_context(|| format!("\"{}\" is no longer in the {}", file_name(path), trash_name()))?;
    trash::os_limited::restore_all([item]).map_err(|e| anyhow::anyhow!("{e}")).context("restoring from the trash")
}

#[cfg(not(any(target_os = "windows", all(unix, not(target_os = "macos")))))]
fn restore_by_original(path: &Path) -> Result<()> {
    bail!("\"{}\" can't be found in the Trash", file_name(path))
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn trash_name() -> &'static str {
    if cfg!(target_os = "macos") { "Trash" } else if cfg!(windows) { "Recycle Bin" } else { "trash" }
}

/// Renames many paths at once (`pairs` of from → to, all distinct), safely
/// handling swaps and chains (a→b, b→a) by going through temporary names.
/// Returns the changes made; on failure everything done so far is reverted.
pub fn rename_all(pairs: &[(PathBuf, PathBuf)]) -> Result<Vec<Change>> {
    let pairs: Vec<_> = pairs.iter().filter(|(a, b)| a != b).cloned().collect();
    let sources: std::collections::HashSet<&PathBuf> = pairs.iter().map(|(a, _)| a).collect();
    let mut targets = std::collections::HashSet::new();
    for (from, to) in &pairs {
        if !targets.insert(to.clone()) {
            bail!("Two items would be named \"{}\"", file_name(to));
        }
        if exists(to) && !sources.contains(to) && !same_file_case_only(from, to) {
            bail!("\"{}\" already exists", file_name(to));
        }
    }
    // Phase 1: everything to a unique temporary name next to it.
    let mut staged: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::new();
    let stamp = std::process::id();
    let undo_staged = |staged: &[(PathBuf, PathBuf, PathBuf)]| {
        for (from, tmp, _) in staged.iter().rev() {
            fs::rename(tmp, from).ok();
        }
    };
    for (i, (from, to)) in pairs.iter().enumerate() {
        let tmp = from.with_file_name(format!(".rile-rename-{stamp}-{i}"));
        if let Err(e) = fs::rename(from, &tmp) {
            undo_staged(&staged);
            return Err(e).with_context(|| format!("renaming \"{}\"", file_name(from)));
        }
        staged.push((from.clone(), tmp, to.clone()));
    }
    // Phase 2: temporary names to the final ones.
    let mut placed: Vec<usize> = Vec::new();
    for (i, (_, tmp, to)) in staged.iter().enumerate() {
        if let Err(e) = fs::rename(tmp, to) {
            for &j in placed.iter().rev() {
                fs::rename(&staged[j].2, &staged[j].1).ok();
            }
            undo_staged(&staged);
            return Err(e).with_context(|| format!("renaming to \"{}\"", file_name(to)));
        }
        placed.push(i);
    }
    Ok(pairs.into_iter().map(|(from, to)| Change::Moved { from, to }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_all_swaps_and_reverts() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let b = t.path().join("b");
        fs::write(&a, "A").unwrap();
        fs::write(&b, "B").unwrap();
        let changes = rename_all(&[(a.clone(), b.clone()), (b.clone(), a.clone())]).unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "B");
        assert_eq!(fs::read_to_string(&b).unwrap(), "A");
        assert_eq!(changes.len(), 2);
        // Conflict with an unrelated existing file is refused up front.
        fs::write(t.path().join("c"), "C").unwrap();
        assert!(rename_all(&[(a.clone(), t.path().join("c"))]).is_err());
        // Duplicate targets too.
        assert!(rename_all(&[(a.clone(), t.path().join("x")), (b.clone(), t.path().join("x"))]).is_err());
        assert!(a.exists() && b.exists());
    }

    #[test]
    fn moved_changes_revert_and_apply() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a.txt");
        let sub = t.path().join("sub");
        fs::write(&a, "x").unwrap();
        fs::create_dir(&sub).unwrap();
        let mut c = Change::Moved { from: a.clone(), to: sub.join("a.txt") };
        apply(&mut c).unwrap();
        assert!(sub.join("a.txt").exists() && !a.exists());
        revert(&mut c).unwrap();
        assert!(a.exists() && !sub.join("a.txt").exists());
        // Reverting onto an existing file is refused.
        apply(&mut c).unwrap();
        fs::write(&a, "other").unwrap();
        assert!(revert(&mut c).is_err());
    }

    #[test]
    fn search_matches_label_kind_and_paths() {
        let r = Record {
            id: 1,
            time: 0,
            kind: OpKind::Copy,
            label: "Copied 2 items to Documents".into(),
            changes: vec![Change::Created { path: "/x/report.pdf".into(), trashed: None }],
            state: State::Done,
        };
        assert!(r.matches("documents"));
        assert!(r.matches("copy report"));
        assert!(!r.matches("photos"));
        assert!(r.undoable());
    }
}
