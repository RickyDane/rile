mod ai;
mod ai_agent;
pub mod ask;
mod ask_agent;
mod ask_text;
pub use ai_agent::tag_file;
pub mod archive;
pub mod backups;
mod file_ops;
mod inspector;
pub mod nearby;
pub mod open_with;
mod plugins;
pub mod space;
pub mod preview;
mod remote_ops;
mod rewards;
mod search;
mod sources;
mod speedtest;
mod tabs;
pub mod text_edit;
mod updates;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::component::{
    ActiveTheme, TitleBar, WindowExt, h_flex,
    input::{InputEvent, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, MouseButton, NavigationDirection, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Window, div,
    prelude::FluentBuilder, px,
};
use rile_core::FileClipboard;
use rile_core::history::Location;
use rile_core::ops::Progress;
use rile_core::places::{self, Place, Volume};
use rile_core::settings::{Settings, StartLocation, ThemeMode, ViewKind};

use crate::actions::*;
use crate::pane::{Pane, PaneEvent};
use crate::views::WorkspaceHandle;
use crate::{dialogs, sidebar, statusbar, theme, views};

pub use preview::PreviewState;
#[cfg(test)]
pub use inspector::width as inspector_width;
pub use tabs::tab_title;
pub use updates::UpdateState;

/// A background file operation shown in the status bar.
pub struct RunningOp {
    pub id: usize,
    pub label: SharedString,
    pub progress: Progress,
    pub cancel: Arc<AtomicBool>,
    pub started: std::time::Instant,
    /// Hidden from the transfer window ("Run in Background").
    pub backgrounded: bool,
    /// Smoothed transfer rate in bytes per second.
    pub speed: f64,
    last_sample: Option<(std::time::Instant, u64)>,
    _task: Task<()>,
}

impl RunningOp {
    /// Feeds a progress report into the op and its speed estimate.
    pub fn update_progress(&mut self, p: Progress) {
        let now = std::time::Instant::now();
        match self.last_sample {
            Some((t, bytes)) if now.duration_since(t).as_secs_f64() >= 0.4 => {
                let rate = p.bytes_done.saturating_sub(bytes) as f64 / now.duration_since(t).as_secs_f64();
                self.speed = if self.speed == 0. { rate } else { self.speed * 0.6 + rate * 0.4 };
                self.last_sample = Some((now, p.bytes_done));
            }
            Some(_) => {}
            None => self.last_sample = Some((now, p.bytes_done)),
        }
        self.progress = p;
    }

