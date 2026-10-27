use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    App, AppContext, Bounds, Context, EventEmitter, ExternalDragPayload, FileDragPaths, Pixels, Point, point, px, FocusHandle, Focusable, Modifiers, ScrollStrategy,
    SharedString, Task, UniformListScrollHandle, WeakEntity, Window,
};
use rile_core::history::Location;
use rile_core::settings::ViewKind;
use rile_core::watcher::DirWatcher;
use rile_core::{Entry, History, SortKey, SortOrder, list_dir, sort_entries};

/// Live directory watching; disabled by headless tests, whose deterministic
/// scheduler rejects wakeups from the watcher's OS thread.
pub static WATCH_FS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub enum PaneEvent {
    /// The pane now shows a different location.
    Navigated,
    /// The user interacted with this pane (makes it the active one).
    Activated,
    /// Files were dropped onto this pane or one of its folders. `*_conn`
    /// is the remote connection of each side (None = local).
    Drop { paths: Vec<PathBuf>, src_conn: Option<String>, dest: PathBuf, dest_conn: Option<String>, copy: bool },
    /// External files (e.g. from Finder) were dropped.
    DropExternal { paths: Vec<PathBuf>, dest: PathBuf, dest_conn: Option<String> },
    /// A remote file was opened (needs downloading first).
    OpenRemote { conn: String, path: String },
}

/// Drag payload for files dragged out of a pane. Paths are resolved on drop
/// so dragging a large selection costs nothing per frame.
#[derive(Clone)]
pub struct DraggedFiles {
    pub source: WeakEntity<Pane>,
    /// Remote connection the files live on (None = local disk).
    pub conn: Option<String>,
    pub path: PathBuf,
    pub label: SharedString,
}

impl DraggedFiles {
    /// Only local entries can be offered to the OS as real files. Use cached
    /// entry metadata so handing off a large selection never stats the disk.
    pub fn external_payload(&self, cx: &App) -> Option<ExternalDragPayload> {
        if self.conn.is_some() {
            return None;
        }
        let source = self.source.upgrade()?;
        let pane = source.read(cx);
        let selected = pane.selected.contains(&self.path);
        let paths = FileDragPaths::new(
            pane.entries().iter()
                .filter(|e| if selected { pane.selected.contains(&e.path) } else { e.path == self.path })
                .map(|e| (e.path.clone(), e.is_dir)),
        );
        if paths.entries().is_empty() { None } else { Some(ExternalDragPayload::Files(paths)) }
    }

    /// Paths being dragged. Must not be called while the source pane is
    /// being updated; inside a pane listener use [`Self::resolve_in`].
    pub fn resolve(&self, cx: &App) -> Vec<PathBuf> {
        match self.source.upgrade() {
            Some(pane) => self.resolve_from(pane.read(cx)),
            None => vec![self.path.clone()],
        }
    }

    /// Resolves from within a listener of `this` pane (which may or may not
    /// be the drag source) without re-entering the entity being updated.
    pub fn resolve_in(&self, this: &Pane, cx: &Context<Pane>) -> Vec<PathBuf> {
        if self.source == cx.entity().downgrade() { self.resolve_from(this) } else { self.resolve(cx) }
    }

    fn resolve_from(&self, pane: &Pane) -> Vec<PathBuf> {
        if pane.selected.contains(&self.path) { pane.selected_paths() } else { vec![self.path.clone()] }
    }
}

/// Rubber-band (drag-to-select) state, in window coordinates.
pub struct Rubber {
    pub origin: Point<Pixels>,
    pub current: Point<Pixels>,
    /// Selection before the drag started (kept for cmd/shift-drags).
    base: HashSet<PathBuf>,
}

impl Rubber {
    pub fn rect(&self) -> Bounds<Pixels> {
        Bounds::from_corners(
            point(self.origin.x.min(self.current.x), self.origin.y.min(self.current.y)),
            point(self.origin.x.max(self.current.x), self.origin.y.max(self.current.y)),
        )
    }
}

