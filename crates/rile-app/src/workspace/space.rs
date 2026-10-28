//! Space Analyzer state: cached scans, the running scan, selection and cleanup.
//!
//! A scan covers its whole folder tree, so every folder below its root is
//! shown from memory: drilling down, going up and back/forward never rescan.
//! Moving items to the Trash patches the cached tree in place.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::{WindowExt, notification::Notification};
use gpui_kit::{AppContext, Context, Task, Window};
use rile_core::history::Location;
use rile_services::space::{Category, FileHit, Progress, Scan, ScanError, SuggestionKind};

use super::Workspace;

/// Finished scans kept in memory (the most recent first).
const KEPT_SCANS: usize = 3;
pub const LARGEST_FILES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpaceTab {
    #[default]
    Overview,
    Files,
    Cleanup,
}

pub struct RunningScan {
    pub root: PathBuf,
    pub progress: Arc<Progress>,
    pub started: Instant,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
    _tick: Task<()>,
}

/// Per-folder results derived from a scan, recomputed only when the shown
/// folder, the filter or the scan changes (never per frame).
pub struct FocusCache {
    pub focus: PathBuf,
    pub generation: u64,
    pub filter: Option<Category>,
    pub largest: Option<Vec<FileHit>>,
}

#[derive(Default)]
pub struct SpaceState {
    pub scans: Vec<Scan>,
    pub running: Option<RunningScan>,
    pub tab: SpaceTab,
    /// Items chosen for cleanup, across folders and tabs.
    pub selected: BTreeSet<PathBuf>,
    pub hovered: Option<PathBuf>,
    pub last_focus: Option<PathBuf>,
    pub error: Option<String>,
    pub trashing: bool,
    /// Cleanup groups showing all their rows.
    pub expanded: HashSet<SuggestionKind>,
    pub file_filter: Option<Category>,
    /// Bumped whenever a cached scan changes.
    pub generation: u64,
    pub cache: Option<FocusCache>,
}

impl SpaceState {
    /// The newest scan that covers `path`.
    pub fn scan_for(&self, path: &Path) -> Option<&Scan> {
        self.scans.iter().find(|s| s.contains(path) && s.node(path).is_some())
    }

    pub fn scanning(&self, path: &Path) -> bool {
        self.running.as_ref().is_some_and(|r| path.starts_with(&r.root))
    }

    /// Selected paths without those inside another selected folder, with
    /// the bytes they account for.
    pub fn selection(&self) -> (Vec<PathBuf>, u64) {
        let mut out: Vec<PathBuf> = Vec::new();
        let mut bytes = 0;
        for path in &self.selected {
            // BTreeSet order puts a folder right before its contents.
            if out.last().is_some_and(|p| path.starts_with(p)) {
                continue;
            }
            bytes += self.scan_for(path).and_then(|s| s.node(path)).map_or(0, |n| n.size);
            out.push(path.clone());
        }
        (out, bytes)
    }

    pub fn is_selected(&self, path: &Path) -> bool {
        path.ancestors().any(|p| self.selected.contains(p))
    }

    /// Largest files below `focus` (with the current filter), cached.
    pub fn largest(&mut self, focus: &Path) -> &[FileHit] {
        let fresh = self.cache.as_ref().is_some_and(|c| c.focus == focus && c.generation == self.generation && c.filter == self.file_filter);
        if !fresh || self.cache.as_ref().is_some_and(|c| c.largest.is_none()) {
            let filter = self.file_filter;
            let hits = self.scan_for(focus).map(|s| match filter {
                None => s.largest_files(focus, LARGEST_FILES),
                Some(cat) => {
                    // Over-fetch, then filter: categories are mixed in the heap.
                    let mut hits = s.largest_files(focus, LARGEST_FILES * 10);
                    hits.retain(|h| h.category == cat);
                    hits.truncate(LARGEST_FILES);
                    hits
                }
            });
            self.cache = Some(FocusCache { focus: focus.to_path_buf(), generation: self.generation, filter, largest: Some(hits.unwrap_or_default()) });
        }
        self.cache.as_ref().and_then(|c| c.largest.as_deref()).unwrap_or_default()
    }
}