    /// Whether the centered transfer window shows this op: byte transfers
    /// still running after a second, unless sent to the background.
    pub fn in_transfer_window(&self) -> bool {
        !self.backgrounded && self.progress.bytes_total > 0 && self.started.elapsed() >= std::time::Duration::from_secs(1)
    }
}

/// A background job that finished recently (shown in the jobs panel).
pub struct FinishedJob {
    pub label: SharedString,
    /// None on success, else the error (or "Cancelled").
    pub error: Option<SharedString>,
    pub finished: std::time::Instant,
}

/// Root view: sidebar, title/tool bar, pane(s), status bar. Owns app-wide
/// state (settings, clipboard, running operations, plugins, previews).
pub struct Workspace {
    /// The visible pane of each slot (one, or two in dual-pane mode).
    pub panes: Vec<Entity<Pane>>,
    /// Open tabs per slot; `panes[slot]` is always one of `tabs[slot]`.
    pub tabs: Vec<Vec<Entity<Pane>>>,
    pub active_pane: usize,
    pub dual: bool,
    pub(crate) drive_menu: Option<(usize, Entity<gpui_kit::component::menu::PopupMenu>)>,
    pub settings: Settings,
    pub clipboard: FileClipboard,
    /// Connection the clipboard paths belong to (None = local).
    pub clipboard_conn: Option<String>,
    /// Saved FTP/SFTP connections.
    pub connections: Vec<rile_remote::ConnectionInfo>,
    pub places: Vec<Place>,
    pub volumes: Vec<Volume>,
    pub ops: Vec<RunningOp>,
    /// Most recent first, at most a handful.
    pub finished_jobs: Vec<FinishedJob>,
    /// The background jobs panel in the sidebar footer.
    pub jobs_open: bool,
    /// Scroll position of the sidebar's places list (for the footer shadow).
    pub sidebar_scroll: gpui_kit::ScrollHandle,
    /// When a click outside last closed the jobs panel (so the button's own
    /// click doesn't immediately reopen it).
    pub(crate) jobs_closed_at: Option<std::time::Instant>,
    next_op_id: usize,
    pub path_input: Entity<InputState>,
    pub editing_path: bool,
    pub search_input: Entity<InputState>,
    /// Search field focus (for its grow/glow animation) and a serial that
    /// restarts the animation on every change; 0 = never changed.
    pub search_focus: (bool, usize),
    pub preview: PreviewState,
    pub plugins: plugins::PluginState,
    pub space: space::SpaceState,
    pub backups: backups::BackupState,
    /// Other Rile devices on the local network.
    pub nearby: nearby::NearbyState,
    pub updates: UpdateState,
    pub search_form: Option<Entity<search::SearchForm>>,
    /// The ⌘K Ask palette, kept so reopening it continues the conversation.
    pub ask: Option<Entity<ask::AskPalette>>,
    _ask_sub: Option<Subscription>,
    /// Sidebar destination currently under a drag (animated feedback).
    pub sidebar_drop_hover: Option<PathBuf>,
    pub sidebar_visible: bool,
    sidebar_transition: Option<(std::time::Instant, f32)>,
    /// Measured width of each pane slot (0 until first painted). Layout
    /// inside a slot follows this, not the window width.
    pub slot_widths: [f32; 2],
    /// The inspector floating over narrow windows; starts closed there so
    /// it never covers the files by default.
    pub inspector_overlay: bool,
    /// Live statistics for the Overview start page.
    pub overview: crate::views::home::OverviewState,
    pub rewards: rewards::RewardState,
    /// Id of the running "Describing photos" job, if any.
    pub photo_job: Option<usize>,
    /// The photo library, created when first opened.
    pub gallery: Option<Entity<crate::views::gallery::GalleryView>>,
    /// Grid zoom slider in the status bar.
    pub zoom: Entity<gpui_kit::component::slider::SliderState>,
    /// A press on a window-drag region that may become a window move.
    #[cfg(not(target_os = "macos"))]
    pub(crate) drag_armed: bool,
    /// The sidebar edge is being dragged (saved on release).
    resizing_sidebar: bool,
    _spring_task: Option<Task<()>>,
    _volumes_task: Option<Task<()>>,
    _volume_watch_task: Option<Task<()>>,
    _index_task: Option<Task<()>>,
    _dialog_sub: Option<Subscription>,
    _pane_subs: std::collections::HashMap<gpui_kit::EntityId, [Subscription; 2]>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(start: Location, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut settings = Settings::load();
        let start = match (&start, settings.start_location, &settings.last_location, &settings.start_dir) {
            (Location::Home, StartLocation::LastVisited, Some(last), _) if last.is_dir() => Location::Dir(last.clone()),
            (Location::Home, StartLocation::Custom, _, Some(dir)) if dir.is_dir() => Location::Dir(dir.clone()),
            _ => start,
        };
        settings.last_location = None;
        let path_input = cx.new(|cx| InputState::new(window, cx).placeholder("Go to folder…"));
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search this folder"));
        let zoom = cx.new(|_| {
            gpui_kit::component::slider::SliderState::new()
                .min(views::grid::MIN_ICON)
                .max(views::grid::MAX_ICON)
                .step(4.)
                .default_value(settings.grid_icon_size.clamp(views::grid::MIN_ICON, views::grid::MAX_ICON))
        });
        let handle = WorkspaceHandle(cx.entity().downgrade());
        cx.set_global(handle);

        // Escape cancels an in-app drag. An interceptor runs before key
        // bindings, so the pane's own Escape action (deselect) can't eat it.
        // Once a drag leaves the window, macOS owns it and handles Escape.
        let weak = cx.entity().downgrade();
        let cancel_drag = cx.intercept_keystrokes(move |ev, window, cx| {
            if ev.keystroke.key == "escape" && cx.stop_active_drag(window) {
                weak.update(cx, |this, cx| this.clear_drop_hover(cx)).ok();
                cx.stop_propagation();
            }
        });
        let subscriptions = vec![
            cancel_drag,
            cx.subscribe_in(&path_input, window, Self::on_path_input_event),
            cx.subscribe_in(&search_input, window, Self::on_search_input_event),
            cx.subscribe_in(&zoom, window, Self::on_zoom_event),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.settings.theme == ThemeMode::System {
                    theme::apply(&this.settings, window, cx);
                }
            }),
        ];

