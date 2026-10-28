//! WASM plugin integration: loading, context-menu commands, actions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui_kit::component::{WindowExt, notification::Notification};
use gpui_kit::{App, AppContext, ClipboardItem, Context, EntityId, SharedString, Task, Window};
use rile_core::history::Location;
use rile_plugins::{Action, Context as PluginContext, Page, PageRequest, PageView, PluginHost, PluginInfo};

use super::Workspace;

#[derive(Default)]
pub struct PluginState {
    pub host: Option<Arc<Mutex<PluginHost>>>,
    pub infos: Vec<PluginInfo>,
    pub errors: Vec<SharedString>,
    pub loading: bool,
    /// Pages contributed by enabled plugins: (plugin id, page).
    pub pages: Vec<(String, Page)>,
    /// Live page sessions keyed by (plugin id, page id); shared by all panes.
    pub sessions: HashMap<(String, String), PageSession>,
    root_picker: Option<RootPicker>,
    _task: Option<Task<()>>,
}

struct RootPicker {
    /// The plugin page asking, or `None` for the Space Analyzer.
    key: Option<(String, String)>,
    pane: EntityId,
    previous: Location,
}

/// The host-side state of an open plugin page.
#[derive(Default)]
pub struct PageSession {
    pub root: Option<PathBuf>,
    /// Last local explorer directory, suggested for browsing, never auto-scanned.
    pub browse_from: Option<PathBuf>,
    /// Opaque plugin state, handed back on every call.
    pub state: String,
    pub view: Option<PageView>,
    pub error: Option<SharedString>,
    /// A call is in flight; further events are dropped until it returns.
    pub busy: bool,
    /// Host-only chart zoom, in quarter steps above 100%; never invokes WASM.
    pub bubble_zoom: u8,
}

/// The Space Analyzer used to ship as a WASM plugin; it is built in now, so an
/// old copy left in the plugin folder must not add a second sidebar entry.
const RETIRED_PLUGINS: &[&str] = &["rile.space-analyzer"];

fn visible_pages(host: &mut PluginHost) -> Vec<(String, Page)> {
    host.pages().into_iter().filter(|(id, _)| !RETIRED_PLUGINS.contains(&id.as_str())).collect()
}

