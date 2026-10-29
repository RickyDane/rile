use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt, h_flex,
    button::{Button, ButtonVariant, ButtonVariants},
    dialog::Dialog,
    input::{Input, InputEvent, InputState},
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::FluentBuilder, px,
};
use rile_core::settings::{Accent, AppIcon, Density, FileCategory, IconSet, Settings, StartLocation, ThemeMode, ViewKind};

use crate::views::kit::PointerUnless;
use crate::theme::accent_color;
use crate::views::kit;
use gpui_kit::assets::IconName;
use crate::workspace::Workspace;

/// Dialog with a single text input and Cancel/confirm buttons;
/// `submit` returns whether the dialog may close.
pub fn input_dialog(
    dialog: Dialog,
    title: &str,
    input: &Entity<InputState>,
    submit: impl Fn(&mut Window, &mut App) -> bool + 'static,
    cx: &App,
) -> Dialog {
    kit::dialog(dialog, IconName::Pencil, SharedString::from(title.to_string()), Some("Press Return to confirm".into()), 440., cx).child(
        v_flex().gap(px(14.)).child(Input::new(input)).child(
            kit::footer()
                .child(Button::new("cancel").cursor_pointer().ghost().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                .child(Button::new("ok").cursor_pointer().primary().label(title.to_string()).on_click(move |_, window, cx| {
                    if submit(window, cx) {
                        window.close_dialog(cx);
                    }
                })),
        ),
    )
}

/// Confirmation alert; `on_confirm` runs when the user accepts.
pub fn confirm(
    message: String,
    ok_label: &'static str,
    destructive: bool,
    window: &mut Window,
    cx: &mut App,
    on_confirm: impl Fn(&mut Window, &mut App) + 'static,
) {
    let on_confirm = std::rc::Rc::new(on_confirm);
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let on_confirm = on_confirm.clone();
        let on_click_confirm = on_confirm.clone();
        let ch = crate::theme::chrome(cx);
        let alert = kit::surface(alert, cx)
            .centered()
            .entrance(gpui_kit::component::dialog::DialogEntrance::Fade)
            .w(px(420.))
            .icon(crate::icons::icon(if destructive { IconName::TriangleAlert } else { IconName::Info }).size(px(20.)).text_color(if destructive {
                gpui_kit::Hsla::from(gpui_kit::rgba(0xf07178ff))
            } else {
                ch.accent
            }))
            .title(SharedString::from(message.clone()))
            .ok_text(ok_label)
            .show_cancel(true)
            // Default alert buttons use an arrow cursor; style both actions
            // explicitly while retaining on_ok for keyboard confirmation.
            .footer(
                h_flex()
                    .justify_center()
                    .gap_2()
                    .child(Button::new("cancel").debug_selector(|| "confirm-cancel".into()).cursor_pointer().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        Button::new("ok").cursor_pointer()
                            .debug_selector(|| "confirm-ok".into())
                            .cursor_pointer()
                            .label(ok_label)
                            .with_variant(if destructive { ButtonVariant::Danger } else { ButtonVariant::Primary })
                            .on_click(move |_, window, cx| {
                                on_click_confirm(window, cx);
                                window.close_dialog(cx);
                            }),
                    ),
            )
            .on_ok(move |_, window, cx| {
                on_confirm(window, cx);
                true
            });
        if destructive { alert.ok_variant(ButtonVariant::Danger) } else { alert }
    });
}

type ChoiceAction = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// One button of a [`choose`] dialog.
pub struct Choice {
    pub label: &'static str,
    pub variant: ButtonVariant,
    pub action: ChoiceAction,
}

impl Choice {
    pub fn new(label: &'static str, variant: ButtonVariant, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self { label, variant, action: std::rc::Rc::new(action) }
    }
}

/// A confirmation with an explanation and several ways to proceed (plus
/// Cancel); the dialog closes after any choice.
pub fn choose(title: String, detail: String, destructive: bool, choices: Vec<Choice>, window: &mut Window, cx: &mut App) {
    let choices: Vec<(SharedString, ButtonVariant, ChoiceAction)> =
        choices.into_iter().map(|c| (SharedString::from(c.label), c.variant, c.action)).collect();
    window.open_dialog(cx, move |dialog, _, cx| {
        let icon = if destructive { IconName::TriangleAlert } else { IconName::Info };
        kit::dialog(dialog, icon, title.clone(), None, 480., cx).child(
            v_flex()
                .gap(px(16.))
                .child(div().text_size(px(13.)).line_height(px(19.)).text_color(crate::theme::chrome(cx).text).child(detail.clone()))
                .child(
                    kit::footer()
                        .flex_wrap()
                        .child(Button::new("choose-cancel").debug_selector(|| "choose-cancel".into()).cursor_pointer().ghost().label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)))
                        .children(choices.iter().enumerate().map(|(i, (label, variant, action))| {
                            let action = action.clone();
                            Button::new(("choose", i)).debug_selector(move || format!("choose-{i}")).cursor_pointer()
                                .label(label.clone()).with_variant(*variant)
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    action(window, cx);
                                })
                        })),
                ),
        )
    });
}

/// Called from inside a workspace update, so the current settings are
/// passed in rather than read from `ws` (which would panic).
pub fn open_settings(ws: Entity<Workspace>, settings: Settings, window: &mut Window, cx: &mut App) {
    open_settings_at(ws, settings, Tab::General, None, window, cx);
}

/// Opens Settings → Backups with the job `job` (or the first one) shown.
pub fn open_backup_settings(ws: Entity<Workspace>, settings: Settings, job: Option<String>, window: &mut Window, cx: &mut App) {
    open_settings_at(ws, settings, Tab::Backups, job, window, cx);
}

/// Opens Settings on the Nearby tab.
pub fn open_nearby_settings(ws: Entity<Workspace>, settings: Settings, window: &mut Window, cx: &mut App) {
    open_settings_at(ws, settings, Tab::Nearby, None, window, cx);
}