        let mut this = Self {
            panes: Vec::new(),
            tabs: Vec::new(),
            active_pane: 0,
            dual: false,
            drive_menu: None,
            settings,
            clipboard: FileClipboard::default(),
            clipboard_conn: None,
            connections: crate::remote::load_saved(),
            places: places::standard_places(),
            volumes: Vec::new(),
            ops: Vec::new(),
            finished_jobs: Vec::new(),
            jobs_open: false,
            sidebar_scroll: gpui_kit::ScrollHandle::new(),
            jobs_closed_at: None,
            next_op_id: 0,
            path_input,
            editing_path: false,
            search_input,
            search_focus: (false, 0),
            preview: PreviewState::default(),
            plugins: plugins::PluginState::default(),
            space: space::SpaceState::default(),
            backups: backups::BackupState::load(),
            nearby: Default::default(),
            updates: UpdateState::default(),
            search_form: None,
            ask: None,
            _ask_sub: None,
            sidebar_drop_hover: None,
            sidebar_visible: true,
            sidebar_transition: None,
            slot_widths: [0.; 2],
            inspector_overlay: false,
            overview: Default::default(),
            rewards: Default::default(),
            gallery: None,
            photo_job: None,
            zoom,
            #[cfg(not(target_os = "macos"))]
            drag_armed: false,
            resizing_sidebar: false,
            _spring_task: None,
            _volumes_task: None,
            _volume_watch_task: None,
            _index_task: None,
            _dialog_sub: None,
            _pane_subs: Default::default(),
            _subscriptions: subscriptions,
        };
        this.add_pane(start, window, cx);
        if this.settings.dual_pane {
            this.set_dual(true, window, cx);
        }
        theme::apply(&this.settings, window, cx);
        rile_services::index::set_enabled(this.settings.index_discovered);
        this._index_task = Some(Self::save_index_periodically(cx));
        this.rewards.task = Some(Self::watch_rewards(cx));
        this.refresh_volumes(cx);
        this._volume_watch_task = Some(Self::watch_volumes(cx));
        this.load_plugins(cx);
        this.start_backups(window, cx);
        this.sync_nearby(window, cx);
        this.sync_local_model();
        // Describe new photos in the background once the app has settled.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(20)).await;
            this.update_in(cx, |this, window, cx| this.start_photo_job(false, window, cx)).ok();
        })
        .detach();
        if this.settings.update_check_on_start {
            this.check_for_updates(false, window, cx);
        }
        let focus = this.pane().read(cx).focus_handle.clone();
        focus.focus(window, cx);
        this
    }

    /// Creates a pane wired to the workspace (events + re-render on change).
    fn create_pane(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) -> Entity<Pane> {
        let s = &self.settings;
        let (hidden, key, order, view) = (s.show_hidden, s.sort_key, s.sort_order, s.default_view);
        let view = if self.dual { ViewKind::List } else { view };
        let pane = cx.new(|cx| Pane::new(location, hidden, key, order, view, cx));
        let subs = [
            cx.subscribe_in(&pane, window, Self::on_pane_event),
            cx.observe(&pane, |this, pane, cx| {
                if pane == *this.pane() {
                    this.sync_preview(cx);
                }
                cx.notify();
            }),
        ];
        self._pane_subs.insert(pane.entity_id(), subs);
        pane
    }

    /// Adds a new slot (dual pane) showing `location` in a single tab.
    fn add_pane(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) -> Entity<Pane> {
        let pane = self.create_pane(location, window, cx);
        self.panes.push(pane.clone());
        self.tabs.push(vec![pane.clone()]);
        self.sync_active_flags(cx);
        pane
    }

    /// Every open pane, including those in background tabs.
    pub fn all_panes(&self) -> Vec<Entity<Pane>> {
        self.tabs.iter().flatten().cloned().collect()
    }

    pub fn pane(&self) -> &Entity<Pane> {
        &self.panes[self.active_pane]
    }

    pub fn other_pane(&self) -> Option<&Entity<Pane>> {
        if !self.dual {
            return None;
        }
        self.panes.get(1 - self.active_pane)
    }

    fn sync_active_flags(&self, cx: &mut App) {
        let dual = self.dual;
        for (i, pane) in self.panes.iter().enumerate() {
            let active = i == self.active_pane || !dual;
            pane.update(cx, |p, cx| {
                if p.active != active {
                    p.active = active;
                    cx.notify();
                }
            });
        }
    }

    pub fn set_active_pane(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.panes.len() || (ix != 0 && !self.dual) {
            return;
        }
        self.active_pane = ix;
        self.sync_active_flags(cx);
        let focus = self.pane().read(cx).focus_handle.clone();
        focus.focus(window, cx);
        self.editing_path = false;
        self.sync_search_input(window, cx);
        self.sync_preview(cx);
        cx.notify();
    }

    /// The search field is shared: show the active pane's filter in it.
    pub(crate) fn sync_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let filter = self.pane().read(cx).filter.clone();
        if self.search_input.read(cx).value().as_ref() != filter.as_str() {
            self.search_input.update(cx, |i, cx| i.set_value(filter, window, cx));
        }
    }

    pub fn set_dual(&mut self, dual: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.dual = dual;
        self.drive_menu = None;
        if dual && self.panes.len() < 2 {
            let loc = self.pane().read(cx).location.clone();
            self.add_pane(loc, window, cx);
        }
        if !dual && self.active_pane != 0 {
            self.panes.swap(0, 1);
            self.tabs.swap(0, 1);
            self.active_pane = 0;
        }
        let view = if dual { ViewKind::List } else { self.settings.default_view };
        for pane in self.all_panes() {
            pane.update(cx, |pane, cx| pane.set_view(view, cx));
        }
        self.sync_active_flags(cx);
        self.settings.dual_pane = dual;
        self.save_settings(window, cx);
        cx.notify();
    }

    /// Updates which sidebar destination is under the drag. Holding a drag
    /// over one for a moment springs it open in the active pane.
    pub fn track_drop_hover(&mut self, target: &Path, inside: bool, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.sidebar_drop_hover.as_deref() == Some(target);
        if inside && !current {
            self.sidebar_drop_hover = Some(target.to_path_buf());
            let target = target.to_path_buf();
            self._spring_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(1100)).await;
                this.update_in(cx, |this, window, cx| {
                    if this.sidebar_drop_hover.as_ref() == Some(&target) && cx.has_active_drag() && target.is_dir() {
                        this.open_location(Location::Dir(target), window, cx);
                    }
                })
                .ok();
            }));
            cx.notify();
        } else if !inside && current {
            self.clear_drop_hover(cx);
        }
    }

    pub fn clear_drop_hover(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_drop_hover.take().is_some() {
            self._spring_task = None;
            cx.notify();
        }
    }

    pub fn current_dir(&self, cx: &App) -> Option<PathBuf> {
        self.pane().read(cx).current_dir().map(Path::to_path_buf)
    }

    pub fn selection(&self, cx: &App) -> Vec<PathBuf> {
        self.pane().read(cx).selected_paths()
    }

    fn on_pane_event(&mut self, pane: &Entity<Pane>, event: &PaneEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PaneEvent::Navigated => {
                if pane != self.pane() {
                    return;
                }
                self.editing_path = false;
                let at_home = matches!(pane.read(cx).location, Location::Home);
                if at_home {
                    self.refresh_overview(cx);
                }
                self.search_input.update(cx, |i, cx| i.set_value("", window, cx));
                self.update_window_title(window, cx);
            }
            PaneEvent::Activated => {
                if let Some(ix) = self.panes.iter().position(|p| p == pane) {
                    self.set_active_pane(ix, window, cx);
                }
            }
            PaneEvent::Drop { paths, src_conn, dest, dest_conn, copy } => {
                if self.dual {
                    self.confirm_transfer(paths.clone(), src_conn.clone(), dest.clone(), dest_conn.clone(), *copy, window, cx);
                } else {
                    self.transfer_between(paths.clone(), src_conn.clone(), dest.clone(), dest_conn.clone(), *copy, window, cx);
                }
            }
            PaneEvent::DropExternal { paths, dest, dest_conn } => {
                self.transfer_between(paths.clone(), None, dest.clone(), dest_conn.clone(), true, window, cx);
            }
            PaneEvent::OpenRemote { conn, path } => {
                self.open_remote(conn.clone(), path.clone(), window, cx);
            }
        }
    }

    fn on_path_input_event(&mut self, input: &Entity<InputState>, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            InputEvent::PressEnter { .. } => {
                let raw = input.read(cx).value().trim().to_string();
                if let Some(conn) = self.pane().read(cx).conn()
                    && raw.starts_with('/')
                {
                    self.open_location(Location::Remote { conn, path: raw }, window, cx);
                    return;
                }
                let path = expand_tilde(&raw);
                if path.is_dir() {
                    self.open_location(Location::Dir(path), window, cx);
                } else if path.is_file() {
                    open_with::open_file(&path, cx);
                } else {
                    window.push_notification(Notification::error(format!("No folder at \"{raw}\"")), cx);
                }
            }
            InputEvent::Blur => {
                self.editing_path = false;
                cx.notify();
            }
            _ => {}
        }
    }

    pub fn open_location(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_path = false;
        self.pane().update(cx, |pane, cx| {
            pane.navigate(location, cx);
            pane.focus(window, cx);
        });
        cx.notify();
    }

    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.open_location(Location::Dir(path), window, cx);
        } else {
            open_with::open_file(&path, cx);
        }
    }

    pub fn refresh_volumes(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async { places::volumes() });
        self._volumes_task = Some(cx.spawn(async move |this, cx| {
            let volumes = task.await;
            this.update(cx, |this, cx| {
                this.volumes = volumes;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Polls mounted volumes so drives attached/detached show up within 2s, and refreshes
    /// drive usage (free/total space) every 10s.
    fn watch_volumes(cx: &mut Context<Self>) -> Task<()> {
        const USAGE_EVERY_TICKS: u32 = 5; // 5 × 2s = 10s
        cx.spawn(async move |this, cx| {
            let mut tick: u32 = 0;
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                tick = tick.wrapping_add(1);
                let check_usage = tick % USAGE_EVERY_TICKS == 0;
                let volumes = cx.background_spawn(async { places::volumes() }).await;
                let Ok(changed) = this.read_with(cx, |this, _| {
                    this.volumes.len() != volumes.len()
                        || this.volumes.iter().zip(&volumes).any(|(a, b)| {
                            a.mount_point != b.mount_point
                                || a.name != b.name
                                || (check_usage && (a.available != b.available || a.total != b.total))
                        })
                }) else {
                    break;
                };
                if changed {
                    let ok = this
                        .update(cx, |this, cx| {
                            this.volumes = volumes;
                            cx.notify();
                        })
                        .is_ok();
                    if !ok {
                        break;
                    }
                }
            }
        })
    }

    pub fn save_settings(&self, window: &mut Window, cx: &mut App) {
        if let Err(e) = self.settings.save() {
            window.push_notification(Notification::error(format!("Couldn't save settings: {e}")), cx);
        }
    }

    /// Applies appearance settings and persists everything.
    pub fn settings_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = Settings::default();
        if self.settings.theme != defaults.theme || self.settings.accent != defaults.accent || self.settings.icon_set != defaults.icon_set {
            self.reward_event(rile_services::rewards::Event::Personalized, cx);
        }
        theme::apply(&self.settings, window, cx);
        // Panes cache their render; icons and colors may have changed.
        for pane in &self.all_panes() {
            pane.update(cx, |_, cx| cx.notify());
        }
        rile_services::index::set_enabled(self.settings.index_discovered);
        let size = self.settings.grid_icon_size.clamp(views::grid::MIN_ICON, views::grid::MAX_ICON);
        self.zoom.update(cx, |z, cx| z.set_value(size, window, cx));
        let hidden = self.settings.show_hidden;
        for pane in &self.all_panes() {
            pane.update(cx, |p, cx| p.set_show_hidden(hidden, cx));
        }
        self.sync_preview(cx);
        self.sync_local_model();
        self.sync_nearby(window, cx);
        self.save_settings(window, cx);
        cx.notify();
    }

    pub fn set_theme(&mut self, mode: ThemeMode, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.theme = mode;
        self.settings_changed(window, cx);
    }

    pub fn set_show_hidden(&mut self, show: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.show_hidden = show;
        for pane in &self.all_panes() {
            pane.update(cx, |p, cx| p.set_show_hidden(show, cx));
        }
        self.save_settings(window, cx);
        cx.notify();
    }

    pub fn set_view(&mut self, view: ViewKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.dual { return; }
        self.pane().update(cx, |p, cx| p.set_view(view, cx));
        self.settings.default_view = view;
        self.save_settings(window, cx);
        cx.notify();
    }

    /// Moves a sidebar section to where `target` is and saves the order.
    pub fn move_sidebar_section(
        &mut self,
        section: rile_core::settings::SidebarSection,
        target: rile_core::settings::SidebarSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if section == target {
            return;
        }
        self.settings.move_sidebar_section(section, target);
        self.save_settings(window, cx);
        cx.notify();
    }

    /// Collapses or expands a sidebar section and saves the choice.
    pub fn toggle_sidebar_section(&mut self, section: rile_core::settings::SidebarSection, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.toggle_sidebar_section(section);
        self.save_settings(window, cx);
        cx.notify();
    }

    pub fn toggle_favorite_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let added = !self.settings.favorites.contains(&path);
        self.settings.toggle_favorite(path);
        if added {
            self.reward_event(rile_services::rewards::Event::Personalized, cx);
        }
        self.save_settings(window, cx);
        cx.notify();
    }

    /// Writes the discovery index to disk every minute if it changed.
    fn save_index_periodically(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(60)).await;
                cx.background_spawn(async {
                    rile_services::index::save().ok();
                    rile_services::tags::save().ok();
                })
                .await;
            }
        })
    }

    /// Remembers the last folder for the "start where I left off" option.
    pub fn on_quit(&mut self, cx: &mut App) {
        rile_services::local_model::shutdown();
        rile_services::index::save().ok();
        rile_services::tags::save().ok();
        if let Some(dir) = self.current_dir(cx) {
            self.settings.last_location = Some(dir);
            self.settings.save().ok();
        }
    }

    // --- Navigation / view actions -------------------------------------------

    fn go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        self.pane().update(cx, |p, cx| p.go_back(cx));
    }

    fn go_forward(&mut self, _: &GoForward, _: &mut Window, cx: &mut Context<Self>) {
        self.pane().update(cx, |p, cx| p.go_forward(cx));
    }

    fn go_up(&mut self, _: &GoUp, _: &mut Window, cx: &mut Context<Self>) {
        self.pane().update(cx, |p, cx| p.go_up(cx));
    }

    fn go_home(&mut self, _: &GoHome, window: &mut Window, cx: &mut Context<Self>) {
        self.open_location(Location::Home, window, cx);
    }

    /// The user's home folder (the toolbar's house button), unlike
    /// [`Self::go_home`] which opens the Overview.
    pub(crate) fn go_home_folder(&mut self, _: &GoHomeFolder, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(home) = dirs_home() {
            self.open_location(Location::Dir(home), window, cx);
        }
    }

    fn refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.pane().update(cx, |p, cx| p.reload(cx));
        self.refresh_volumes(cx);
    }

    pub fn edit_path(&mut self, _: &EditPath, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.pane().read(cx).dir_path().map(|p| p.display().to_string()).unwrap_or_default();
        self.editing_path = true;
        self.path_input.update(cx, |input, cx| {
            input.set_value(value, window, cx);
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        cx.notify();
    }

    fn toggle_hidden(&mut self, _: &ToggleHidden, window: &mut Window, cx: &mut Context<Self>) {
        self.set_show_hidden(!self.settings.show_hidden, window, cx);
    }

    pub fn toggle_theme(&mut self, _: &ToggleTheme, window: &mut Window, cx: &mut Context<Self>) {
        let next = if cx.theme().is_dark() { ThemeMode::Light } else { ThemeMode::Dark };
        self.set_theme(next, window, cx);
    }

    pub fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        dialogs::open_settings(cx.entity(), self.settings.clone(), window, cx);
    }

    fn show_shortcuts(&mut self, _: &ShowShortcuts, window: &mut Window, cx: &mut Context<Self>) {
        dialogs::open_shortcuts(window, cx);
    }

    fn open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.pane().update(cx, |p, cx| p.open_selected(cx));
    }

    fn view_list(&mut self, _: &ViewList, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewKind::List, window, cx);
    }

    fn view_grid(&mut self, _: &ViewGrid, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewKind::Grid, window, cx);
    }

    fn view_columns(&mut self, _: &ViewColumns, window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(ViewKind::Columns, window, cx);
    }

    pub(crate) fn toggle_dual(&mut self, _: &ToggleDualPane, window: &mut Window, cx: &mut Context<Self>) {
        self.set_dual(!self.dual, window, cx);
    }

    fn switch_pane(&mut self, _: &SwitchPane, window: &mut Window, cx: &mut Context<Self>) {
        if self.dual {
            self.set_active_pane(1 - self.active_pane, window, cx);
        }
    }

    fn open_in_other_pane(&mut self, _: &OpenInOtherPane, window: &mut Window, cx: &mut Context<Self>) {
        let target = {
            let pane = self.pane().read(cx);
            pane.cursor_entry()
                .map(|e| if e.is_dir { e.path.clone() } else { e.path.parent().map(Path::to_path_buf).unwrap_or_default() })
                .or_else(|| pane.dir_path())
                .map(|t| pane.location_for(t))
        };
        let Some(target) = target else { return };
        if !self.dual {
            self.set_dual(true, window, cx);
        }
        if let Some(other) = self.other_pane().cloned() {
            other.update(cx, |p, cx| p.navigate(target, cx));
        }
    }

    fn render_pane_slot(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.panes[ix].clone();
        let location = pane.read(cx).location.clone();
        let content = if location == Location::Home {
            views::home::render(self, ix, window, cx).into_any_element()
        } else if location == Location::Gallery {
            views::gallery::render(self, ix, window, cx)
        } else if location == Location::Backups {
            views::backups::render(self, ix, window, cx)
        } else if location == Location::Devices {
            views::devices::render(self, ix, window, cx)
        } else if let Location::SpaceAnalyzer(focus) = &location {
            views::space_analyzer::render(self, ix, focus.clone(), window, cx)
        } else if let Location::PluginPage { plugin, page, .. } = &location {
            views::plugin_page::render(self, ix, plugin, page, window, cx)
        } else {
            views::chrome::render_browser(self, pane, ix, window, cx).into_any_element()
        };
        let hairline = crate::theme::chrome(cx).hairline;
        let ws = cx.entity();
        v_flex()
            .id(("pane-slot", ix))
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(crate::theme::chrome(cx).browser)
            .when(self.dual && ix == 1, |d| d.border_l_1().border_color(hairline))
            .when(self.dual && matches!(location, Location::Home | Location::Gallery | Location::SpaceAnalyzer(_) | Location::Backups | Location::Devices | Location::PluginPage { .. }), |d| {
                d.child(h_flex().px(px(12.)).py(px(4.))
                    .when(ix == 1, |d| d.justify_end())
                    .child(self.render_drive_selector(ix, cx)))
            })
            .child(content)
            // Records the slot's width so its layout can adapt to it.
            .child(
                gpui_kit::canvas(
                    move |bounds, _, cx| {
                        let w: f32 = bounds.size.width.into();
                        ws.update(cx, |ws, cx| {
                            if (ws.slot_widths[ix] - w).abs() > 1. {
                                ws.slot_widths[ix] = w;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }

    /// Width available to a pane slot: measured once painted, estimated
    /// from the window and the visible chrome before that.
    pub fn slot_width(&self, slot: usize, window: &Window) -> f32 {
        let measured = self.slot_widths.get(slot).copied().unwrap_or(0.);
        if measured > 0. {
            return measured;
        }
        let mut w: f32 = window.viewport_size().width.into();
        w -= self.sidebar_display_width(window);
        if self.inspector_inline(window) {
            w -= inspector::width(window);
        }
        if self.dual { w / 2. } else { w }
    }

    /// Whether the inspector is shown beside the files (wide windows).
    pub fn inspector_inline(&self, window: &Window) -> bool {
        self.settings.show_preview_panel && views::chrome::width_class(window) > views::chrome::Width::Phone
    }

    /// Whether the inspector is visible at all (inline or floating).
    pub fn inspector_visible(&self, window: &Window) -> bool {
        if views::chrome::width_class(window) <= views::chrome::Width::Phone {
            self.inspector_overlay
        } else {
            self.settings.show_preview_panel
        }
    }

    pub(crate) fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let from = self.sidebar_fraction(now);
        self.sidebar_visible = !self.sidebar_visible;
        self.sidebar_transition = Some((now, from));
        cx.notify();
    }

    fn sidebar_fraction(&self, now: std::time::Instant) -> f32 {
        let target = if self.sidebar_visible { 1. } else { 0. };
        let Some((started, from)) = self.sidebar_transition else { return target };
        let t = (now.duration_since(started).as_secs_f32() / 0.2).clamp(0., 1.);
        // Cubic ease-out; reversals start at the currently displayed width.
        from + (target - from) * (1. - (1. - t).powi(3))
    }

    pub(crate) fn sidebar_display_width(&self, window: &Window) -> f32 {
        sidebar::width(self, window) * self.sidebar_fraction(std::time::Instant::now())
    }

    fn on_zoom_event(
        &mut self,
        _: &Entity<gpui_kit::component::slider::SliderState>,
        event: &gpui_kit::component::slider::SliderEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::slider::SliderEvent;
        let (SliderEvent::Change(v) | SliderEvent::Release(v)) = event;
        let size = v.start();
        if (self.settings.grid_icon_size - size).abs() < 0.5 && matches!(event, SliderEvent::Change(_)) {
            return;
        }
        self.settings.grid_icon_size = size;
        if matches!(event, SliderEvent::Release(_)) {
            self.save_settings(window, cx);
        }
        theme::apply(&self.settings, window, cx);
        for pane in self.all_panes() {
            pane.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Closing a dialog can leave nothing focused, which would silence all
        // shortcuts until the next click; hand focus back to the active pane.
        if window.focused(cx).is_none() && !window.has_active_dialog(cx) {
            let focus = self.pane().read(cx).focus_handle.clone();
            window.defer(cx, move |window, cx| {
                if window.focused(cx).is_none() {
                    focus.focus(window, cx);
                }
            });
        }
        let fg = cx.theme().foreground;
        let ch = crate::theme::chrome(cx);
        let width = views::chrome::width_class(window);
        let slots: Vec<_> =
            (0..if self.dual { 2 } else { 1 }).map(|i| self.render_pane_slot(i, window, cx).into_any_element()).collect();
        // The photo library and the Space Analyzer show their own details.
        let in_gallery = matches!(self.pane().read(cx).location, Location::Gallery | Location::SpaceAnalyzer(_) | Location::Backups | Location::Devices);
        let inspector = (self.inspector_visible(window) && !in_gallery).then(|| self.render_inspector(window, cx));
        let (inline_inspector, overlay_inspector) =
            if width <= views::chrome::Width::Phone { (None, inspector) } else { (inspector, None) };
        let quicklook = self.preview.quicklook.then(|| self.render_quicklook(window, cx));
        let transfers = crate::jobs::render_transfer_window(self, window, cx);
        let jobs_panel = self.jobs_open.then(|| {
            div()
                .absolute()
                .right(px(10.))
                .bottom(px(36.))
                .child(crate::jobs::render_panel(self, window, cx))
        });
        if let Some((started, _)) = self.sidebar_transition {
            if started.elapsed() >= std::time::Duration::from_millis(200) {
                self.sidebar_transition = None;
            } else {
                cx.on_next_frame(window, |_, _, cx| cx.notify());
            }
        }
        let sidebar_width = self.sidebar_display_width(window);
        let sidebar = (self.sidebar_visible || self.sidebar_transition.is_some()).then(|| {
            div()
                .w(px(sidebar_width))
                .h_full()
                .flex_none()
                .overflow_hidden()
                .child(sidebar::render(self, window, cx))
                .into_any_element()
        });
        let toolbar = views::chrome::render_toolbar(self, window, cx);
        let tabs = views::chrome::render_tab_strip(self, window, cx);
        let root_picker = views::plugin_page::root_picker_bar(self, cx);
        let status = statusbar::render(self, window, cx);

        let main = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .child(toolbar)
            .child(tabs)
            .children(root_picker)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(h_flex().flex_1().min_w_0().h_full().children(slots))
                    .children(inline_inspector),
            )
            .child(status)
            .when_some(overlay_inspector, |d, inspector| {
                d.child(
                    div()
                        .id("inspector-overlay")
                        .occlude()
                        // A click elsewhere dismisses the floating inspector.
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.inspector_overlay = false;
                            cx.notify();
                        }))
                        .absolute()
                        .right_0()
                        .top(px(views::chrome::TOOLBAR_HEIGHT + views::chrome::TAB_STRIP_HEIGHT))
                        .bottom(px(31.))
                        .shadow_2xl()
                        .child(inspector),
                )
            });

        v_flex()
            .size_full()
            .relative()
            .bg(ch.window)
            .text_color(fg)
            .key_context("Workspace")
            .on_drag_move(cx.listener(|_this, _event: &gpui_kit::DragMoveEvent<crate::pane::DraggedFiles>, _window, _cx| {
                #[cfg(any(target_os = "macos", windows))]
                if crate::native_drag::handoff(_event, _window, _cx) {
                    _this.clear_drop_hover(_cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.clear_drop_hover(cx);
                if std::mem::take(&mut this.resizing_sidebar) {
                    this.save_settings(window, cx);
                }
            }))
            .on_drag_move(cx.listener(|this, ev: &gpui_kit::DragMoveEvent<sidebar::SidebarResize>, window, cx| {
                let x: f32 = ev.event.position.x.into();
                let max = (f32::from(window.viewport_size().width) * 0.45).max(sidebar::MIN_WIDTH);
                let w = x.clamp(sidebar::MIN_WIDTH, sidebar::MAX_WIDTH.min(max)).round();
                if this.settings.sidebar_width != Some(w) {
                    this.settings.sidebar_width = Some(w);
                    this.resizing_sidebar = true;
                    cx.notify();
                }
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.clear_drop_hover(cx)))
            // Thumb buttons anywhere outside a pane (home page, sidebar) act on the active pane.
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, w, cx| this.go_back(&GoBack, w, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, w, cx| this.go_forward(&GoForward, w, cx)),
            )
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(Self::go_up))
            .on_action(cx.listener(Self::go_home))
            .on_action(cx.listener(Self::go_home_folder))
            .on_action(cx.listener(Self::compress))
            .on_action(cx.listener(Self::smart_tag))
            .on_action(cx.listener(Self::extract))
            .on_action(cx.listener(Self::refresh))
            .on_action(cx.listener(Self::edit_path))
            .on_action(cx.listener(Self::toggle_hidden))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::show_shortcuts))
            .on_action(cx.listener(Self::open_selected))
            .on_action(cx.listener(Self::view_list))
            .on_action(cx.listener(Self::view_grid))
            .on_action(cx.listener(Self::view_columns))
            .on_action(cx.listener(Self::toggle_dual))
            .on_action(cx.listener(Self::switch_pane))
            .on_action(cx.listener(Self::select_left_drive))
            .on_action(cx.listener(Self::select_right_drive))
            .on_action(cx.listener(Self::open_in_other_pane))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy_path))
            .on_action(cx.listener(Self::duplicate))
            .on_action(cx.listener(Self::move_to_trash))
            .on_action(cx.listener(Self::delete_permanently))
            .on_action(cx.listener(Self::rename))
            .on_action(cx.listener(Self::new_folder))
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::toggle_favorite))
            .on_action(cx.listener(Self::copy_to_other))
            .on_action(cx.listener(Self::move_to_other))
            .on_action(cx.listener(Self::toggle_quicklook))
            .on_action(cx.listener(Self::close_preview))
            .on_action(cx.listener(Self::toggle_preview_panel))
            .on_action(cx.listener(Self::show_properties))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::advanced_search))
            .on_action(cx.listener(Self::ai_ask))
            .on_action(cx.listener(Self::ai_summarize))
            .on_action(cx.listener(Self::check_updates_action))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_tab_action))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::open_in_new_tab))
            .on_action(cx.listener(Self::toggle_sidebar))
            // Linux keeps a drawn title bar for the window controls; Windows
            // puts its caption buttons in the toolbar (chrome::caption_buttons).
            .when(cfg!(target_os = "linux"), |d| d.child(TitleBar::new()))
            .child(h_flex().flex_1().min_h_0().children(sidebar).child(main))
            .children(transfers)
            .children(jobs_panel)
            .children(quicklook)
            .children(rewards::celebration(self, cx))
    }
}

pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn item_count(n: usize) -> String {
    if n == 1 { "1 item".into() } else { format!("{n} items") }
}

pub fn dirs_home() -> Option<PathBuf> {
    dirs::home_dir().filter(|p| p.is_dir())
}

pub fn expand_tilde(raw: &str) -> PathBuf {
    match raw.strip_prefix('~') {
        Some(rest) => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR_STR))
            .join(rest.trim_start_matches(['/', std::path::MAIN_SEPARATOR])),
        None => PathBuf::from(raw),
    }
}