/// Active recursive search whose results replace the listing.
pub struct SearchInfo {
    pub label: SharedString,
    pub running: bool,
    /// Stopped early by the user (Esc); the hits found so far stay.
    pub stopped: bool,
    pub scanned: u64,
    pub error: Option<SharedString>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

/// One browsing context: location, listing, selection and history.
pub struct Pane {
    pub location: Location,
    pub history: History,
    /// Full directory listing (or search results).
    all_entries: Arc<Vec<Entry>>,
    /// `all_entries` after the quick filter.
    entries: Arc<Vec<Entry>>,
    pub filter: String,
    pub selected: HashSet<PathBuf>,
    anchor: Option<usize>,
    pub cursor: Option<usize>,
    pub sort_key: SortKey,
    pub sort_order: SortOrder,
    pub show_hidden: bool,
    pub view: ViewKind,
    pub error: Option<SharedString>,
    /// Paths currently on the clipboard in cut mode (rendered dimmed).
    pub cut_paths: HashSet<PathBuf>,
    pub loading: bool,
    pub search: Option<SearchInfo>,
    /// Set by the workspace while Quick Look is open (changes key context).
    pub previewing: bool,
    /// Whether this is the workspace's active pane (dual-pane highlight).
    pub active: bool,
    /// Width of the content area, measured during paint (for grid layout).
    pub viewport_width: f32,
    /// Grid cell metrics for the current icon size (set by the grid view).
    pub tile_min_width: f32,
    pub tile_height: f32,
    /// Window-space origin of the content area (for the rubber-band overlay).
    pub viewport_origin: Point<Pixels>,
    pub rubber: Option<Rubber>,
    /// Listings of neighbour folders for Miller columns, loaded on demand.
    side_listings: HashMap<PathBuf, Arc<Vec<Entry>>>,
    side_loading: HashSet<PathBuf>,
    generation: u64,
    _load_task: Option<Task<()>>,
    _watcher: Option<DirWatcher>,
    _watch_task: Option<Task<()>>,
    _search_task: Option<Task<()>>,
    /// Paths to select once the next load completes (after paste, new folder...).
    pending_select: Option<Vec<PathBuf>>,
    pub focus_handle: FocusHandle,
    pub scroll: UniformListScrollHandle,
}

impl EventEmitter<PaneEvent> for Pane {}

impl Focusable for Pane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Pane {
    pub fn new(
        location: Location,
        show_hidden: bool,
        sort_key: SortKey,
        sort_order: SortOrder,
        view: ViewKind,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            location: Location::Home,
            history: History::default(),
            all_entries: Arc::default(),
            entries: Arc::default(),
            filter: String::new(),
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            sort_key,
            sort_order,
            show_hidden,
            view,
            error: None,
            cut_paths: HashSet::new(),
            loading: false,
            search: None,
            previewing: false,
            active: true,
            viewport_width: 800.,
            tile_min_width: crate::views::grid::tile_metrics(64.).0,
            tile_height: crate::views::grid::tile_metrics(64.).1,
            viewport_origin: Point::default(),
            rubber: None,
            side_listings: HashMap::new(),
            side_loading: HashSet::new(),
            generation: 0,
            _load_task: None,
            _watcher: None,
            _watch_task: None,
            _search_task: None,
            pending_select: None,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        };
        this.set_location(location, cx);
        this
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn total_count(&self) -> usize {
        self.all_entries.len()
    }

    /// The local directory shown, if any (None at Home and on remotes).
    pub fn current_dir(&self) -> Option<&Path> {
        match &self.location {
            Location::Dir(p) => Some(p),
            _ => None,
        }
    }

    /// Remote connection id when browsing a server.
    pub fn conn(&self) -> Option<String> {
        match &self.location {
            Location::Remote { conn, .. } => Some(conn.clone()),
            _ => None,
        }
    }

