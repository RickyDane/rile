//! WASM plugin integration: loading, context-menu commands, actions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui_kit::component::{WindowExt, notification::Notification};
use gpui_kit::{AppContext, ClipboardItem, Context, SharedString, Task, Window};
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
    _task: Option<Task<()>>,
}

/// The host-side state of an open plugin page.
#[derive(Default)]
pub struct PageSession {
    pub root: Option<PathBuf>,
    /// Opaque plugin state, handed back on every call.
    pub state: String,
    pub view: Option<PageView>,
    pub error: Option<SharedString>,
    /// A call is in flight; further events are dropped until it returns.
    pub busy: bool,
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
            let pages = host.pages();
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
            self.plugins.pages = h.pages();
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

    /// Opens a plugin page in the active pane. A new session starts at the
    /// explorer's current directory.
    pub fn open_plugin_page(&mut self, plugin: &str, page: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, info)) = self.plugins.pages.iter().find(|(p, pg)| p == plugin && pg.id == page) else { return };
        let location = Location::PluginPage { plugin: plugin.to_string(), page: page.to_string(), title: info.title.clone() };
        let key = (plugin.to_string(), page.to_string());
        if !self.plugins.sessions.contains_key(&key) {
            let root = self.pane().read(cx).dir_path().filter(|p| p.is_dir());
            self.plugins.sessions.insert(key.clone(), PageSession { root, ..Default::default() });
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
                        session.view = Some(update.view);
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
                Action::ChooseRoot => {
                    let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some("Choose".into()),
                    });
                    let key = key.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        if let Ok(Ok(Some(paths))) = rx.await
                            && let Some(dir) = paths.into_iter().next()
                        {
                            this.update_in(cx, |this, window, cx| this.set_page_root(key, dir, window, cx)).ok();
                        }
                    })
                    .detach();
                }
                other => rest.push(other),
            }
        }
        self.apply_plugin_actions(rest, window, cx);
    }

    fn set_page_root(&mut self, key: (String, String), dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !dir.is_dir() {
            window.push_notification(Notification::error(format!("Not a folder: {}", dir.display())), cx);
            return;
        }
        self.plugins.sessions.entry(key.clone()).or_default().root = Some(dir);
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