impl Workspace {
    /// Compiles plugins from the plugin folder on a background thread.
    pub fn load_plugins(&mut self, cx: &mut Context<Self>) {
        self.plugins.host = None;
        self.plugins.infos.clear();
        self.plugins.errors.clear();
        self.plugins.pages.clear();
        self.plugins.sessions.clear();
        if !self.settings.plugins_enabled {
            cx.notify();
            return;
        }
        let Some(dir) = rile_plugins::default_plugin_dir() else { return };
        let disabled = self.settings.disabled_plugins.clone();
        self.plugins.loading = true;
        let task = cx.background_spawn(async move {
            std::fs::create_dir_all(&dir).ok();
            let mut host = PluginHost::new()?;
            let errors = host.load_dir(&dir);
            for id in &disabled {
                host.set_enabled(id, false);
            }
            let pages = visible_pages(&mut host);
            anyhow::Ok((host, errors, pages))
        });
        self.plugins._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.plugins.loading = false;
                match result {
                    Ok((host, errors, pages)) => {
                        this.plugins.infos = host.plugins();
                        this.plugins.pages = pages;
                        this.plugins.errors = errors.into_iter().map(|e| format!("{e:#}").into()).collect();
                        this.plugins.host = Some(Arc::new(Mutex::new(host)));
                    }
                    Err(e) => this.plugins.errors = vec![format!("Plugin runtime failed: {e:#}").into()],
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn set_plugin_enabled(&mut self, id: &str, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.disabled_plugins.retain(|d| d != id);
        if !enabled {
            self.settings.disabled_plugins.push(id.to_string());
        }
        if let Some(host) = &self.plugins.host
            && let Ok(mut h) = host.lock()
        {
            h.set_enabled(id, enabled);
            self.plugins.infos = h.plugins();
            self.plugins.pages = visible_pages(&mut h);
        }
        self.plugins.sessions.retain(|(pid, _), _| pid != id);
        self.save_settings(window, cx);
        cx.notify();
    }

    /// (plugin id, command id, label) for the context menu.
    pub fn plugin_commands(&self, current_dir: Option<PathBuf>, selection: Vec<PathBuf>) -> Vec<(String, String, String)> {
        let Some(host) = &self.plugins.host else { return Vec::new() };
        let Ok(mut host) = host.try_lock() else { return Vec::new() };
        let ctx = PluginContext { current_dir, selection };
        host.commands(&ctx).into_iter().map(|(pid, c)| (pid, c.id, c.label)).collect()
    }

    pub fn run_plugin_command(
        &mut self,
        plugin_id: &str,
        command_id: &str,
        current_dir: Option<PathBuf>,
        selection: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.plugins.host.clone() else { return };
        let (pid, cid) = (plugin_id.to_string(), command_id.to_string());
        let task = cx.background_spawn(async move {
            let ctx = PluginContext { current_dir, selection };
            host.lock().map_err(|_| anyhow::anyhow!("plugin host poisoned"))?.run_command(&pid, &cid, &ctx)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(actions) => this.apply_plugin_actions(actions, window, cx),
                Err(e) => window.push_notification(Notification::error(format!("Plugin error: {e:#}")), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Opens a plugin page without implicitly granting it a scan root. Pages
    /// ask the user to choose a directory before doing filesystem work.
    pub fn open_plugin_page(&mut self, plugin: &str, page: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, info)) = self.plugins.pages.iter().find(|(p, pg)| p == plugin && pg.id == page) else { return };
        let location = Location::PluginPage { plugin: plugin.to_string(), page: page.to_string(), title: info.title.clone() };
        let key = (plugin.to_string(), page.to_string());
        if !self.plugins.sessions.contains_key(&key) {
            let browse_from = self.pane().read(cx).dir_path();
            self.plugins.sessions.insert(key.clone(), PageSession { browse_from, ..Default::default() });
            self.call_plugin_page(key, None, window, cx);
        }
        self.open_location(location, window, cx);
    }

    /// Re-renders a page (e.g. a refresh button, or after a root change).
    pub fn refresh_plugin_page(&mut self, plugin: &str, page: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.call_plugin_page((plugin.to_string(), page.to_string()), None, window, cx);
    }

    /// Sends a UI event to a page.
    pub fn plugin_page_event(&mut self, plugin: &str, page: &str, event: String, window: &mut Window, cx: &mut Context<Self>) {
        self.call_plugin_page((plugin.to_string(), page.to_string()), Some(event), window, cx);
    }

    fn call_plugin_page(&mut self, key: (String, String), event: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.plugins.host.clone() else { return };
        let session = self.plugins.sessions.entry(key.clone()).or_default();
        if session.busy {
            return;
        }
        session.busy = true;
        let req = PageRequest { page_id: key.1.clone(), root: session.root.clone(), state: session.state.clone() };
        let pid = key.0.clone();
        let task = cx.background_spawn(async move {
            let mut host = host.lock().map_err(|_| anyhow::anyhow!("plugin host poisoned"))?;
            match &event {
                Some(ev) => host.page_event(&pid, &req, ev),
                None => host.render_page(&pid, &req),
            }
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let Some(session) = this.plugins.sessions.get_mut(&key) else { return };
                session.busy = false;
                match result {
                    Ok(update) => {
                        // A picker-only update need not regenerate the current
                        // view. Keep it visible if the user cancels the picker.
                        if !update.view.nodes.is_empty() || !update.actions.iter().any(|a| matches!(a, Action::ChooseRoot)) {
                            session.view = Some(update.view);
                        }
                        session.state = update.state;
                        session.error = None;
                        this.apply_page_actions(&key, update.actions, window, cx);
                    }
                    Err(e) => session.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Handles page-only actions and forwards the rest.
    fn apply_page_actions(&mut self, key: &(String, String), actions: Vec<Action>, window: &mut Window, cx: &mut Context<Self>) {
        let mut rest = Vec::new();
        for action in actions {
            match action {
                Action::SetRoot(dir) => self.set_page_root(key.clone(), dir, window, cx),
                Action::ChooseRoot => self.choose_plugin_root(&key.0, &key.1, window, cx),
                other => rest.push(other),
            }
        }
        self.apply_plugin_actions(rest, window, cx);
    }

    /// Reuse Rile's explorer rather than opening a separate platform picker.
    pub fn choose_plugin_root(&mut self, plugin: &str, page: &str, window: &mut Window, cx: &mut Context<Self>) {
        let key = (plugin.to_string(), page.to_string());
        let start = self.plugins.sessions.get(&key).and_then(|s| s.root.clone().or_else(|| s.browse_from.clone()));
        self.start_root_picker(Some(key), start, window, cx);
    }

    /// Lets the user browse to the folder the Space Analyzer should analyze.
    pub fn choose_space_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start = self.space.last_focus.clone();
        self.start_root_picker(None, start, window, cx);
    }

    fn start_root_picker(&mut self, key: Option<(String, String)>, start: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.pane().read(cx).location.clone();
        let start = start.or_else(|| self.pane().read(cx).dir_path()).or_else(dirs::home_dir).filter(|p| p.is_dir());
        self.plugins.root_picker = Some(RootPicker { key, pane: self.pane().entity_id(), previous });
        self.open_location(start.map(Location::Dir).unwrap_or(Location::Home), window, cx);
    }

    pub fn plugin_root_picker_active(&self) -> bool {
        self.plugins.root_picker.as_ref().is_some_and(|p| p.pane == self.pane().entity_id())
    }

    /// A single selected directory wins; otherwise use the folder being viewed.
    /// Remote and virtual locations are not valid WASM filesystem scan roots.
    pub fn plugin_root_picker_path(&self, cx: &App) -> Option<PathBuf> {
        if !self.plugin_root_picker_active() { return None; }
        let pane = self.pane().read(cx);
        let Location::Dir(dir) = &pane.location else { return None };
        let entries = pane.selected_entries();
        Some(if let [entry] = entries.as_slice() && entry.is_dir { entry.path.clone() } else { dir.clone() })
    }

    pub fn confirm_plugin_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.plugin_root_picker_path(cx).filter(|p| p.is_dir()) else { return };
        let Some(picker) = self.plugins.root_picker.take() else { return };
        let Some(key) = picker.key else {
            self.analyze_folder(dir, window, cx);
            return;
        };
        self.set_page_root(key.clone(), dir, window, cx);
        self.open_plugin_page(&key.0, &key.1, window, cx);
    }

    pub fn cancel_plugin_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.plugin_root_picker_active() { return; }
        if let Some(picker) = self.plugins.root_picker.take() { self.open_location(picker.previous, window, cx); }
    }

    fn set_page_root(&mut self, key: (String, String), dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !dir.is_dir() {
            window.push_notification(Notification::error(format!("Not a folder: {}", dir.display())), cx);
            return;
        }
        let session = self.plugins.sessions.entry(key.clone()).or_default();
        session.browse_from = Some(dir.clone());
        session.root = Some(dir);
        session.bubble_zoom = 0;
        self.call_plugin_page(key, None, window, cx);
    }

    fn apply_plugin_actions(&mut self, actions: Vec<Action>, window: &mut Window, cx: &mut Context<Self>) {
        for action in actions {
            match action {
                Action::Open(p) => cx.open_with_system(&p),
                Action::Navigate(p) => self.open_location(Location::Dir(p), window, cx),
                Action::Refresh => self.pane().update(cx, |p, cx| p.reload(cx)),
                Action::CopyText(t) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(t));
                    window.push_notification(Notification::success("Copied to clipboard"), cx);
                }
                Action::Notify(msg) => window.push_notification(Notification::info(msg), cx),
                Action::ChooseRoot | Action::SetRoot(_) => {}
            }
        }
    }
}