impl Workspace {
    /// Opens the analyzer where the user left it.
    pub fn open_space_analyzer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.space.last_focus.clone().filter(|f| self.space.scan_for(f).is_some() || self.space.scanning(f));
        self.open_location(Location::SpaceAnalyzer(focus), window, cx);
    }

    /// Shows `dir` in the analyzer, scanning only if no cached scan covers it.
    pub fn analyze_folder(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let dir = dir.canonicalize().unwrap_or(dir);
        if !dir.is_dir() {
            window.push_notification(Notification::error(format!("Not a folder: {}", dir.display())), cx);
            return;
        }
        if self.space.scan_for(&dir).is_none() && !self.space.scanning(&dir) {
            self.start_space_scan(dir.clone(), cx);
        }
        self.space_navigate(dir, window, cx);
    }

    /// Drills into (or back out to) a folder of a cached scan.
    pub fn space_navigate(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.space.hovered = None;
        self.space.last_focus = Some(dir.clone());
        self.open_location(Location::SpaceAnalyzer(Some(dir)), window, cx);
    }

    /// The parent folder: from memory when cached, otherwise a new scan.
    pub fn space_up(&mut self, focus: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(parent) = focus.parent() {
            self.analyze_folder(parent.to_path_buf(), window, cx);
        }
    }

    /// Scans the cached root that covers `focus` again.
    pub fn rescan_space(&mut self, focus: &Path, cx: &mut Context<Self>) {
        let root = self.space.scan_for(focus).map(|s| s.root.clone()).unwrap_or_else(|| focus.to_path_buf());
        self.start_space_scan(root, cx);
    }

    pub fn cancel_space_scan(&mut self, cx: &mut Context<Self>) {
        if let Some(running) = self.space.running.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
        cx.notify();
    }

    fn start_space_scan(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.cancel_space_scan(cx);
        self.space.error = None;
        let progress = Arc::new(Progress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let task = {
            let (root, progress, cancel) = (root.clone(), progress.clone(), cancel.clone());
            cx.background_spawn(async move { rile_services::space::scan(&root, &cancel, &progress) })
        };
        let cancel_flag = cancel.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |ws, cx| {
                // A newer scan (or a cancel) replaced this one.
                if !ws.space.running.as_ref().is_some_and(|r| Arc::ptr_eq(&r.cancel, &cancel_flag)) {
                    return;
                }
                ws.space.running = None;
                match result {
                    Ok(scan) => {
                        // The new scan supersedes cached scans inside it.
                        ws.space.scans.retain(|s| !s.root.starts_with(&scan.root));
                        ws.space.scans.insert(0, scan);
                        ws.reward_event(rile_services::rewards::Event::SpaceAnalyzed, cx);
                        ws.space.scans.truncate(KEPT_SCANS);
                        ws.space.generation += 1;
                        let scanned: Vec<PathBuf> = ws.space.scans.iter().map(|s| s.root.clone()).collect();
                        ws.space.selected.retain(|p| scanned.iter().any(|r| p.starts_with(r)));
                    }
                    Err(ScanError::Cancelled) => {}
                    Err(ScanError::Io(e)) => ws.space.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        });
        // Live counters: repaint a few times per second while scanning.
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(120)).await;
                let Ok(running) = this.update(cx, |ws, cx| {
                    let running = ws.space.running.is_some();
                    if running && matches!(ws.pane().read(cx).location, Location::SpaceAnalyzer(_)) {
                        cx.notify();
                    }
                    running
                }) else {
                    break;
                };
                if !running {
                    break;
                }
            }
        });
        self.space.running = Some(RunningScan { root, progress, started: Instant::now(), cancel, _task: task, _tick: tick });
        cx.notify();
    }

    pub fn toggle_space_selection(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if rile_services::space::is_protected(&path) {
            return;
        }
        if !self.space.selected.remove(&path) {
            // Selecting a folder subsumes anything selected inside it.
            self.space.selected.retain(|p| !p.starts_with(&path));
            self.space.selected.insert(path);
        }
        cx.notify();
    }

    /// Selects every suggestion of `kind` below `focus`, or clears them when
    /// all are already selected.
    pub fn toggle_space_group(&mut self, kind: SuggestionKind, focus: &Path, cx: &mut Context<Self>) {
        let Some(scan) = self.space.scan_for(focus) else { return };
        let paths: Vec<PathBuf> = scan.suggestions_in(focus).filter(|s| s.kind == kind).map(|s| s.path.clone()).collect();
        let all = paths.iter().all(|p| self.space.selected.contains(p));
        for p in paths {
            if all {
                self.space.selected.remove(&p);
            } else {
                self.space.selected.insert(p);
            }
        }
        cx.notify();
    }

    /// Asks, then moves the selected items to the Trash.
    pub fn trash_space_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (paths, bytes) = self.space.selection();
        self.confirm_space_trash(paths, bytes, window, cx);
    }

    /// Asks, then moves one item to the Trash.
    pub fn trash_space_item(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let bytes = self.space.scan_for(&path).and_then(|s| s.node(&path)).map_or(0, |n| n.size);
        self.confirm_space_trash(vec![path], bytes, window, cx);
    }

    fn confirm_space_trash(&mut self, mut paths: Vec<PathBuf>, mut bytes: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.space.trashing {
            return;
        }
        // macOS refuses to trash system-managed folders; say so up front.
        let (protected, rest): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| rile_services::space::is_protected(p));
        if !protected.is_empty() {
            for p in &protected {
                bytes = bytes.saturating_sub(self.space.scan_for(p).and_then(|s| s.node(p)).map_or(0, |n| n.size));
                self.space.selected.remove(p);
            }
            let names: Vec<String> = protected.iter().map(|p| p.display().to_string()).collect();
            window.push_notification(
                Notification::warning(format!("{} is managed by macOS and can't be moved to the Trash. Open it to remove what's inside.", names.join(", "))),
                cx,
            );
            cx.notify();
        }
        // Nothing can be removed from a read-only folder (e.g. module caches).
        let (locked, rest): (Vec<_>, Vec<_>) = rest.into_iter().partition(|p| parent_read_only(p));
        if !locked.is_empty() {
            for p in &locked {
                bytes = bytes.saturating_sub(self.space.scan_for(p).and_then(|s| s.node(p)).map_or(0, |n| n.size));
                self.space.selected.remove(p);
            }
            let what = match locked.as_slice() {
                [one] => format!("“{}” is", one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                _ => format!("{} items are", locked.len()),
            };
            window.push_notification(
                Notification::warning(format!("{what} inside a read-only folder and can't be moved to the Trash. Remove them with the tool that manages them.")),
                cx,
            );
            cx.notify();
        }
        paths = rest;
        if paths.is_empty() {
            return;
        }
        let what = match paths.as_slice() {
            [one] => format!("“{}”", one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| one.display().to_string())),
            _ => format!("{} items", paths.len()),
        };
        let message = format!("Move {what} ({}) to the Trash?", humansize::format_size(bytes, humansize::DECIMAL));
        let ws = cx.entity().downgrade();
        crate::dialogs::confirm(message, "Move to Trash", true, window, cx, move |window, cx| {
            let paths = paths.clone();
            ws.update(cx, |ws, cx| ws.trash_space_paths(paths, window, cx)).ok();
        });
    }

    fn trash_space_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.space.trashing = true;
        cx.notify();
        let task = cx.background_spawn(async move {
            // One at a time, so a single failure doesn't hide what succeeded.
            paths.into_iter().map(|p| { let r = rile_core::ops::move_to_trash(std::slice::from_ref(&p)); (p, r) }).collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = task.await;
            this.update_in(cx, |ws, window, cx| {
                ws.space.trashing = false;
                let mut freed = 0;
                let mut moved = 0;
                let mut failed = Vec::new();
                for (path, result) in results {
                    match result {
                        Ok(()) => {
                            moved += 1;
                            freed += ws.space.scans.iter_mut().filter_map(|s| s.remove(&path)).max().unwrap_or(0);
                            ws.space.selected.retain(|p| !p.starts_with(&path));
                        }
                        Err(e) => failed.push(trash_error(&path, &format!("{e:#}"))),
                    }
                }
                ws.space.generation += 1;
                if freed > 0 {
                    ws.reward_event(rile_services::rewards::Event::SpaceFreed(freed), cx);
                }
                if moved > 0 {
                    let items = if moved == 1 { "1 item".to_string() } else { format!("{moved} items") };
                    window.push_notification(
                        Notification::success(format!("Moved {items} to the Trash · {} freed", humansize::format_size(freed, humansize::DECIMAL))),
                        cx,
                    );
                }
                if !failed.is_empty() {
                    let title = if failed.len() == 1 { "1 item couldn't be moved to the Trash".to_string() } else { format!("{} items couldn't be moved to the Trash", failed.len()) };
                    let mut lines: Vec<String> = failed.iter().take(3).cloned().collect();
                    if failed.len() > 3 {
                        lines.push(format!("…and {} more", failed.len() - 3));
                    }
                    window.push_notification(Notification::error(format!("{title}\n{}", lines.join("\n"))), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn parent_read_only(path: &Path) -> bool {
    path.parent().and_then(|p| std::fs::metadata(p).ok()).is_some_and(|m| m.permissions().readonly())
}

/// A short, human explanation instead of the trash backend's debug output.
fn trash_error(path: &Path, raw: &str) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let lower = raw.to_lowercase();
    let reason = if parent_read_only(path) {
        "its folder is read-only"
    } else if lower.contains("permission") || lower.contains("not permitted") {
        "no permission (it may be protected by macOS or owned by another user)"
    } else if lower.contains("no such file") || lower.contains("doesn’t exist") || lower.contains("doesn't exist") {
        "it no longer exists"
    } else if lower.contains("in use") || lower.contains("busy") {
        "it is in use"
    } else {
        return format!("“{name}”: {}", raw.lines().next().unwrap_or(raw));
    };
    format!("“{name}”: {reason}")
}
