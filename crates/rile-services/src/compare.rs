//! Folder comparison and synchronization.
//!
//! [`compare`] walks two folder trees side by side and reports every path
//! that exists on either side with how the two sides differ. A folder that
//! exists on one side only is reported once (its contents travel with it).
//! [`plan`] turns a comparison into per-item [`Action`]s for a sync
//! [`Mode`]; [`sync`] carries them out. Sync never destroys data outright:
//! files it replaces or removes go to the trash, and everything it does is
//! returned as [`journal::Change`]s so the whole run can be undone.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use rile_core::ops::{self, Progress, TransferKind};

use crate::journal::{self, Change};

/// Modification times this close count as equal (FAT has 2 s resolution).
const MTIME_SLACK: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    /// Same size and modification time = same file. Fast.
    #[default]
    SizeAndDate,
    /// Files of equal size are read and compared byte by byte.
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub method: Method,
    pub include_hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub is_dir: bool,
    pub size: u64,
    /// Unix seconds.
    pub modified: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Diff {
    Same,
    LeftOnly,
    RightOnly,
    LeftNewer,
    RightNewer,
    /// Same date but different size or content.
    Different,
    /// A file on one side, a folder on the other.
    Conflict,
}

impl Diff {
    pub fn label(self) -> &'static str {
        match self {
            Diff::Same => "Identical",
            Diff::LeftOnly => "Only left",
            Diff::RightOnly => "Only right",
            Diff::LeftNewer => "Left newer",
            Diff::RightNewer => "Right newer",
            Diff::Different => "Different",
            Diff::Conflict => "File vs. folder",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Path relative to both roots.
    pub rel: PathBuf,
    pub left: Option<Meta>,
    pub right: Option<Meta>,
    pub diff: Diff,
}

impl Item {
    pub fn is_dir(&self) -> bool {
        self.left.or(self.right).is_some_and(|m| m.is_dir)
    }
}

fn meta_of(m: &fs::Metadata) -> Meta {
    let modified = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
    Meta { is_dir: m.is_dir(), size: if m.is_dir() { 0 } else { m.len() }, modified }
}

fn read_level(dir: &Path, include_hidden: bool) -> std::collections::BTreeMap<std::ffi::OsString, Meta> {
    let mut out = std::collections::BTreeMap::new();
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name();
        if !include_hidden && name.to_string_lossy().starts_with('.') {
            continue;
        }
        // Follow symlinks for comparison (a link to a file compares as that file).
        let Ok(m) = fs::metadata(e.path()).or_else(|_| fs::symlink_metadata(e.path())) else { continue };
        out.insert(name, meta_of(&m));
    }
    out
}

