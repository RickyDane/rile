//! App updater UI: background check, release notes, download + install.

use gpui_kit::component::{
    WindowExt, button::{Button, ButtonVariants}, notification::Notification, v_flex,
};
use gpui_kit::{AppContext, Context, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, Task, Window, div, px};
use rile_services::updater::{self, Release, UpdateConfig};

use super::Workspace;
use crate::actions::*;

#[derive(Default)]
pub struct UpdateState {
    pub checking: bool,
    pub available: Option<Release>,
    pub installed_pending_restart: bool,
    pub last_error: Option<String>,
    _task: Option<Task<()>>,
}

impl Workspace {
    fn update_config(&self) -> UpdateConfig {
        UpdateConfig {
            repo: self.settings.update_repo.clone(),
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            allow_prerelease: self.settings.update_prereleases,
        }
    }

    pub(crate) fn check_updates_action(&mut self, _: &CheckForUpdates, window: &mut Window, cx: &mut Context<Self>) {
        self.check_for_updates(true, window, cx);
    }

    /// Checks GitHub releases in the background. `interactive` reports
    /// "up to date" and errors; silent startup checks only surface updates.
    pub fn check_for_updates(&mut self, interactive: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.updates.checking || self.settings.update_repo.trim().is_empty() {
            return;
        }
        self.updates.checking = true;
        let cfg = self.update_config();
        let task = cx.background_spawn(async move { updater::check(&cfg) });
        self.updates._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.updates.checking = false;
                match result {
                    Ok(Some(release)) => {
                        let version = release.version.clone();
                        this.updates.available = Some(release);
                        if interactive {
                            this.open_update_dialog(window, cx);
                        } else {
                            window.push_notification(
                                Notification::info(format!("Rile {version} is available — click the update badge in the toolbar.")),
                                cx,
                            );
                        }
                    }
                    Ok(None) => {
                        this.updates.available = None;
                        if interactive {
                            window.push_notification(Notification::success("Rile is up to date"), cx);
                        }
                    }
                    Err(e) => {
                        this.updates.last_error = Some(format!("{e:#}"));
                        if interactive {
                            window.push_notification(Notification::error(format!("Update check failed: {e:#}")), cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn open_update_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(release) = self.updates.available.clone() else { return };
        let ws = cx.entity().downgrade();
        let restart_pending = self.updates.installed_pending_restart;
        window.open_dialog(cx, move |dialog, _, cx| {
            use crate::views::kit;
            let ch = crate::theme::chrome(cx);
            let ws = ws.clone();
            let release = release.clone();
            let can_install = release.asset.is_some();
            let notes = if release.notes.trim().is_empty() { "No release notes.".to_string() } else { release.notes.clone() };
            kit::dialog(
                dialog,
                gpui_kit::assets::IconName::Download,
                format!("Rile {} is available", release.version),
                Some(format!("You're on {}", env!("CARGO_PKG_VERSION")).into()),
                580.,
                cx,
            )
            .child(
                v_flex()
                    .gap(px(14.))
                    .child(kit::caption("WHAT'S NEW", cx))
                    .child(
                        kit::card(cx).child(
                            div()
                                .id("release-notes")
                                .max_h(px(300.))
                                .overflow_y_scroll()
                                .p(px(14.))
                                .text_size(px(12.))
                                .line_height(px(19.))
                                .text_color(ch.text)
                                .child(notes),
                        ),
                    )
                    .children((!can_install).then(|| {
                        kit::notice(format!("No download for this platform ({}).", updater::asset_name_for_platform()), false, cx)
                    }))
                    .child(
                        kit::footer()
                            .child(Button::new("upd-web").cursor_pointer().ghost().label("View Release").on_click({
                                let url = release.url.clone();
                                move |_, _, cx| cx.open_url(&url)
                            }))
                            .child(if restart_pending {
                                Button::new("upd-restart").cursor_pointer().primary().label("Restart Now").on_click(|_, _, cx| cx.restart())
                            } else {
                                Button::new("upd-install").cursor_pointer().primary().label("Download & Install").disabled_if(!can_install).on_click(
                                    move |_, window, cx| {
                                        window.close_dialog(cx);
                                        ws.update(cx, |ws, cx| ws.install_update(window, cx)).ok();
                                    },
                                )
                            }),
                    ),
            )
        });
    }

    fn install_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(asset) = self.updates.available.as_ref().and_then(|r| r.asset.clone()) else { return };
        let label = format!("Downloading {}", asset.name);
        let message = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let msg_out = message.clone();
        self.start_op_then(
            label,
            window,
            cx,
            move |cancel, report| {
                let dir = std::env::temp_dir().join("rile-update");
                std::fs::create_dir_all(&dir)?;
                let file = updater::download(&asset, &dir, cancel, |done, total| {
                    report(rile_core::ops::Progress { bytes_done: done, bytes_total: total, items_done: 0, items_total: 1, ..Default::default() })
                })?;
                let outcome = updater::install(&file)?;
                *msg_out.lock().unwrap() = outcome.message;
                Ok(Vec::new())
            },
            move |this, window, cx| {
                this.updates.installed_pending_restart = true;
                let msg = message.lock().unwrap().clone();
                window.push_notification(Notification::success(format!("{msg} Restart Rile to finish updating.")), cx);
                this.open_update_dialog(window, cx);
            },
        );
    }
}

trait DisabledIf {
    fn disabled_if(self, disabled: bool) -> Self;
}

impl DisabledIf for Button {
    fn disabled_if(self, disabled: bool) -> Self {
        use gpui_kit::component::Disableable;
        self.disabled(disabled)
    }
}