fn open_settings_at(ws: Entity<Workspace>, settings: Settings, tab: Tab, job: Option<String>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| {
        let mut view = SettingsView::new(ws, settings, job, window, cx);
        view.tab = tab;
        view
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        kit::dialog(dialog, IconName::Settings, "Settings", Some(format!("Rile {}", env!("CARGO_PKG_VERSION")).into()), 760., cx)
            .child(view.clone())
    });
}

pub fn open_shortcuts(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, cx| {
        let ch = crate::theme::chrome(cx);
        let mac = cfg!(target_os = "macos");
        // Several keys, or the macOS / other-platform variant of one.
        let keys = |list: &[&str]| list.iter().map(|k| crate::views::chrome::shortcut(k)).collect::<Vec<_>>().join("  ");
        let pick = |m: &[&str], other: &[&str]| keys(if mac { m } else { other });
        let groups: Vec<(&str, Vec<(String, &str)>)> = vec![
            (
                "Files",
                vec![
                    (keys(&["mod-c", "mod-x", "mod-v"]), "Copy, cut, paste"),
                    (keys(&["mod-d"]), "Duplicate"),
                    ("F2".into(), "Rename"),
                    (keys(&["shift-mod-n"]), "New folder"),
                    (pick(&["alt-mod-n"], &["shift-mod-e"]), "New file"),
                    (pick(&["mod-backspace"], &["delete"]), if mac { "Move to Trash" } else { "Move to Recycle Bin" }),
                    (pick(&["shift-mod-backspace"], &["shift-delete"]), "Delete permanently"),
                    (keys(&["mod-i"]), "Properties"),
                    ("Space".into(), if mac { "Quick Look" } else { "Preview" }),
                    ("Type letters".into(), "Jump to item"),
                    ((if mac { "⌥ while dropping" } else { "Alt while dropping" }).into(), "Copy instead of move"),
                ],
            ),
            (
                "Navigation",
                vec![
                    (pick(&["mod-[", "mod-]"], &["alt-left", "alt-right"]), "Back, forward"),
                    ("Backspace".into(), "Back in focused pane"),
                    (pick(&["mod-up"], &["alt-up"]), "Enclosing folder"),
                    (keys(&["shift-mod-h"]), "Home"),
                    (pick(&["mod-l"], &["mod-l", "alt-d"]), "Go to path"),
                    (keys(&["mod-f", "shift-mod-f"]), "Filter, advanced search"),
                    (keys(&["mod-t", "mod-w"]), "New tab, close tab"),
                    (format!("{}  {}", if mac { "⌃Tab" } else { "Ctrl+Tab" }, keys(&["shift-mod-]", "shift-mod-["])), "Next, previous tab"),
                    (keys(&["shift-mod-o"]), "Open folder in new tab"),
                    (pick(&["ctrl-mod-t"], &["shift-mod-b"]), "Add to Favorites"),
                ],
            ),
            (
                "View",
                vec![
                    (keys(&["mod-1", "mod-2", "mod-3"]), "List, icons, columns"),
                    (keys(&["shift-mod-d"]), "Dual pane"),
                    (keys(&["mod-f1", "mod-f2"]), "Select left, right drive"),
                    ("Tab".into(), "Switch pane"),
                    ("F5  F6".into(), "Copy, move to other pane"),
                    (keys(&["shift-mod-p"]), "Inspector"),
                    (keys(&["shift-mod-s"]), "Sidebar"),
                    (keys(&["shift-mod-."]), "Hidden files"),
                    (keys(&["shift-mod-t"]), "Light, dark"),
                    (keys(&["mod-k"]), "Ask AI"),
                    (keys(&["mod-,"]), "Settings"),
                ],
            ),
        ];
        let column = |title: &'static str, items: Vec<(String, &'static str)>, cx: &App| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(8.))
                .child(kit::caption(title, cx))
                .child(kit::card(cx).py(px(4.)).children(items.into_iter().map(|(k, v)| {
                    h_flex()
                        .px(px(12.))
                        .py(px(6.))
                        .gap(px(10.))
                        .justify_between()
                        .child(div().min_w_0().truncate().text_size(px(12.)).text_color(ch.text).child(v.to_string()))
                        .child(kit::kbd(k, cx))
                })))
        };
        let mut groups = groups.into_iter();
        let (files, nav, view) = (groups.next().unwrap(), groups.next().unwrap(), groups.next().unwrap());
        kit::dialog(dialog, IconName::Keyboard, "Keyboard Shortcuts", Some(format!("{} shows this list anytime", crate::views::chrome::shortcut("mod-/")).into()), 860., cx).child(
            h_flex()
                .gap(px(14.))
                .items_start()
                .child(column(files.0, files.1, cx))
                .child(column(nav.0, nav.1, cx))
                .child(column(view.0, view.1, cx)),
        )
    });
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    General,
    Appearance,
    Search,
    Programs,
    Plugins,
    Backups,
    Nearby,
    Ai,
    About,
}

/// Settings dialog content with tabs. Re-renders when the workspace changes.
pub struct SettingsView {
    ws: Entity<Workspace>,
    tab: Tab,
    repo_input: Entity<InputState>,
    max_results_input: Entity<InputState>,
    device_name_input: Entity<InputState>,
    glass_hue: Entity<SliderState>,
    glass_translucency: Entity<SliderState>,
    file_manager: rile_services::file_manager::Status,
    file_manager_busy: bool,
    file_manager_message: Option<String>,
    backups: Entity<crate::views::backup_settings::BackupSettingsView>,
    ai: Entity<crate::views::ai_settings::AiSettings>,
    _subs: Vec<Subscription>,
}

