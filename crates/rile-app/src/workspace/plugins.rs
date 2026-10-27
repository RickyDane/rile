//! WASM plugin integration: loading, context-menu commands, actions.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui_kit::component::{WindowExt, notification::Notification};
use gpui_kit::{AppContext, ClipboardItem, Context, SharedString, Task, Window};
use rile_core::history::Location;
use rile_plugins::{Action, Context as PluginContext, PluginHost, PluginInfo};

use super::Workspace;

#[derive(Default)]
pub struct PluginState {
    pub host: Option<Arc<Mutex<PluginHost>>>,
    pub infos: Vec<PluginInfo>,
    pub errors: Vec<SharedString>,
    pub loading: bool,
    _task: Option<Task<()>>,
}

impl Workspace {
    /// Compiles plugins from the plugin folder on a background thread.
    pub fn load_plugins(&mut self, cx: &mut Context<Self>) {
        self.plugins.host = None;
        self.plugins.infos.clear();
        self.plugins.errors.clear();
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
            anyhow::Ok((host, errors))
        });
        self.plugins._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.plugins.loading = false;
                match result {
                    Ok((host, errors)) => {
                        this.plugins.infos = host.plugins();
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
        }
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
            }
        }
    }
}