fn same_content(a: &Path, b: &Path, cancel: &AtomicBool) -> Result<bool> {
    let mut fa = fs::File::open(a)?;
    let mut fb = fs::File::open(b)?;
    let mut ba = vec![0u8; 256 * 1024];
    let mut bb = vec![0u8; 256 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let n = read_full(&mut fa, &mut ba)?;
        let m = read_full(&mut fb, &mut bb)?;
        if n != m || ba[..n] != bb[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(f: &mut fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// Compares the trees under `left` and `right`. `on_progress` gets the
/// number of entries looked at so far.
pub fn compare(left: &Path, right: &Path, opts: Options, cancel: &AtomicBool, mut on_progress: impl FnMut(usize)) -> Result<Vec<Item>> {
    if !left.is_dir() {
        bail!("\"{}\" isn't a folder", left.display());
    }
    if !right.is_dir() {
        bail!("\"{}\" isn't a folder", right.display());
    }
    if ops::path_starts_with(left, right) || ops::path_starts_with(right, left) {
        bail!("One folder is inside the other; pick two separate folders");
    }
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let l = read_level(&left.join(&rel), opts.include_hidden);
        let r = read_level(&right.join(&rel), opts.include_hidden);
        let mut names: Vec<_> = l.keys().chain(r.keys()).cloned().collect();
        names.sort();
        names.dedup();
        for name in names {
            seen += 1;
            if seen.is_multiple_of(256) {
                on_progress(seen);
            }
            let child = rel.join(&name);
            let (lm, rm) = (l.get(&name).copied(), r.get(&name).copied());
            let diff = match (lm, rm) {
                (Some(_), None) => Diff::LeftOnly,
                (None, Some(_)) => Diff::RightOnly,
                (Some(a), Some(b)) if a.is_dir != b.is_dir => Diff::Conflict,
                (Some(a), Some(_)) if a.is_dir => {
                    stack.push(child);
                    continue;
                }
                (Some(a), Some(b)) => {
                    let close = (a.modified - b.modified).abs() <= MTIME_SLACK;
                    match opts.method {
                        Method::SizeAndDate if a.size == b.size && close => Diff::Same,
                        Method::Content if a.size == b.size && same_content(&left.join(&child), &right.join(&child), cancel)? => Diff::Same,
                        _ if close => Diff::Different,
                        _ if a.modified > b.modified => Diff::LeftNewer,
                        _ => Diff::RightNewer,
                    }
                }
                (None, None) => unreachable!(),
            };
            out.push(Item { rel: child, left: lm, right: rm, diff });
        }
    }
    on_progress(seen);
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Make the right side an exact copy of the left (deletes extras).
    MirrorToRight,
    /// Copy new and newer files left → right; never delete.
    UpdateRight,
    /// Newest wins, missing items are copied both ways; never delete.
    #[default]
    TwoWay,
    UpdateLeft,
    MirrorToLeft,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::MirrorToRight, Mode::UpdateRight, Mode::TwoWay, Mode::UpdateLeft, Mode::MirrorToLeft];

    pub fn label(self) -> &'static str {
        match self {
            Mode::MirrorToRight => "Mirror →",
            Mode::UpdateRight => "Update →",
            Mode::TwoWay => "Two-way",
            Mode::UpdateLeft => "← Update",
            Mode::MirrorToLeft => "← Mirror",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Mode::MirrorToRight => "Make the right folder an exact copy of the left. Extra items on the right go to the trash.",
            Mode::UpdateRight => "Copy new and newer items from left to right. Nothing is deleted.",
            Mode::TwoWay => "Copy missing items both ways; where both changed, the newer one wins. Nothing is deleted.",
            Mode::UpdateLeft => "Copy new and newer items from right to left. Nothing is deleted.",
            Mode::MirrorToLeft => "Make the left folder an exact copy of the right. Extra items on the left go to the trash.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Skip,
    CopyToRight,
    CopyToLeft,
    DeleteLeft,
    DeleteRight,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Skip => "Skip",
            Action::CopyToRight => "Copy right",
            Action::CopyToLeft => "Copy left",
            Action::DeleteLeft => "Delete left",
            Action::DeleteRight => "Delete right",
        }
    }

    /// Actions that make sense for an item, for cycling through in the UI.
    pub fn choices(item: &Item) -> Vec<Action> {
        let mut v = vec![Action::Skip];
        if item.left.is_some() {
            v.push(Action::CopyToRight);
        }
        if item.right.is_some() {
            v.push(Action::CopyToLeft);
        }
        if item.right.is_some() {
            v.push(Action::DeleteRight);
        }
        if item.left.is_some() {
            v.push(Action::DeleteLeft);
        }
        v
    }
}

/// The default action for `item` under `mode`.
pub fn plan(item: &Item, mode: Mode) -> Action {
    use Diff::*;
    match (mode, item.diff) {
        (_, Same) => Action::Skip,
        (Mode::MirrorToRight, LeftOnly | LeftNewer | RightNewer | Different | Conflict) => Action::CopyToRight,
        (Mode::MirrorToRight, RightOnly) => Action::DeleteRight,
        (Mode::MirrorToLeft, RightOnly | LeftNewer | RightNewer | Different | Conflict) => Action::CopyToLeft,
        (Mode::MirrorToLeft, LeftOnly) => Action::DeleteLeft,
        (Mode::UpdateRight, LeftOnly | LeftNewer) => Action::CopyToRight,
        (Mode::UpdateLeft, RightOnly | RightNewer) => Action::CopyToLeft,
        (Mode::TwoWay, LeftOnly | LeftNewer) => Action::CopyToRight,
        (Mode::TwoWay, RightOnly | RightNewer) => Action::CopyToLeft,
        _ => Action::Skip,
    }
}

/// Bytes a sync of `actions` copies.
pub fn bytes_to_copy(actions: &[(Item, Action)]) -> u64 {
    actions
        .iter()
        .map(|(i, a)| match a {
            Action::CopyToRight => i.left.map(|m| m.size).unwrap_or(0),
            Action::CopyToLeft => i.right.map(|m| m.size).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Carries out `actions`. Returns the changes made (for undo) and the first
/// error, if any; the changes cover everything done up to that error.
pub fn sync(
    left: &Path,
    right: &Path,
    actions: &[(Item, Action)],
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
) -> (Vec<Change>, Result<()>) {
    let mut changes = Vec::new();
    let mut progress = Progress {
        bytes_total: bytes_to_copy(actions),
        items_total: actions.iter().filter(|(_, a)| *a != Action::Skip).count(),
        ..Default::default()
    };
    on_progress(progress.clone());
    let result = (|| -> Result<()> {
        for (item, action) in actions {
            if cancel.load(Ordering::Relaxed) {
                bail!("Cancelled");
            }
            match action {
                Action::Skip => continue,
                Action::CopyToRight | Action::CopyToLeft => {
                    let (from_root, to_root) = if *action == Action::CopyToRight { (left, right) } else { (right, left) };
                    let src = from_root.join(&item.rel);
                    let dest = to_root.join(&item.rel);
                    let parent = dest.parent().context("no parent folder")?.to_path_buf();
                    make_parents(to_root, &parent, &mut changes)?;
                    if fs::symlink_metadata(&dest).is_ok() {
                        let trashed = journal::trash_one(&dest)?;
                        changes.push(Change::Trashed { path: dest.clone(), trashed });
                    }
                    let base = progress.clone();
                    let created = ops::transfer(std::slice::from_ref(&src), &parent, TransferKind::Copy, cancel, |p| {
                        let mut q = base.clone();
                        q.bytes_done += p.bytes_done;
                        q.current = p.current.clone();
                        q.file_bytes_done = p.file_bytes_done;
                        q.file_bytes_total = p.file_bytes_total;
                        on_progress(q);
                    })?;
                    for c in created {
                        // The name was freed above, so the copy lands on `dest`.
                        if c != dest {
                            fs::rename(&c, &dest)?;
                        }
                        changes.push(Change::Created { path: dest.clone(), trashed: None });
                    }
                    progress.bytes_done += if *action == Action::CopyToRight { item.left } else { item.right }.map(|m| m.size).unwrap_or(0);
                }
                Action::DeleteLeft | Action::DeleteRight => {
                    let root = if *action == Action::DeleteLeft { left } else { right };
                    let path = root.join(&item.rel);
                    let trashed = journal::trash_one(&path)?;
                    changes.push(Change::Trashed { path, trashed });
                }
            }
            progress.items_done += 1;
            on_progress(progress.clone());
        }
        Ok(())
    })();
    (changes, result)
}

/// Creates the missing folders between `root` and `dir`, recording each.
fn make_parents(root: &Path, dir: &Path, changes: &mut Vec<Change>) -> Result<()> {
    let mut missing = Vec::new();
    let mut cur = dir;
    while cur != root && fs::symlink_metadata(cur).is_err() {
        missing.push(cur.to_path_buf());
        cur = cur.parent().context("no parent folder")?;
    }
    for d in missing.into_iter().rev() {
        fs::create_dir(&d).with_context(|| format!("creating {}", d.display()))?;
        changes.push(Change::Created { path: d, trashed: None });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn compare_and_plan() {
        let t = tempfile::tempdir().unwrap();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        write(&l.join("same.txt"), "x");
        write(&r.join("same.txt"), "x");
        let now = std::time::SystemTime::now();
        for p in [l.join("same.txt"), r.join("same.txt")] {
            fs::File::options().write(true).open(p).unwrap().set_modified(now).unwrap();
        }
        write(&l.join("only_l/deep.txt"), "l");
        write(&r.join("only_r.txt"), "r");
        write(&l.join("sub/changed.txt"), "new content");
        write(&r.join("sub/changed.txt"), "old");
        let old = now - std::time::Duration::from_secs(3600);
        fs::File::options().write(true).open(r.join("sub/changed.txt")).unwrap().set_modified(old).unwrap();
        write(&l.join(".hidden"), "h");

        let cancel = AtomicBool::new(false);
        let items = compare(&l, &r, Options::default(), &cancel, |_| {}).unwrap();
        let find = |rel: &str| items.iter().find(|i| i.rel == Path::new(rel)).map(|i| i.diff);
        assert_eq!(find("same.txt"), Some(Diff::Same));
        assert_eq!(find("only_l"), Some(Diff::LeftOnly));
        assert_eq!(find("only_l/deep.txt"), None, "one-sided folders aren't expanded");
        assert_eq!(find("only_r.txt"), Some(Diff::RightOnly));
        assert_eq!(find("sub/changed.txt"), Some(Diff::LeftNewer));
        assert_eq!(find(".hidden"), None);

        assert_eq!(plan(items.iter().find(|i| i.rel == Path::new("only_r.txt")).unwrap(), Mode::MirrorToRight), Action::DeleteRight);
        assert_eq!(plan(items.iter().find(|i| i.rel == Path::new("only_r.txt")).unwrap(), Mode::UpdateRight), Action::Skip);
        // Only additions here: replacing/deleting would use the real trash.
        let actions: Vec<_> = items.iter().filter(|i| i.diff == Diff::LeftOnly).map(|i| (i.clone(), plan(i, Mode::UpdateRight))).collect();
        let (changes, res) = sync(&l, &r, &actions, &cancel, |_| {});
        res.unwrap();
        assert!(r.join("only_l/deep.txt").exists());
        assert_eq!(changes, vec![Change::Created { path: r.join("only_l"), trashed: None }]);
    }

    #[test]
    fn refuses_nested() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("a/b")).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(compare(&t.path().join("a"), &t.path().join("a/b"), Options::default(), &cancel, |_| {}).is_err());
    }
}