impl SettingsView {
    fn new(ws: Entity<Workspace>, s: Settings, backup_job: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backups = cx.new(|cx| crate::views::backup_settings::BackupSettingsView::new(ws.clone(), backup_job, window, cx));
        // A download or runtime install started earlier keeps running after
        // Settings closes; show its view again rather than a fresh one.
        let ai = crate::views::ai_settings::AiSettings::running(&ws, cx).unwrap_or_else(|| {
            cx.new(|cx| crate::views::ai_settings::AiSettings::new(ws.clone(), s.ai_llama_server.clone(), s.ai_mlx_server.clone(), s.ai_model_path.clone(), window, cx))
        });
        let repo_input = cx.new(|cx| InputState::new(window, cx).default_value(s.update_repo.clone()).placeholder("owner/repo"));
        let max_results_input = cx.new(|cx| InputState::new(window, cx).default_value(s.search_max_results.to_string()));
        // Built inside a workspace update: the workspace can't be read here.
        let computer_name = crate::workspace::nearby::cached_facts().map(|f| f.name).unwrap_or_default();
        let device_name_input = cx.new(|cx| {
            InputState::new(window, cx).default_value(s.nearby.device_name.clone()).placeholder(if computer_name.is_empty() { "Computer name".to_string() } else { computer_name })
        });
        let glass_hue = cx.new(|_| SliderState::new().min(0.).max(360.).step(1.).default_value(s.glass_hue.min(360) as f32));
        let glass_translucency = cx.new(|_| SliderState::new().min(0.).max(100.).step(1.).default_value(s.glass_translucency.min(100) as f32));
        let mut subs = vec![
            cx.observe(&ws, |_, _, cx| cx.notify()),
            cx.subscribe_in(&repo_input, window, |this: &mut Self, input, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let v = input.read(cx).value().trim().to_string();
                    this.update(window, cx, move |s| s.update_repo = v);
                }
            }),
            cx.subscribe_in(&device_name_input, window, |this: &mut Self, input, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let v = input.read(cx).value().trim().to_string();
                    this.update(window, cx, move |s| s.nearby.device_name = v);
                }
            }),
            cx.subscribe_in(&max_results_input, window, |this: &mut Self, input, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. })
                    && let Ok(n) = input.read(cx).value().trim().parse::<usize>()
                {
                    this.update(window, cx, move |s| s.search_max_results = n);
                }
            }),
        ];
        for (slider, hue) in [(&glass_hue, true), (&glass_translucency, false)] {
            subs.push(cx.subscribe_in(slider, window, move |this: &mut Self, _, ev: &SliderEvent, window, cx| {
                let (SliderEvent::Change(v) | SliderEvent::Release(v)) = ev;
                this.ws.update(cx, |ws, cx| {
                    if hue {
                        ws.settings.glass_hue = v.start().round().clamp(0., 360.) as u16;
                    } else {
                        ws.settings.glass_translucency = v.start().round().clamp(0., 100.) as u8;
                    }
                    if matches!(ev, SliderEvent::Release(_)) {
                        ws.settings_changed(window, cx);
                    } else {
                        crate::theme::apply(&ws.settings, window, cx);
                        for pane in ws.all_panes() {
                            pane.update(cx, |_, cx| cx.notify());
                        }
                        cx.notify();
                    }
                });
            }));
        }
        Self {
            ws, tab: Tab::General, repo_input, max_results_input, device_name_input, glass_hue, glass_translucency, _subs: subs,
            file_manager: rile_services::file_manager::status(), file_manager_busy: false, file_manager_message: None, backups, ai,
        }
    }

    fn set_file_manager(&mut self, rile: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_manager_busy { return; }
        self.file_manager_busy = true;
        self.file_manager_message = Some(if cfg!(target_os = "macos") { "Waiting for macOS approval…" } else { "Updating…" }.into());
        cx.notify();
        let (tx, rx) = smol::channel::bounded(1);
        rile_services::file_manager::set_default(rile, move |result| { tx.try_send(result).ok(); });
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.recv().await;
            this.update_in(cx, |this, _, cx| {
                this.file_manager_busy = false;
                this.file_manager = rile_services::file_manager::status();
                this.file_manager_message = Some(match result {
                    Ok(Ok(rile_services::file_manager::Outcome::Updated)) if !cfg!(target_os = "macos") => "Updated. Explorer windows that are already open keep their current behavior until reopened.".into(),
                    Ok(Ok(rile_services::file_manager::Outcome::Updated)) => "Updated. Restart other apps or log out if they still use the previous viewer.".into(),
                    Ok(Ok(rile_services::file_manager::Outcome::ViewerOnly)) => "File viewer preference updated for compatible Reveal in Finder actions. This macOS version rejected the folder association; folder opening is unchanged. Restart other apps or log out to apply the viewer preference.".into(),
                    Ok(Err(error)) => format!("{error:#}"),
                    Err(_) => "macOS didn't complete the change. Please try again.".into(),
                });
                cx.notify();
            }).ok();
        }).detach();
    }

    fn file_manager_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let mac = cfg!(target_os = "macos");
        let message = self.file_manager_message.clone().unwrap_or_else(|| {
            if !mac {
                if self.file_manager.is_default {
                    "Opening a folder in Explorer opens it in Rile. Win+E and the desktop still use Explorer.".into()
                } else {
                    "\"Open in Rile\" is in the folder context menu; Explorer stays the default.".into()
                }
            } else if !self.file_manager.can_set_rile {
                "Launch the installed Rile.app to make it the default.".into()
            } else if self.file_manager.is_default {
                "Rile is the default file manager.".into()
            } else if self.file_manager.is_viewer {
                "Rile is the preferred viewer for compatible Reveal in Finder actions, but not the default folder handler.".into()
            } else {
                "Finder's desktop and system Open/Save dialogs stay unchanged.".into()
            }
        });
        v_flex().gap(px(8.))
            .child(h_flex().gap(px(8.))
                .child(Button::new("s-default-manager").cursor_pointer().small().outline().label("Make Rile Default")
                    .pointer_unless(self.file_manager_busy || !self.file_manager.can_set_rile || self.file_manager.is_default)
                    .on_click(cx.listener(|this, _, window, cx| this.set_file_manager(true, window, cx))))
                .child(Button::new("s-restore-finder").cursor_pointer().small().outline().label(if mac { "Restore Finder" } else { "Restore Explorer" })
                    .pointer_unless(self.file_manager_busy)
                    .on_click(cx.listener(|this, _, window, cx| this.set_file_manager(false, window, cx)))))
            .child(div().max_w(px(340.)).text_size(px(11.)).text_color(cx.theme().muted_foreground).child(message))
            .into_any_element()
    }

    /// Mutates settings, then re-applies appearance and saves.
    fn update(&self, window: &mut Window, cx: &mut App, f: impl FnOnce(&mut Settings)) {
        self.ws.update(cx, |ws, cx| {
            f(&mut ws.settings);
            ws.settings_changed(window, cx);
        });
    }

    /// Settings → Nearby: discovery, this device's name, shared folders,
    /// where received files go, and paired devices.
    fn render_nearby(&self, s: &Settings, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let ws = self.ws.read(cx);
        let id = ws.nearby.store.as_ref().map(|st| rile_remote::peer::short_id(st.id())).unwrap_or_else(|| "—".into());
        let port = ws.nearby.port();
        let error = ws.nearby.error.clone();
        let trusted = ws.nearby.trusted.clone();
        let online: Vec<String> = ws.nearby.online.iter().map(|p| p.id.clone()).collect();
        let r = |label: &'static str, hint: String, control: AnyElement| (SharedString::from(label), SharedString::from(hint), control);
        let section = |title: &'static str, items: Vec<(SharedString, SharedString, AnyElement)>, cx: &App| {
            v_flex().gap(px(8.)).child(kit::caption(title, cx)).child(kit::rows(items, cx))
        };
        let status = match (&error, port, s.nearby.enabled) {
            (_, _, false) => "Off".to_string(),
            (Some(e), _, _) => e.clone(),
            (None, Some(port), _) => format!("Visible on your network · port {port} · id {id}"),
            (None, None, _) => "Starting…".to_string(),
        };

        let shares: Vec<(SharedString, SharedString, AnyElement)> = s
            .nearby
            .shares
            .iter()
            .enumerate()
            .map(|(ix, share)| {
                let ws = self.ws.clone();
                let path = share.path.clone();
                let missing = !share.path.is_dir();
                (
                    SharedString::from(share.name.clone()),
                    SharedString::from(format!("{}{}", share.path.display(), if missing { " · missing" } else { " · read-only" })),
                    kit::button(("share-remove", ix), "Stop Sharing", kit::Tone::Secondary, true, cx)
                        .on_click(move |_, window, cx| ws.update(cx, |ws, cx| ws.unshare_folder(&path, window, cx)))
                        .into_any_element(),
                )
            })
            .collect();
        let ws_share = self.ws.clone();
        let add_share = kit::button("share-add", "Share a Folder…", kit::Tone::Secondary, true, cx).on_click(move |_, window, cx| {
            let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: false, directories: true, multiple: true, prompt: Some("Share".into()) });
            let ws = ws_share.clone();
            window
                .spawn(cx, async move |cx| {
                    let Ok(Ok(Some(paths))) = picked.await else { return };
                    cx.update(|window, cx| {
                        ws.update(cx, |ws, cx| {
                            for p in paths {
                                ws.share_folder(p, window, cx);
                            }
                        })
                    })
                    .ok();
                })
                .detach();
        });

        let ws_inbox = self.ws.clone();
        let inbox = s.nearby.inbox_dir();
        let choose_inbox = kit::button("inbox-choose", "Choose…", kit::Tone::Secondary, true, cx).on_click(move |_, window, cx| {
            let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Choose".into()) });
            let ws = ws_inbox.clone();
            window
                .spawn(cx, async move |cx| {
                    let Ok(Ok(Some(mut paths))) = picked.await else { return };
                    let Some(dir) = paths.pop() else { return };
                    cx.update(|window, cx| {
                        ws.update(cx, |ws, cx| {
                            ws.settings.nearby.inbox = Some(dir);
                            ws.settings_changed(window, cx);
                        })
                    })
                    .ok();
                })
                .detach();
        });

        let paired: Vec<(SharedString, SharedString, AnyElement)> = trusted
            .iter()
            .enumerate()
            .map(|(ix, t)| {
                let ws = self.ws.clone();
                let id = t.id.clone();
                let state = if online.contains(&t.id) { "Online" } else { "Offline" };
                let model = if t.model.is_empty() { String::new() } else { format!("{} · ", t.model) };
                (
                    SharedString::from(t.name.clone()),
                    SharedString::from(format!("{model}{state} · {}", rile_remote::peer::short_id(&t.id))),
                    kit::button(("unpair", ix), "Unpair…", kit::Tone::Secondary, true, cx)
                        .on_click(move |_, window, cx| ws.update(cx, |ws, cx| ws.unpair(&id, window, cx)))
                        .into_any_element(),
                )
            })
            .collect();
        let has_paired = !paired.is_empty();
        let has_shares = !shares.is_empty();

        v_flex()
            .gap(px(16.))
            .child(section(
                "NEARBY DEVICES",
                vec![
                    r("Find nearby devices", status, self.switch("s-nearby", s.nearby.enabled, |s, v| s.nearby.enabled = v)),
                    r(
                        "Device name",
                        "How other devices see this computer".into(),
                        div().w(px(220.)).child(gpui_kit::component::input::Input::new(&self.device_name_input).small()).into_any_element(),
                    ),
                ],
                cx,
            ))
            .child(
                v_flex()
                    .gap(px(8.))
                    .child(h_flex().justify_between().child(kit::caption("SHARED FOLDERS", cx)).child(add_share))
                    .when(has_shares, |d| d.child(kit::rows(shares, cx)))
                    .when(!has_shares, |d| {
                        d.child(kit::hint("Nothing is shared. Paired devices can browse shared folders but never change them.", cx))
                    }),
            )
            .child(section(
                "RECEIVING",
                vec![r("Save received files in", inbox.display().to_string(), choose_inbox.into_any_element())],
                cx,
            ))
            .when(has_paired, |d| d.child(section("PAIRED DEVICES", paired, cx)))
            .child(kit::hint(
                "Devices find each other with Bonjour and talk directly over encrypted connections. Pair from the Nearby section in the sidebar: both screens show the same code.",
                cx,
            ))
            .child(div().text_color(ch.text_faint))
            .into_any_element()
    }

    fn switch(&self, id: &'static str, value: bool, f: fn(&mut Settings, bool)) -> AnyElement {
        let ws = self.ws.clone();
        Switch::new(id).cursor_pointer()
            .checked(value)
            .on_click(move |checked, window, cx| {
                let checked = *checked;
                ws.update(cx, |ws, cx| {
                    f(&mut ws.settings, checked);
                    ws.settings_changed(window, cx);
                });
            })
            .into_any_element()
    }

    fn choice<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        current: T,
        options: &[(T, &'static str)],
        f: fn(&mut Settings, T),
        cx: &App,
    ) -> AnyElement {
        kit::segmented(cx)
            .children(options.iter().enumerate().map(|(i, (v, label))| {
                let v = *v;
                let ws = self.ws.clone();
                kit::segment((id, i), *label, v == current, cx).on_click(move |_, window, cx| {
                    ws.update(cx, |ws, cx| {
                        f(&mut ws.settings, v);
                        ws.settings_changed(window, cx);
                    });
                })
            }))
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ch = crate::theme::chrome(cx);
        let ws = self.ws.read(cx);
        let s = ws.settings.clone();
        let plugin_infos = ws.plugins.infos.clone();
        let plugin_errors = ws.plugins.errors.clone();
        let plugins_loading = ws.plugins.loading;
        let update_checking = ws.updates.checking;
        let update_available = ws.updates.available.as_ref().map(|r| r.version.clone());
        let r = |label: &'static str, hint: &'static str, control: AnyElement| (SharedString::from(label), SharedString::from(hint), control);

        let tabs = [
            (Tab::General, "General", IconName::SlidersHorizontal),
            (Tab::Appearance, "Appearance", IconName::SunMoon),
            (Tab::Search, "Search", IconName::Search),
            (Tab::Programs, "Programs", IconName::AppWindow),
            (Tab::Plugins, "Plugins", IconName::Puzzle),
            (Tab::Backups, "Backups", IconName::DatabaseBackup),
            (Tab::Nearby, "Nearby", IconName::MonitorSmartphone),
            (Tab::Ai, "AI", IconName::Sparkles),
            (Tab::About, "About", IconName::Info),
        ];
        let tab_list = v_flex().gap(px(2.)).children(tabs.into_iter().map(|(t, label, icon)| {
            let active = self.tab == t;
            let color = if active { ch.sidebar_active_icon } else { ch.sidebar_icon };
            let glyph = if t == Tab::Ai {
                crate::icons::apple_intelligence(15., color, cx)
            } else {
                crate::icons::icon(icon).size(px(15.)).text_color(color).into_any_element()
            };
            h_flex()
                .id(label)
                .h(px(32.))
                .px(px(10.))
                .gap(px(10.))
                .rounded(px(7.))
                .text_size(px(13.))
                .cursor_pointer()
                .text_color(if active { ch.text_strong } else { ch.text })
                .when(active, |d| d.bg(ch.sidebar_active))
                .when(!active, |d| d.hover(|d| d.bg(ch.hover_strong)))
                .child(glyph)
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = t;
                    cx.notify();
                }))
        }));
        let ws_check = self.ws.clone();
        let ws_show = self.ws.clone();
        let nav = v_flex()
            .w(px(170.))
            .flex_none()
            .self_stretch()
            .justify_between()
            .child(tab_list)
            .child({
                let has_update = update_available.is_some();
                let (icon, label) = if update_checking {
                    (IconName::RotateCw, "Checking…".to_string())
                } else if let Some(v) = &update_available {
                    (IconName::Download, format!("Update to {v}"))
                } else {
                    (IconName::RotateCw, "Check for Updates".to_string())
                };
                let color = if has_update { ch.accent } else { ch.text_muted };
                h_flex()
                    .id("upd-check")
                    .justify_center()
                    .h(px(28.))
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(7.))
                    .text_size(px(12.))
                    .text_color(color)
                    .cursor_pointer()
                    .hover(|d| d.bg(ch.hover_strong).text_color(if has_update { ch.accent } else { ch.text }))
                    .child(crate::icons::icon(icon).size(px(13.)).text_color(color))
                    .child(label)
                    .on_click(move |_, window, cx| {
                        if has_update {
                            ws_show.update(cx, |ws, cx| ws.open_update_dialog(window, cx));
                        } else if !update_checking {
                            ws_check.update(cx, |ws, cx| ws.check_for_updates(true, window, cx));
                        }
                    })
            });

        let section = |title: &'static str, items: Vec<(SharedString, SharedString, AnyElement)>, cx: &App| {
            v_flex().gap(px(8.)).child(kit::caption(title, cx)).child(kit::rows(items, cx))
        };

        let body: AnyElement = match self.tab {
            Tab::General => v_flex()
                .gap(px(16.))
                .child(section(
                    "BROWSING",
                    vec![
                        r("Show hidden files", "Dotfiles and system-hidden items", self.switch("s-hidden", s.show_hidden, |s, v| s.show_hidden = v)),
                        r(
                            "Default view",
                            "Used for new tabs and panes",
                            self.choice("s-view", s.default_view, &[(ViewKind::Grid, "Icons"), (ViewKind::List, "List"), (ViewKind::Columns, "Columns")], |s, v| s.default_view = v, cx),
                        ),
                        r("Dual pane by default", "Two panes side by side (Tab switches)", self.switch("s-dual", s.dual_pane, |s, v| s.dual_pane = v)),
                        r("Inspector", "Details and preview of the selection on the right", self.switch("s-prev", s.show_preview_panel, |s, v| s.show_preview_panel = v)),
                    ],
                    cx,
                ))
                .child(section(
                    "BEHAVIOR",
                    vec![
                        r(
                            "On launch open",
                            "Where Rile starts",
                            self.choice(
                                "s-start",
                                s.start_location,
                                &[(StartLocation::Home, "Home"), (StartLocation::LastVisited, "Last folder"), (StartLocation::Custom, "Folder…")],
                                |s, v| s.start_location = v,
                                cx,
                            ),
                        ),
                        (
                            SharedString::from("Startup folder"),
                            SharedString::from(match (&s.start_dir, s.start_location) {
                                (Some(d), StartLocation::Custom) => d.display().to_string(),
                                (Some(d), _) => format!("{} (used with \"Folder…\")", d.display()),
                                (None, _) => "Choose any folder to open on launch".to_string(),
                            }),
                            {
                                let ws = self.ws.clone();
                                Button::new("s-start-dir")
                                    .cursor_pointer()
                                    .small()
                                    .outline()
                                    .label("Choose…")
                                    .on_click(move |_, _, cx| {
                                        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                                            files: false,
                                            directories: true,
                                            multiple: false,
                                            prompt: Some("Open on Launch".into()),
                                        });
                                        let ws = ws.clone();
                                        cx.spawn(async move |cx| {
                                            if let Ok(Ok(Some(paths))) = rx.await
                                                && let Some(dir) = paths.into_iter().next()
                                            {
                                                cx.update(|cx| {
                                                    ws.update(cx, |ws, cx| {
                                                        ws.settings.start_dir = Some(dir);
                                                        ws.settings.start_location = StartLocation::Custom;
                                                        ws.settings.save().ok();
                                                        cx.notify();
                                                    })
                                                });
                                            }
                                        })
                                        .detach();
                                    })
                                    .into_any_element()
                            },
                        ),
                        r(if cfg!(target_os = "macos") { "Confirm before moving to Trash" } else { "Confirm before moving to Recycle Bin" }, "Ask before deleting items", self.switch("s-trash", s.confirm_trash, |s, v| s.confirm_trash = v)),
                    ],
                    cx,
                ))
                .when(cfg!(target_os = "macos"), |body| body.child(section(
                    "MACOS INTEGRATION",
                    vec![r("Default file manager", "Open folders and handle compatible Reveal in Finder actions", self.file_manager_controls(cx))],
                    cx,
                )))
                .when(cfg!(windows), |body| body.child(section(
                    "WINDOWS INTEGRATION",
                    vec![r("Default file manager", "Open folders from Explorer, shortcuts and other apps in Rile", self.file_manager_controls(cx))],
                    cx,
                )))
                .child(section(
                    "UPDATES",
                    vec![
                        r("Check on launch", "Look for a new release when Rile starts", self.switch("s-upd", s.update_check_on_start, |s, v| s.update_check_on_start = v)),
                        r("Include pre-releases", "Offer beta versions", self.switch("s-pre", s.update_prereleases, |s, v| s.update_prereleases = v)),
                        r("Release source", "GitHub repository (owner/repo)", div().w(px(200.)).child(Input::new(&self.repo_input).small()).into_any_element()),
                    ],
                    cx,
                ))
                .into_any_element(),
            Tab::Appearance => {
                let swatches = h_flex()
                    .gap(px(6.))
                    .children(Accent::ALL.into_iter().map(|a| {
                        let ws = self.ws.clone();
                        let selected = s.accent == a;
                        div()
                            .id(a.label())
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .border_2()
                            .border_color(if selected { ch.text_strong } else { gpui_kit::transparent_black() })
                            .child(div().size(px(14.)).rounded_full().bg(accent_color(a, theme.is_dark())))
                            .on_click(move |_, window, cx| {
                                ws.update(cx, |ws, cx| {
                                    ws.settings.accent = a;
                                    ws.settings_changed(window, cx);
                                })
                            })
                    }))
                    .into_any_element();
                let preview = [IconName::Folder, IconName::FileText, IconName::Star, IconName::Settings];
                let icon_sets = h_flex()
                    .gap(px(6.))
                    .children(IconSet::ALL.into_iter().map(|set| {
                        let ws = self.ws.clone();
                        let selected = s.icon_set == set;
                        let previous = crate::icons::icon_set();
                        // Draw the preview glyphs in that set.
                        crate::icons::set_icon_set(set);
                        let glyphs = h_flex().gap(px(4.)).children(
                            preview.iter().map(|i| crate::icons::icon(*i).size(px(14.)).text_color(if selected { ch.accent } else { ch.text_muted })),
                        );
                        crate::icons::set_icon_set(previous);
                        v_flex()
                            .id(set.label())
                            .items_center()
                            .gap(px(5.))
                            .px(px(9.))
                            .py(px(7.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .border_1()
                            .border_color(if selected { ch.accent.opacity(0.6) } else { ch.hairline })
                            .when(selected, |d| d.bg(ch.accent_soft))
                            .hover(|d| d.bg(ch.hover_strong))
                            .child(glyphs)
                            .child(div().text_size(px(10.)).text_color(if selected { ch.text_strong } else { ch.text_muted }).child(set.label()))
                            .on_click(move |_, window, cx| {
                                ws.update(cx, |ws, cx| {
                                    ws.settings.icon_set = set;
                                    ws.settings_changed(window, cx);
                                })
                            })
                    }))
                    .into_any_element();
                let app_icons = h_flex()
                    .flex_wrap()
                    .w(px(332.))
                    .gap(px(4.))
                    .children(AppIcon::ALL.into_iter().map(|style| {
                        let ws = self.ws.clone();
                        let selected = s.app_icon == style;
                        v_flex()
                            .id(style.label())
                            .w(px(80.))
                            .items_center()
                            .gap(px(3.))
                            .py(px(6.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .border_1()
                            .border_color(if selected { ch.accent.opacity(0.6) } else { gpui_kit::transparent_black() })
                            .when(selected, |d| d.bg(ch.accent_soft))
                            .hover(|d| d.bg(ch.hover_strong))
                            .when_some(crate::app_icon::png(style, s.accent), |d, image| {
                                use gpui_kit::StyledImage;
                                d.child(gpui_kit::img(image).size(px(44.)).object_fit(gpui_kit::ObjectFit::Contain))
                            })
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(10.))
                                    .text_center()
                                    .text_color(if selected { ch.text_strong } else { ch.text_muted })
                                    .child(if style == AppIcon::DrawerHotRod { "Drawer\nHot Rod" } else { style.label() }),
                            )
                            .on_click(move |_, window, cx| {
                                ws.update(cx, |ws, cx| {
                                    ws.settings.app_icon = style;
                                    ws.settings_changed(window, cx);
                                })
                            })
                    }))
                    .into_any_element();
                v_flex()
                    .gap(px(16.))
                    .child(section(
                        "THEME",
                        vec![
                            r(
                                "Appearance",
                                "Follow the system or force a mode",
                                self.choice("s-theme", s.theme, &[(ThemeMode::System, "System"), (ThemeMode::Light, "Light"), (ThemeMode::Dark, "Dark")], |s, v| s.theme = v, cx),
                            ),
                            r("Accent color", "Selection, buttons, highlights and sidebar icons", swatches),
                            r(
                                "Glass effect",
                                "Blurred sidebar and toolbar; solid panels and popups for readability",
                                self.switch("s-glass", s.glass, |s, v| s.glass = v),
                            ),
                            r(
                                "Glass background color",
                                "Sidebar and header tint; slide fully left for neutral",
                                h_flex().gap(px(8.)).child(div().size(px(16.)).rounded_full().bg(ch.sidebar_top.opacity(1.)))
                                    .child(div().w(px(130.)).child(Slider::new(&self.glass_hue).disabled(!s.glass).accessibility_label("Glass background color")))
                                    .child(div().w(px(46.)).text_size(px(11.)).child(if s.glass_hue == 0 { "Neutral".to_string() } else { format!("{}°", s.glass_hue.min(360)) }))
                                    .into_any_element(),
                            ),
                            r(
                                "Glass translucency",
                                "0% is solid; 100% reveals the blurred background",
                                h_flex().gap(px(8.))
                                    .child(div().w(px(130.)).child(Slider::new(&self.glass_translucency).disabled(!s.glass).accessibility_label("Glass translucency")))
                                    .child(div().w(px(46.)).text_size(px(11.)).child(format!("{}%", s.glass_translucency.min(100))))
                                    .into_any_element(),
                            ),
                        ],
                        cx,
                    ))
                    .child(
                        v_flex().gap(px(8.)).child(kit::caption("ICONS", cx)).child(
                            kit::card(cx)
                                .child(kit::row("Icon style", "Used across the sidebar, toolbar, menus and files", icon_sets, true, &ch))
                                .child(kit::row("App icon", if cfg!(windows) { "Shown in the taskbar and window title; follows the accent color" } else { "Shown in the Dock and Finder; follows the accent color" }, app_icons, false, &ch).items_start()),
                        ),
                    )
                    .child(section(
                        "LAYOUT",
                        vec![
                            r(
                                "Density",
                                "Row height in lists",
                                self.choice("s-density", s.density, &[(Density::Compact, "Compact"), (Density::Comfortable, "Comfortable"), (Density::Spacious, "Spacious")], |s, v| s.density = v, cx),
                            ),
                            r(
                                "Icon size",
                                "Icons and thumbnails in the icon view",
                                self.choice("s-icon", s.grid_icon_size as u32, &[(48, "S"), (64, "M"), (80, "L"), (96, "XL")], |s, v| s.grid_icon_size = v as f32, cx),
                            ),
                            r("Image thumbnails", "Show image previews in the icon view", self.switch("s-thumbs", s.show_thumbnails, |s, v| s.show_thumbnails = v)),
                        ],
                        cx,
                    ))
                    .into_any_element()
            }
            Tab::Search => v_flex()
                .gap(px(16.))
                .child(section(
                    "SEARCH",
                    vec![
                        r("Respect .gitignore", "Skip ignored files in git repositories", self.switch("s-git", s.search_respect_gitignore, |s, v| s.search_respect_gitignore = v)),
                        r(if cfg!(windows) { "Prefer Windows Search index" } else { "Prefer Spotlight index" }, if cfg!(windows) { "Advanced search uses Windows Search by default" } else { "Advanced search uses macOS Spotlight by default" }, self.switch("s-spot", s.search_use_spotlight, |s, v| s.search_use_spotlight = v)),
                        r("Max results", "Search stops after this many hits", div().w(px(110.)).child(Input::new(&self.max_results_input).small()).into_any_element()),
                    ],
                    cx,
                ))
                .child({
                    let (dirs, items) = rile_services::index::stats();
                    let ws = self.ws.clone();
                    section(
                        "INSTANT SEARCH",
                        vec![
                            r(
                                "Remember opened folders",
                                "Folders you open are remembered, so searching them again is instant",
                                self.switch("s-index", s.index_discovered, |s, v| s.index_discovered = v),
                            ),
                            (
                                SharedString::from("Remembered"),
                                SharedString::from(format!("{items} items in {dirs} folders, on {} only", crate::platform_text::this_computer_lower())),
                                Button::new("s-index-clear").cursor_pointer()
                                    .small()
                                    .outline()
                                    .label("Clear")
                                    .pointer_unless(items == 0)
                                    .on_click(move |_, _, cx| {
                                        rile_services::index::clear();
                                        ws.update(cx, |_, cx| cx.notify());
                                    })
                                    .into_any_element(),
                            ),
                        ],
                        cx,
                    )
                })
                .child(kit::hint(
                    "Rile never crawls your disk in the background. Remembered results are re-checked on disk and shown first; the live search then finds everything else.",
                    cx,
                ))
                .into_any_element(),
            Tab::Programs => {
                let rows: Vec<(SharedString, SharedString, AnyElement)> = FileCategory::ALL
                    .into_iter()
                    .map(|cat| {
                        let key = Settings::category_key(cat);
                        let current = s.default_apps.get(&key).cloned();
                        let hint = match &current {
                            Some(app) => app.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            None => format!("System default · {}", cat.extensions().iter().take(5).map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ")),
                        };
                        let (ws_pick, ws_reset) = (self.ws.clone(), self.ws.clone());
                        let (key_pick, key_reset) = (key.clone(), key);
                        let controls = h_flex()
                            .gap(px(6.))
                            .when_some(current.clone(), |d, app| d.child(crate::icons::app_icon(&app, 20., ch.text_muted, cx)))
                            .child(Button::new(SharedString::from(format!("dp-pick-{key_pick}"))).cursor_pointer().small().outline().label("Choose…").on_click(
                                move |_, window, cx| {
                                    let ws = ws_pick.clone();
                                    let key = key_pick.clone();
                                    crate::workspace::open_with::choose_app(
                                        "Default Program",
                                        format!("Open {} files with", cat.label().to_lowercase()),
                                        window,
                                        cx,
                                        move |app, window, cx| {
                                            ws.update(cx, |ws, cx| ws.set_default_app(key.clone(), Some(app), window, cx));
                                        },
                                    );
                                },
                            ))
                            .when(current.is_some(), |d| {
                                d.child(
                                    Button::new(SharedString::from(format!("dp-reset-{key_reset}")))
                                        .cursor_pointer()
                                        .small()
                                        .ghost()
                                        .label("Reset")
                                        .on_click(move |_, window, cx| {
                                            ws_reset.update(cx, |ws, cx| ws.set_default_app(key_reset.clone(), None, window, cx));
                                        }),
                                )
                            })
                            .into_any_element();
                        (SharedString::from(cat.label()), SharedString::from(hint), controls)
                    })
                    .collect();
                v_flex()
                    .gap(px(16.))
                    .child(section("DEFAULT PROGRAMS", rows, cx))
                    .child(kit::hint(
                        "Files of these types open in the chosen app when you double-click them in Rile. Right-click → Open With picks an app just once.",
                        cx,
                    ))
                    .into_any_element()
            }
            Tab::Plugins => {
                let ws_reload = self.ws.clone();
                let ws_open = rile_plugins::default_plugin_dir();
                let plugin_rows: Vec<(SharedString, SharedString, AnyElement)> = plugin_infos
                    .into_iter()
                    .map(|p| {
                        let ws = self.ws.clone();
                        let id = p.id.clone();
                        (
                            SharedString::from(format!("{} {}", p.name, p.version)),
                            SharedString::from(p.description.clone()),
                            Switch::new(SharedString::from(format!("pl-{}", p.id))).cursor_pointer()
                                .checked(p.enabled)
                                .on_click(move |v, window, cx| {
                                    let v = *v;
                                    ws.update(cx, |ws, cx| ws.set_plugin_enabled(&id, v, window, cx));
                                })
                                .into_any_element(),
                        )
                    })
                    .collect();
                let has_plugins = !plugin_rows.is_empty();
                v_flex()
                    .gap(px(16.))
                    .child(section(
                        "EXTENSIONS",
                        vec![
                            r("Remote connections", "Built-in FTP, FTPS and SFTP (Network in the sidebar)", self.switch("s-remote", s.remote_enabled, |s, v| s.remote_enabled = v)),
                            r("Enable plugins", "Sandboxed WebAssembly components", self.switch("s-plugins", s.plugins_enabled, |s, v| s.plugins_enabled = v)),
                        ],
                        cx,
                    ))
                    .when(has_plugins, |d| d.child(section("INSTALLED", plugin_rows, cx)))
                    .children(plugin_errors.into_iter().map(|e| kit::notice(e, true, cx)))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(Button::new("pl-reload").cursor_pointer().small().outline().label("Reload Plugins").loading(plugins_loading).on_click(move |_, _, cx| {
                                ws_reload.update(cx, |ws, cx| ws.load_plugins(cx));
                            }))
                            .child(Button::new("pl-folder").cursor_pointer().small().outline().label("Open Plugin Folder").on_click(move |_, _, cx| {
                                if let Some(d) = &ws_open {
                                    std::fs::create_dir_all(d).ok();
                                    cx.open_with_system(d);
                                }
                            })),
                    )
                    .child(kit::hint("Build plugins with `cargo build --target wasm32-wasip2` and drop the .wasm into the plugin folder. See plugins/README.md.", cx))
                    .into_any_element()
            }
            Tab::Backups => self.backups.clone().into_any_element(),
            Tab::Nearby => self.render_nearby(&s, cx),
            Tab::Ai => self.ai.clone().into_any_element(),
            Tab::About => {
                let repo = s.update_repo.trim().to_string();
                let url = format!("https://github.com/{repo}");
                v_flex()
                    .gap(px(16.))
                    .child(
                        v_flex()
                            .items_center()
                            .gap(px(6.))
                            .py(px(16.))
                            .child(match crate::app_icon::png(s.app_icon, s.accent) {
                                Some(image) => { use gpui_kit::StyledImage; gpui_kit::img(image).size(px(96.)).object_fit(gpui_kit::ObjectFit::Contain).into_any_element() }
                                None => crate::icons::icon(IconName::Folder).size(px(56.)).text_color(ch.accent).into_any_element(),
                            })
                            .child(div().text_size(px(20.)).font_weight(gpui_kit::FontWeight::SEMIBOLD).text_color(ch.text_strong).child("Rile"))
                            .child(div().text_size(px(12.)).text_color(ch.text_muted).child(format!("Version {}", env!("CARGO_PKG_VERSION"))))
                            .child(div().text_size(px(12.)).text_color(ch.text).child("A fast, native file explorer.")),
                    )
                    .child(section(
                        "INFO",
                        vec![
                            r("Version", "Installed release", div().text_size(px(12.)).child(env!("CARGO_PKG_VERSION")).into_any_element()),
                            (
                                SharedString::from("Source"),
                                SharedString::from(if repo.is_empty() { "Not set".to_string() } else { repo.clone() }),
                                Button::new("about-gh")
                                    .cursor_pointer()
                                    .small()
                                    .outline()
                                    .label("Open on GitHub")
                                    .pointer_unless(repo.is_empty())
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                                    .into_any_element(),
                            ),
                        ],
                        cx,
                    ))
                    .into_any_element()
            }
        };

        h_flex()
            .gap(px(20.))
            .items_start()
            .h(px(520.))
            .child(nav)
            .child(div().w(px(1.)).self_stretch().bg(ch.hairline))
            .child(div().id("settings-body").flex_1().min_w_0().h_full().overflow_y_scroll().child(body))
    }
}