    /// The shown directory as a path, local or remote.
    pub fn dir_path(&self) -> Option<PathBuf> {
        match &self.location {
            Location::Dir(p) => Some(p.clone()),
            Location::Remote { path, .. } => Some(PathBuf::from(path)),
            Location::Home | Location::Gallery => None,
        }
    }

    /// A location for `dir` on the same filesystem as the current one.
    pub fn location_for(&self, dir: PathBuf) -> Location {
        match &self.location {
            Location::Remote { conn, .. } => Location::Remote { conn: conn.clone(), path: crate::remote::path_str(&dir) },
            _ => Location::Dir(dir),
        }
    }

    /// Navigates and records history.
    pub fn navigate(&mut self, location: Location, cx: &mut Context<Self>) {
        if location == self.location && self.search.is_none() {
            return;
        }
        self.history.push(self.location.clone());
        self.set_location(location, cx);
    }

    fn set_location(&mut self, location: Location, cx: &mut Context<Self>) {
        let previous = std::mem::replace(&mut self.location, location);
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.all_entries = Arc::default();
        self.entries = Arc::default();
        self.filter.clear();
        self.error = None;
        self.search = None;
        self._search_task = None;
        self._watcher = None;
        self._watch_task = None;
        // Coming back up to a parent: preselect the folder we came from.
        let prev_dir = match &previous {
            Location::Dir(p) => Some(p.clone()),
            Location::Remote { path, .. } => Some(PathBuf::from(path)),
            Location::Home | Location::Gallery => None,
        };
        if let (Some(dir), Some(prev)) = (self.dir_path(), prev_dir)
            && prev.parent() == Some(dir.as_path())
        {
            self.pending_select = Some(vec![prev]);
        }
        if let Location::Dir(dir) = &self.location {
            self.watch(dir.clone(), cx);
        }
        self.reload(cx);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.emit(PaneEvent::Navigated);
        cx.notify();
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.search.is_some() {
            self.clear_search(cx);
            return;
        }
        if let Some(loc) = self.history.go_back(self.location.clone()) {
            self.set_location(loc, cx);
        }
    }

    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(loc) = self.history.go_forward(self.location.clone()) {
            self.set_location(loc, cx);
        }
    }

    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        let parent = match &self.location {
            Location::Remote { path, .. } => crate::remote::parent(path).map(PathBuf::from),
            _ => self.current_dir().and_then(Path::parent).map(Path::to_path_buf),
        };
        if let Some(parent) = parent {
            let loc = self.location_for(parent);
            self.navigate(loc, cx);
        }
    }

    pub fn select_after_reload(&mut self, paths: Vec<PathBuf>) {
        self.pending_select = Some(paths);
    }

    /// Re-reads the directory on a background thread. Results of stale
    /// loads (superseded by navigation) are dropped via the generation.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.side_listings.clear();
        if self.search.is_some() {
            return;
        }
        let Some(dir) = self.dir_path() else { return };
        let conn = self.conn();
        self.generation += 1;
        let generation = self.generation;
        let (show_hidden, key, order) = (self.show_hidden, self.sort_key, self.sort_order);
        self.loading = true;
        let task = cx.background_spawn(async move {
            let mut entries = list_any(conn.as_deref(), &dir, show_hidden)?;
            if conn.is_none() {
                rile_services::index::record(&dir, &entries);
            }
            sort_entries(&mut entries, key, order);
            anyhow::Ok(entries)
        });
        self._load_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(entries) => this.apply_entries(entries),
                    Err(e) => {
                        this.error = Some(format!("{e:#}").into());
                        this.all_entries = Arc::default();
                        this.entries = Arc::default();
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn apply_entries(&mut self, entries: Vec<Entry>) {
        self.error = None;
        let cursor_path = self.cursor_entry().map(|e| e.path.clone());
        self.all_entries = Arc::new(entries);
        self.refilter();
        if let Some(paths) = self.pending_select.take() {
            self.selected = paths.into_iter().collect();
            self.cursor = self.entries.iter().position(|e| self.selected.contains(&e.path));
            self.anchor = self.cursor;
            if let Some(ix) = self.cursor {
                self.scroll.scroll_to_item(self.row_of(ix), ScrollStrategy::Center);
            }
        } else {
            let existing: HashSet<&Path> = self.entries.iter().map(|e| e.path.as_path()).collect();
            self.selected.retain(|p| existing.contains(p.as_path()));
            self.cursor = cursor_path.and_then(|p| self.entries.iter().position(|e| e.path == p));
            if self.cursor.is_none() && !self.selected.is_empty() {
                self.cursor = self.entries.iter().position(|e| self.selected.contains(&e.path));
            }
            self.anchor = self.anchor.and(self.cursor);
        }
    }

    fn refilter(&mut self) {
        if self.filter.is_empty() {
            self.entries = self.all_entries.clone();
        } else {
            let needle = self.filter.to_lowercase();
            self.entries = Arc::new(
                self.all_entries
                    .iter()
                    .filter(|e| e.name_lower.contains(&needle))
                    .cloned()
                    .collect(),
            );
        }
    }

    /// In-memory filter of the current listing (type-to-filter).
    pub fn set_filter(&mut self, filter: String, cx: &mut Context<Self>) {
        if filter == self.filter {
            return;
        }
        let cursor_path = self.cursor_entry().map(|e| e.path.clone());
        self.filter = filter;
        self.refilter();
        self.cursor = cursor_path
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .or((!self.entries.is_empty()).then_some(0));
        if let Some(ix) = self.cursor
            && !self.filter.is_empty()
        {
            self.selected = HashSet::from([self.entries[ix].path.clone()]);
            self.anchor = Some(ix);
        }
        self.scroll.scroll_to_item(self.cursor.map(|c| self.row_of(c)).unwrap_or(0), ScrollStrategy::Top);
        cx.notify();
    }

    fn watch(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !WATCH_FS.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let (tx, rx) = smol::channel::unbounded::<()>();
        self._watcher = DirWatcher::new(&dir, move || {
            tx.try_send(()).ok();
        })
        .ok();
        self._watch_task = Some(cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                // Debounce bursts (e.g. extracting an archive).
                cx.background_executor().timer(Duration::from_millis(150)).await;
                while rx.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub fn set_sort(&mut self, key: SortKey, cx: &mut Context<Self>) {
        if self.sort_key == key {
            self.sort_order = self.sort_order.toggled();
        } else {
            self.sort_key = key;
            self.sort_order = SortOrder::Ascending;
        }
        let mut entries = (*self.all_entries).clone();
        sort_entries(&mut entries, self.sort_key, self.sort_order);
        self.apply_entries(entries);
        cx.notify();
    }

    pub fn set_show_hidden(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_hidden != show {
            self.show_hidden = show;
            self.reload(cx);
        }
    }

    pub fn set_view(&mut self, view: ViewKind, cx: &mut Context<Self>) {
        self.view = view;
        if let Some(ix) = self.cursor {
            self.scroll.scroll_to_item(self.row_of(ix), ScrollStrategy::Center);
        }
        cx.notify();
    }

    // --- Search ------------------------------------------------------------

    /// Starts showing streamed search results instead of the listing.
    /// `run` executes on a background thread; it pushes hit batches and
    /// progress through the given callbacks.
    /// An AI (intelligent) search is running in this pane.
    pub fn ai_searching(&self) -> bool {
        self.search.as_ref().is_some_and(|s| s.running && s.label.starts_with("AI:"))
    }

    pub fn start_search(
        &mut self,
        label: SharedString,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        cx: &mut Context<Self>,
        run: impl FnOnce(&(dyn Fn(Vec<Entry>) + Sync), &(dyn Fn(u64) + Sync)) -> anyhow::Result<()> + Send + 'static,
    ) {
        self.generation += 1;
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.filter.clear();
        self.all_entries = Arc::default();
        self.entries = Arc::default();
        self.error = None;
        self.search = Some(SearchInfo { label, running: true, stopped: false, scanned: 0, error: None, cancel: cancel.clone() });
        let (tx, rx) = smol::channel::unbounded::<SearchMsg>();
        let job = cx.background_spawn(async move {
            let push = {
                let tx = tx.clone();
                move |batch: Vec<Entry>| {
                    tx.try_send(SearchMsg::Hits(batch)).ok();
                }
            };
            let scanned = {
                let tx = tx.clone();
                move |n: u64| {
                    tx.try_send(SearchMsg::Scanned(n)).ok();
                }
            };
            let result = run(&push, &scanned);
            tx.try_send(SearchMsg::Done(result.err().map(|e| format!("{e:#}")))).ok();
        });
        self._search_task = Some(cx.spawn(async move |this, cx| {
            let _cancel_on_drop = CancelOnDrop(cancel);
            let _job = job;
            while let Ok(msg) = rx.recv().await {
                // Coalesce whatever else is queued into one UI update.
                let mut msgs = vec![msg];
                while let Ok(m) = rx.try_recv() {
                    msgs.push(m);
                }
                let mut done = false;
                let alive = this.update(cx, |this, cx| {
                    let mut hits = Vec::new();
                    for m in msgs {
                        match m {
                            SearchMsg::Hits(h) => hits.extend(h),
                            SearchMsg::Scanned(n) => {
                                if let Some(s) = &mut this.search {
                                    s.scanned = n;
                                }
                            }
                            SearchMsg::Done(err) => {
                                done = true;
                                if let Some(s) = &mut this.search {
                                    s.running = false;
                                    // A search the user stopped isn't an error.
                                    s.error = if s.stopped { None } else { err.map(Into::into) };
                                }
                            }
                        }
                    }
                    if !hits.is_empty() {
                        let mut all = (*this.all_entries).clone();
                        all.extend(hits);
                        this.all_entries = Arc::new(all);
                        this.refilter();
                    }
                    cx.notify();
                });
                if alive.is_err() || done {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(60)).await;
            }
        }));
        cx.notify();
    }

    pub fn clear_search(&mut self, cx: &mut Context<Self>) {
        if self.search.take().is_some() {
            self._search_task = None;
            self.all_entries = Arc::default();
            self.entries = Arc::default();
            self.reload(cx);
            cx.notify();
        }
    }

    // --- Miller columns ----------------------------------------------------

    /// Cached listing of another folder, loading it in the background.
    pub fn side_listing(&mut self, dir: &Path, cx: &mut Context<Self>) -> Option<Arc<Vec<Entry>>> {
        if let Some(l) = self.side_listings.get(dir) {
            return Some(l.clone());
        }
        if self.side_loading.insert(dir.to_path_buf()) {
            let dir = dir.to_path_buf();
            let conn = self.conn();
            let (show_hidden, key, order) = (self.show_hidden, self.sort_key, self.sort_order);
            let task = cx.background_spawn({
                let dir = dir.clone();
                async move {
                    let mut e = list_any(conn.as_deref(), &dir, show_hidden).unwrap_or_default();
                    if conn.is_none() {
                        rile_services::index::record(&dir, &e);
                    }
                    sort_entries(&mut e, key, order);
                    e
                }
            });
            cx.spawn(async move |this, cx| {
                let entries = task.await;
                this.update(cx, |this, cx| {
                    this.side_loading.remove(&dir);
                    if this.side_listings.len() > 64 {
                        this.side_listings.clear();
                    }
                    this.side_listings.insert(dir, Arc::new(entries));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        None
    }

    // --- Opening -----------------------------------------------------------

    /// Opens an entry: folders navigate, files open with the system app.
    pub fn open_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(ix).cloned() else { return };
        self.open_entry(&entry, cx);
    }

    pub fn open_entry(&mut self, entry: &Entry, cx: &mut Context<Self>) {
        // App bundles are folders, but opening one launches the app.
        if self.conn().is_none() && crate::icons::is_app_bundle(entry) {
            cx.open_with_system(&entry.path);
        } else if entry.is_dir {
            let loc = self.location_for(entry.path.clone());
            self.navigate(loc, cx);
        } else if let Some(conn) = self.conn() {
            cx.emit(PaneEvent::OpenRemote { conn, path: crate::remote::path_str(&entry.path) });
        } else {
            crate::workspace::open_with::open_file(&entry.path, cx);
        }
    }

    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let entries: Vec<Entry> = self.selected_entries().into_iter().cloned().collect();
        if let [single] = entries.as_slice()
            && single.is_dir
        {
            self.open_entry(single, cx);
            return;
        }
        for e in entries.iter().filter(|e| !e.is_dir) {
            self.open_entry(e, cx);
        }
    }

    // --- Selection -----------------------------------------------------------

    pub fn cursor_entry(&self) -> Option<&Entry> {
        self.cursor.and_then(|ix| self.entries.get(ix))
    }

    /// Selected paths in display order.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        if self.selected.is_empty() {
            return Vec::new();
        }
        self.entries
            .iter()
            .filter(|e| self.selected.contains(&e.path))
            .map(|e| e.path.clone())
            .collect()
    }

    pub fn selected_entries(&self) -> Vec<&Entry> {
        if self.selected.is_empty() {
            return Vec::new();
        }
        self.entries.iter().filter(|e| self.selected.contains(&e.path)).collect()
    }

    pub fn click(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(path) = self.entries.get(ix).map(|e| e.path.clone()) else { return };
        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(ix).min(self.entries.len() - 1);
            if !modifiers.secondary(cx) {
                self.selected.clear();
            }
            let (lo, hi) = (anchor.min(ix), anchor.max(ix));
            self.selected.extend(self.entries[lo..=hi].iter().map(|e| e.path.clone()));
            self.anchor = Some(anchor);
        } else if modifiers.secondary(cx) {
            if !self.selected.remove(&path) {
                self.selected.insert(path);
            }
            self.anchor = Some(ix);
        } else {
            self.selected.clear();
            self.selected.insert(path);
            self.anchor = Some(ix);
        }
        self.cursor = Some(ix);
        cx.notify();
    }

    /// Right-click keeps an existing multi-selection if clicking inside it.
    pub fn ensure_selected(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(e) = self.entries.get(ix)
            && !self.selected.contains(&e.path)
        {
            self.click(ix, Modifiers::default(), cx);
        }
    }

    /// Number of items per row in the current view (1 except in grid view).
    pub fn columns_per_row(&self) -> usize {
        match self.view {
            ViewKind::Grid => (self.grid_width() / self.tile_min_width).floor().max(1.) as usize,
            _ => 1,
        }
    }

    fn grid_width(&self) -> f32 {
        (self.viewport_width - 2. * crate::views::grid::GRID_PADDING_X).max(1.)
    }

    /// Actual grid cell width: cells stretch to fill the row.
    pub fn tile_width(&self) -> f32 {
        self.grid_width() / self.columns_per_row() as f32
    }

    fn row_of(&self, ix: usize) -> usize {
        ix / self.columns_per_row()
    }

    pub fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let len = self.entries.len();
        if len == 0 {
            return;
        }
        let ix = match self.cursor {
            None => if delta >= 0 { 0 } else { len - 1 },
            Some(c) => (c as isize + delta).clamp(0, len as isize - 1) as usize,
        };
        self.cursor_to(ix, extend, cx);
    }

    /// Vertical movement: one row (grid rows contain several items).
    pub fn move_vertical(&mut self, dir: isize, extend: bool, cx: &mut Context<Self>) {
        let step = self.columns_per_row() as isize;
        self.move_cursor(dir * step, extend, cx);
    }

    pub fn cursor_to(&mut self, ix: usize, extend: bool, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            return;
        }
        let ix = ix.min(self.entries.len() - 1);
        let mods = Modifiers { shift: extend, ..Default::default() };
        if !extend {
            self.anchor = Some(ix);
        }
        self.click(ix, mods, cx);
        self.scroll.scroll_to_item(self.row_of(ix), ScrollStrategy::Nearest);
    }

    // --- Rubber-band selection -------------------------------------------------

    pub fn rubber_start(&mut self, position: Point<Pixels>, additive: bool, cx: &mut Context<Self>) {
        if !additive {
            self.selected.clear();
            self.anchor = None;
        }
        self.rubber = Some(Rubber { origin: position, current: position, base: self.selected.clone() });
        cx.notify();
    }

    /// Updates the band and selects every item it touches. Dragging past
    /// the top/bottom edge scrolls the list.
    pub fn rubber_update(&mut self, position: Point<Pixels>, row_height: f32, cx: &mut Context<Self>) {
        let Some(rubber) = &mut self.rubber else { return };
        let handle = self.scroll.0.borrow().base_handle.clone();
        let view = handle.bounds();
        let mut offset = handle.offset();
        let edge = |d: Pixels| (d * 0.5).min(px(40.));
        if position.y > view.bottom() {
            offset.y = (offset.y - edge(position.y - view.bottom())).max(-handle.max_offset().y);
        } else if position.y < view.top() {
            offset.y = (offset.y + edge(view.top() - position.y)).min(px(0.));
        }
        if offset != handle.offset() {
            // Keep the band anchored to the content while scrolling.
            rubber.origin.y += offset.y - handle.offset().y;
            handle.set_offset(offset);
        }
        rubber.current = position;
        let rect = rubber.rect();
        let hits = self.items_in_rect(rect, view, offset, row_height);
        let mut selected = self.rubber.as_ref().map(|r| r.base.clone()).unwrap_or_default();
        selected.extend(hits.iter().map(|&ix| self.entries[ix].path.clone()));
        self.selected = selected;
        if let Some(&last) = hits.last() {
            self.cursor = Some(last);
            self.anchor = hits.first().copied();
        }
        cx.notify();
    }

    pub fn rubber_end(&mut self, cx: &mut Context<Self>) {
        if self.rubber.take().is_some() {
            cx.notify();
        }
    }

    /// Indices of entries whose row/tile intersects `rect` (window coords).
    fn items_in_rect(&self, rect: Bounds<Pixels>, view: Bounds<Pixels>, offset: Point<Pixels>, row_height: f32) -> Vec<usize> {
        let n = self.entries.len();
        if n == 0 || rect.size.height <= px(0.) && rect.size.width <= px(0.) {
            return Vec::new();
        }
        let f = |p: Pixels| -> f32 { p.into() };
        match self.view {
            ViewKind::List | ViewKind::Columns => {
                let top = f(rect.top() - view.top() - offset.y);
                let bottom = f(rect.bottom() - view.top() - offset.y);
                if bottom < 0. {
                    return Vec::new();
                }
                let first = (top.max(0.) / row_height).floor() as usize;
                let last = ((bottom / row_height).floor() as usize).min(n - 1);
                (first..=last).filter(|&i| i < n).collect()
            }
            ViewKind::Grid => {
                use crate::views::grid::{GRID_PADDING_X, GRID_PADDING_Y, TILE_INSET};
                let per_row = self.columns_per_row();
                let (tile_w, tile_h) = (self.tile_width(), self.tile_height);
                let mut hits = Vec::new();
                let top = f(rect.top() - view.top() - offset.y) - GRID_PADDING_Y;
                let bottom = f(rect.bottom() - view.top() - offset.y) - GRID_PADDING_Y;
                let left = f(rect.left() - view.left()) - GRID_PADDING_X;
                let right = f(rect.right() - view.left()) - GRID_PADDING_X;
                let first_row = (top.max(0.) / tile_h).floor() as usize;
                let last_row = (bottom.max(0.) / tile_h).floor() as usize;
                for row in first_row..=last_row {
                    for col in 0..per_row {
                        let ix = row * per_row + col;
                        if ix >= n {
                            break;
                        }
                        // Tile body (excluding its margin) must overlap the band.
                        let (x0, y0) = (col as f32 * tile_w + TILE_INSET, row as f32 * tile_h + TILE_INSET);
                        let (x1, y1) = (x0 + tile_w - 2. * TILE_INSET, y0 + tile_h - 2. * TILE_INSET);
                        if x0 < right && x1 > left && y0 < bottom && y1 > top {
                            hits.push(ix);
                        }
                    }
                }
                hits
            }
        }
    }

    /// Type-ahead: jump to the first entry starting with `prefix`.
    pub fn jump_to_prefix(&mut self, prefix: &str, cx: &mut Context<Self>) {
        let prefix = prefix.to_lowercase();
        let n = self.entries.len();
        if n == 0 {
            return;
        }
        let start = self.cursor.unwrap_or(0);
        let found = (0..n)
            .map(|i| (start + i) % n)
            .find(|&i| self.entries[i].name_lower.starts_with(&prefix));
        if let Some(ix) = found {
            self.cursor_to(ix, false, cx);
        }
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selected = self.entries.iter().map(|e| e.path.clone()).collect();
        cx.notify();
    }

    /// Stops a running search but keeps the results found so far. The
    /// worker still delivers what it already found before it notices.
    pub fn stop_search(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.search.as_mut().filter(|s| s.running) else { return false };
        s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        s.running = false;
        s.stopped = true;
        cx.notify();
        true
    }

    /// Deselect items without changing the active search or filter.
    pub fn deselect(&mut self, cx: &mut Context<Self>) {
        self.selected.clear();
        self.anchor = None;
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        // Esc first stops a running search (keeping its results); pressed
        // again it clears the search as before.
        if self.stop_search(cx) {
            return;
        }
        if !self.filter.is_empty() {
            self.set_filter(String::new(), cx);
        } else if self.search.is_some() && self.selected.is_empty() {
            self.clear_search(cx);
        }
        self.deselect(cx);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle.focus(window, cx);
    }

    /// Focuses the pane and tells the workspace it became active.
    pub fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window, cx);
        if !self.active {
            cx.emit(PaneEvent::Activated);
        }
    }

    /// Decides move vs copy for a drop: alt forces copy, otherwise move within
    /// one filesystem and copy across filesystems (like Finder). Anything
    /// crossing between local disk and a server (or two servers) copies.
    pub fn drop_is_copy(paths: &[PathBuf], src_conn: Option<&str>, dest: &Path, dest_conn: Option<&str>, window: &Window) -> bool {
        if window.modifiers().alt || src_conn != dest_conn {
            return true;
        }
        if src_conn.is_some() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
            let dest_dev = dev(dest);
            paths.iter().any(|p| dev(p) != dest_dev)
        }
        #[cfg(not(unix))]
        {
            paths.iter().any(|p| p.components().next() != dest.components().next())
        }
    }
}

enum SearchMsg {
    Hits(Vec<Entry>),
    Scanned(u64),
    Done(Option<String>),
}

struct CancelOnDrop(Arc<std::sync::atomic::AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Lists a local or remote directory (blocking; background threads only).
fn list_any(conn: Option<&str>, dir: &Path, show_hidden: bool) -> anyhow::Result<Vec<Entry>> {
    match conn {
        Some(conn) => crate::remote::list(conn, &crate::remote::path_str(dir), show_hidden),
        None => Ok(list_dir(dir, show_hidden)?),
    }
}
