//! Settings → Backups: create, edit and delete backup jobs.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Sizable, Side, h_flex,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder, px,
};
use rile_backup::{BackupJob, Destination, Schedule, Trigger};

use crate::icons::icon;
use crate::theme::chrome;
use crate::views::kit::{self, PointerUnless};
use crate::workspace::Workspace;

#[derive(Clone)]
struct DestinationChoice {
    id: String,
    name: String,
    detail: String,
    icon: IconName,
    protocol: rile_remote::Protocol,
    group: &'static str,
    online: Option<bool>,
}

impl DestinationChoice {
    fn glyph(&self, size: f32, cx: &App) -> AnyElement {
        let color = chrome(cx).text_strong;
        if self.protocol.is_cloud() {
            crate::icons::protocol_glyph(self.protocol, size, color)
        } else {
            icon(self.icon).size(px(size)).text_color(color).into_any_element()
        }
    }

    fn render(&self, cx: &App) -> AnyElement {
        let ch = chrome(cx);
        h_flex().w_full().cursor_pointer().gap(px(10.)).py(px(6.))
            .child(h_flex().flex_none().justify_center().size(px(36.)).rounded(px(8.)).bg(ch.text_strong.opacity(0.06))
                .child(self.glyph(22., cx)))
            .child(v_flex().flex_1().min_w_0().gap(px(2.))
                .child(div().truncate().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(ch.text_strong).child(self.name.clone()))
                .child(div().truncate().text_size(px(11.)).text_color(ch.text_muted).child(self.detail.clone())))
            .when_some(self.online, |row, online| {
                let color = if online { ch.success } else { ch.text_muted };
                row.child(h_flex().flex_none().gap(px(5.)).px(px(7.)).py(px(3.)).rounded(px(5.)).bg(color.opacity(0.1))
                    .child(div().size(px(5.)).rounded_full().bg(color))
                    .child(div().text_size(px(10.)).text_color(color).child(if online { "Online" } else { "Offline" })))
            }).into_any_element()
    }
}

pub struct BackupSettingsView {
    ws: Entity<Workspace>,
    selected: Option<String>,
    name: Entity<InputState>,
    remote_path: Entity<InputState>,
    excludes: Entity<InputState>,
    /// "Delete" was clicked once; the next click deletes.
    confirm_delete: bool,
    _subs: Vec<Subscription>,
}

impl BackupSettingsView {
    pub fn new(ws: Entity<Workspace>, selected: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Backup name"));
        let remote_path = cx.new(|cx| InputState::new(window, cx).placeholder("/Backups/Rile"));
        let excludes = cx.new(|cx| InputState::new(window, cx).placeholder("node_modules, *.tmp"));
        let mut subs = vec![cx.observe(&ws, |_, _, cx| cx.notify())];
        for input in [&name, &remote_path, &excludes] {
            subs.push(cx.subscribe_in(input, window, |_: &mut Self, _, ev: &InputEvent, window, cx| {
                // Deferred: the event may arrive while the workspace updates.
                if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    cx.defer_in(window, |this, _, cx| this.commit_inputs(cx));
                }
            }));
        }
        // Created from inside workspace updates (actions, page buttons), so
        // the workspace is read only once that update is over.
        cx.defer_in(window, move |this: &mut Self, window, cx| {
            let selected = selected.or_else(|| this.ws.read(cx).backups.config.job.first().map(|j| j.id.clone()));
            this.select(selected, window, cx);
        });
        Self { ws, selected: None, name, remote_path, excludes, confirm_delete: false, _subs: subs }
    }

    fn job(&self, cx: &App) -> Option<BackupJob> {
        let id = self.selected.as_ref()?;
        self.ws.read(cx).backups.config.get(id).cloned()
    }

    /// Shows another job, saving edits to the current one first.
    fn select(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        self.selected = id;
        self.confirm_delete = false;
        let job = self.job(cx).unwrap_or_default();
        let remote = match &job.destination {
            Destination::Remote { path, .. } => path.clone(),
            Destination::Local { .. } => String::new(),
        };
        self.name.update(cx, |i, cx| i.set_value(job.name.clone(), window, cx));
        self.remote_path.update(cx, |i, cx| i.set_value(remote, window, cx));
        self.excludes.update(cx, |i, cx| i.set_value(job.excludes.join(", "), window, cx));
        cx.notify();
    }

    /// Writes the text fields into the selected job (when they changed).
    fn commit_inputs(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.job(cx) else { return };
        let name = self.name.read(cx).value().trim().to_string();
        let remote = self.remote_path.read(cx).value().trim().to_string();
        let excludes: Vec<String> = self.excludes.read(cx).value().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let mut edited = job.clone();
        if !name.is_empty() {
            edited.name = name;
        }
        if let Destination::Remote { path, .. } = &mut edited.destination {
            *path = remote;
        }
        edited.excludes = excludes;
        if edited != job {
            self.ws.update(cx, |ws, cx| ws.update_backup_job(&job.id, cx, move |j| *j = edited));
        }
    }

    /// A click handler that edits the selected job.
    fn edit(&self, f: impl Fn(&mut BackupJob) + 'static) -> impl Fn(&mut Window, &mut App) + 'static {
        let ws = self.ws.clone();
        let id = self.selected.clone().unwrap_or_default();
        move |_, cx| ws.update(cx, |ws, cx| ws.update_backup_job(&id, cx, &f))
    }

    fn add_job(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.ws.update(cx, |ws, cx| ws.add_backup_job(Vec::new(), cx));
        self.select(Some(id), window, cx);
    }

    fn pick_folders(&self, cx: &mut Context<Self>, multiple: bool, apply: impl FnOnce(&mut BackupJob, Vec<PathBuf>) + 'static) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple,
            prompt: Some(if multiple { "Back Up".into() } else { "Choose".into() }),
        });
        let ws = self.ws.clone();
        let id = self.selected.clone().unwrap_or_default();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && !paths.is_empty()
            {
                cx.update(|cx| ws.update(cx, |ws, cx| ws.update_backup_job(&id, cx, move |j| apply(j, paths))));
            }
        })
        .detach();
    }
}

fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    current: T,
    options: &[(T, String)],
    on: impl Fn(T) -> Box<dyn Fn(&mut Window, &mut App)> + 'static,
    cx: &App,
) -> AnyElement {
    kit::segmented(cx)
        .flex_wrap()
        .children(options.iter().enumerate().map(|(i, (v, label))| {
            let f = on(*v);
            kit::segment((id, i), label.clone(), *v == current, cx).on_click(move |_, window, cx| f(window, cx))
        }))
        .into_any_element()
}

impl Render for BackupSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ch = chrome(cx);
        let theme = cx.theme().clone();
        let ws = self.ws.read(cx);
        let jobs: Vec<(String, String)> = ws.backups.config.job.iter().map(|j| (j.id.clone(), j.name.clone())).collect();
        let mut connections = ws.connections.clone();
        for device in &ws.nearby.trusted {
            connections.push(rile_remote::peer::connection_info(&device.id, &device.name, "", 0));
        }
        let choices: Vec<DestinationChoice> = connections.iter().map(|c| {
            let peer = rile_remote::peer::peer_of_conn(&c.id)
                .and_then(|id| ws.nearby.trusted.iter().find(|p| p.id == id));
            let (group, glyph, detail, online) = if let Some(peer) = peer {
                let online = ws.nearby.online.iter().any(|p| p.id == peer.id);
                ("Nearby devices", super::devices::kind_icon(peer.kind),
                    if peer.os.is_empty() { "Paired device · Local network".into() } else { format!("{} · Local network", peer.os) }, Some(online))
            } else if c.protocol.is_cloud() {
                ("Cloud drives", IconName::Cloud, c.protocol.label().to_string(), None)
            } else {
                ("Servers", IconName::Server, format!("{} · {}", c.protocol.label(), c.host), None)
            };
            DestinationChoice {
                id: c.id.clone(), name: if c.name.is_empty() { c.display() } else { c.name.clone() },
                detail, icon: glyph, protocol: c.protocol, group, online,
            }
        }).collect();
        let agent_error = ws.backups.agent_error.clone();
        let menu_bar = ws.backups.config.menu_bar;
        let running = self.selected.as_ref().is_some_and(|id| ws.backups.running.contains_key(id));
        let job = self.job(cx);
        if self.selected.is_some() && job.is_none() {
            self.selected = None;
        }
        let r = |label: &'static str, hint: String, control: AnyElement| (SharedString::from(label), SharedString::from(hint), control);
        let section = |title: &'static str, items: Vec<(SharedString, SharedString, AnyElement)>, cx: &App| {
            v_flex().gap(px(8.)).child(kit::caption(title, cx)).child(kit::rows(items, cx))
        };

        let strip = h_flex()
            .gap(px(6.))
            .flex_wrap()
            .children(jobs.iter().enumerate().map(|(i, (id, name))| {
                let active = self.selected.as_ref() == Some(id);
                let id = id.clone();
                h_flex()
                    .id(("bk-job", i))
                    .gap(px(6.))
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .border_1()
                    .border_color(if active { ch.accent.opacity(0.5) } else { ch.hairline })
                    .text_color(if active { ch.text_strong } else { ch.text })
                    .when(active, |d| d.bg(ch.accent.opacity(0.10)))
                    .when(!active, |d| d.hover(|d| d.bg(ch.hover)))
                    .child(icon(IconName::DatabaseBackup).size(px(13.)).text_color(if active { ch.accent } else { ch.text_muted }))
                    .child(name.clone())
                    .on_click(cx.listener(move |this, _, window, cx| this.select(Some(id.clone()), window, cx)))
            }))
            .child(Button::new("bk-add").cursor_pointer().small().ghost().icon(IconName::Plus).label("New Backup")
                .on_click(cx.listener(|this, _, window, cx| this.add_job(window, cx))));

        let Some(job) = job else {
            return v_flex()
                .gap(px(16.))
                .child(strip)
                .child(kit::hint("Back up folders to another drive, a folder or one of your servers. After the first run only new and changed files are copied; replaced and deleted files are kept as older versions.", cx))
                .into_any_element();
        };

        // Folders
        let mut folders = kit::card(cx);
        for (i, source) in job.sources.iter().enumerate() {
            let remove = source.clone();
            let missing = !source.exists();
            folders = folders.child(
                h_flex()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(8.))
                    .when(i > 0, |d| d.border_t_1().border_color(ch.hairline))
                    .child(icon(IconName::Folder).size(px(14.)).text_color(ch.accent).flex_none())
                    .child(v_flex().flex_1().min_w_0()
                        .child(div().text_size(px(12.)).text_color(ch.text).truncate().child(source.display().to_string()))
                        .when(missing, |d| d.child(div().text_size(px(10.)).text_color(theme.warning).child("Not available right now"))))
                    .child(Button::new(("bk-src-rm", i)).cursor_pointer().xsmall().ghost().icon(IconName::X)
                        .on_click({
                            let f = self.edit(move |j| j.sources.retain(|s| *s != remove));
                            move |_, w, cx| f(w, cx)
                        })),
            );
        }
        folders = folders.child(
            h_flex()
                .px(px(14.))
                .py(px(8.))
                .when(!job.sources.is_empty(), |d| d.border_t_1().border_color(ch.hairline))
                .child(Button::new("bk-src-add").cursor_pointer().small().outline().icon(IconName::FolderPlus).label("Add Folders…")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pick_folders(cx, true, |j, paths| {
                            for p in paths {
                                if !j.sources.contains(&p) {
                                    j.sources.push(p);
                                }
                            }
                        })
                    }))),
        );

        // Destination
        let remote = matches!(job.destination, Destination::Remote { .. });
        let first_conn = connections.first().map(|c| c.id.clone());
        let kind = segmented(
            "bk-dest-kind",
            remote,
            &[(false, "Folder or Drive".into()), (true, "Server or Device".into())],
            {
                let (ws, id) = (self.ws.clone(), job.id.clone());
                move |to_remote| {
                    let (ws, id, first_conn) = (ws.clone(), id.clone(), first_conn.clone());
                    Box::new(move |_, cx| {
                        let first_conn = first_conn.clone();
                        ws.update(cx, |ws, cx| ws.update_backup_job(&id, cx, |j| {
                            let is_remote = matches!(j.destination, Destination::Remote { .. });
                            if to_remote != is_remote {
                                j.destination = if to_remote {
                                    Destination::Remote { conn: first_conn.unwrap_or_default(), path: String::new() }
                                } else {
                                    Destination::default()
                                };
                            }
                        }))
                    })
                }
            },
            cx,
        );
        let mut dest_rows = vec![r("Store on", "Where the backup is written".into(), kind)];
        match &job.destination {
            Destination::Local { path } => {
                let hint = if path.as_os_str().is_empty() { "Choose an empty folder, e.g. on an external drive".to_string() } else { path.display().to_string() };
                dest_rows.push(r(
                    "Folder",
                    hint,
                    Button::new("bk-dest-choose").cursor_pointer().small().outline().label("Choose…")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pick_folders(cx, false, |j, paths| {
                                if let Some(p) = paths.into_iter().next() {
                                    j.destination = Destination::Local { path: p };
                                }
                            })
                        }))
                        .into_any_element(),
                ));
            }
            Destination::Remote { conn, .. } => {
                let control = if connections.is_empty() {
                    div().text_size(px(11.)).text_color(ch.text_faint).child("Add a server under Network or pair a device under Nearby first").into_any_element()
                } else {
                    let current = conn.clone();
                    let selected = choices.iter().find(|c| c.id == current);
                    let label = selected.map(|c| c.name.clone()).unwrap_or_else(|| "Choose destination…".into());
                    let glyph = selected.map(|c| c.glyph(18., cx))
                        .unwrap_or_else(|| icon(IconName::HardDrive).size(px(18.)).text_color(ch.text_strong).into_any_element());
                    let (ws, id) = (self.ws.clone(), job.id.clone());
                    Button::new("bk-conn").cursor_pointer().small().outline().h(px(34.)).w(px(260.)).max_w_full()
                        .dropdown_caret(true).child(glyph)
                        .child(div().flex_1().min_w_0().truncate().text_left().child(label))
                        .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |mut menu, _, _| {
                            menu = menu.min_w(px(340.)).max_w(px(400.)).check_side(Side::Right);
                            let mut first = true;
                            for group in ["Servers", "Cloud drives", "Nearby devices"] {
                                if !choices.iter().any(|c| c.group == group) { continue; }
                                if !first { menu = menu.separator(); }
                                first = false;
                                menu = menu.label(group);
                                for c in choices.iter().filter(|c| c.group == group) {
                                    let (ws, id, conn) = (ws.clone(), id.clone(), c.id.clone());
                                    let choice = c.clone();
                                    menu = menu.item(PopupMenuItem::element(move |_, cx| choice.render(cx)).checked(c.id == current).on_click(move |_, _, cx| {
                                        let conn = conn.clone();
                                        ws.update(cx, |ws, cx| ws.update_backup_job(&id, cx, move |j| {
                                            if let Destination::Remote { conn: c, .. } = &mut j.destination {
                                                *c = conn;
                                            }
                                        }));
                                    }));
                                }
                            }
                            menu
                        }).into_any_element()
                };
                dest_rows.push(r("Server or device", "A saved server, cloud drive or paired Nearby device".into(), control));
                let hint = if rile_remote::peer::peer_of_conn(conn).is_some() {
                    "Use a subfolder of a writable share, e.g. /Shared/Backups. Enable writing in Settings → Nearby on the receiving device."
                } else {
                    "Created if it doesn't exist"
                };
                dest_rows.push(r("Destination folder", hint.into(), div().w(px(200.)).child(Input::new(&self.remote_path).small()).into_any_element()));
            }
        }

        // When
        let schedules: Vec<(Schedule, String)> = Schedule::ALL.iter().map(|s| (*s, s.label().to_string())).collect();
        let (ws_s, id_s) = (self.ws.clone(), job.id.clone());
        let schedule = segmented("bk-schedule", job.schedule, &schedules, move |s| {
            let (ws, id) = (ws_s.clone(), id_s.clone());
            Box::new(move |_, cx| ws.update(cx, |ws, cx| ws.update_backup_job(&id, cx, move |j| j.schedule = s)))
        }, cx);
        let switch = |id: &'static str, value: bool, enabled: bool, f: fn(&mut BackupJob, bool)| -> AnyElement {
            let (ws, job_id) = (self.ws.clone(), job.id.clone());
            Switch::new(id).checked(value).pointer_unless(!enabled)
                .on_click(move |v, _, cx| {
                    let v = *v;
                    ws.update(cx, |ws, cx| ws.update_backup_job(&job_id, cx, move |j| f(j, v)));
                })
                .into_any_element()
        };
        let mac = cfg!(any(target_os = "macos", windows));
        let background_hint = if !mac {
            "Only available on macOS and Windows".to_string()
        } else if job.schedule == Schedule::Manual {
            "Pick a schedule to run it in the background".to_string()
        } else if cfg!(windows) {
            "Task Scheduler keeps the schedule while Rile is closed".to_string()
        } else {
            "macOS keeps the schedule while Rile is closed".to_string()
        };

        // Options
        let keep: Vec<(u32, String)> = [(7, "7 days"), (30, "30 days"), (90, "90 days"), (365, "1 year"), (0, "Forever")].iter().map(|(d, l)| (*d, l.to_string())).collect();
        let (ws_k, id_k) = (self.ws.clone(), job.id.clone());
        let keep = segmented("bk-keep", job.keep_versions_days, &keep, move |d| {
            let (ws, id) = (ws_k.clone(), id_k.clone());
            Box::new(move |_, cx| ws.update(cx, |ws, cx| ws.update_backup_job(&id, cx, move |j| j.keep_versions_days = d)))
        }, cx);

        let delete_label = if self.confirm_delete { "Click Again to Delete" } else { "Delete Backup Job" };
        let footer = h_flex()
            .justify_between()
            .gap_2()
            .child(Button::new("bk-delete").cursor_pointer().small().ghost().when(self.confirm_delete, |b| b.danger()).icon(IconName::Trash).label(delete_label)
                .on_click(cx.listener(|this, _, window, cx| {
                    if !this.confirm_delete {
                        this.confirm_delete = true;
                        cx.notify();
                        return;
                    }
                    let Some(id) = this.selected.clone() else { return };
                    this.ws.update(cx, |ws, cx| ws.remove_backup_job(&id, cx));
                    let next = this.ws.read(cx).backups.config.job.first().map(|j| j.id.clone());
                    this.selected = None;
                    this.select(next, window, cx);
                })))
            .child(Button::new("bk-run").cursor_pointer().small().primary().icon(IconName::Play)
                .label(if running { "Backing Up…" } else { "Back Up Now" })
                .pointer_unless(running || job.problem().is_some())
                .on_click({
                    let (ws, id) = (self.ws.clone(), job.id.clone());
                    cx.listener(move |this, _, window, cx| {
                        this.commit_inputs(cx);
                        ws.update(cx, |ws, cx| ws.run_backup(&id, Trigger::Manual, false, window, cx));
                    })
                }));

        v_flex()
            .gap(px(16.))
            .child(strip)
            .when_some(job.problem(), |d, p| d.child(kit::notice(p, false, cx)))
            .child(section(
                "BACKUP",
                vec![
                    r("Name", "Shown on the Backups page".into(), div().w(px(200.)).child(Input::new(&self.name).small()).into_any_element()),
                    r("Active", "Off pauses the schedule and change detection".into(), switch("bk-enabled", job.enabled, true, |j, v| j.enabled = v)),
                ],
                cx,
            ))
            .child(v_flex().gap(px(8.)).child(kit::caption("FOLDERS TO BACK UP", cx)).child(folders))
            .child(section("DESTINATION", dest_rows, cx))
            .child(section(
                "WHEN",
                vec![
                    r("Schedule", "Missed runs catch up when Rile starts".into(), schedule),
                    r("When files change", "Back up 30 seconds after changes settle (while Rile is open)".into(), switch("bk-watch", job.watch, true, |j, v| j.watch = v)),
                    r("When Rile is closed", background_hint, switch("bk-bg", job.background, mac && job.schedule != Schedule::Manual, |j, v| j.background = v)),
                ],
                cx,
            ))
            .when_some(agent_error, |d, e| d.child(kit::notice(format!("Background backups: {e}"), true, cx)))
            .when(cfg!(any(target_os = "macos", windows)), |d| {
                let ws = self.ws.clone();
                let (title, label, hint) = if cfg!(windows) {
                    ("NOTIFICATION AREA", "Show in notification area", "Follow, start and stop backups from the taskbar's notification area, also while Rile is closed")
                } else {
                    ("MENU BAR", "Show in menu bar", "Follow, start and stop backups from the menu bar, also while Rile is closed")
                };
                d.child(section(
                    title,
                    vec![r(
                        label,
                        hint.into(),
                        Switch::new("bk-menu-bar").cursor_pointer().checked(menu_bar)
                            .on_click(move |v, _, cx| {
                                let v = *v;
                                ws.update(cx, |ws, cx| ws.set_backup_menu_bar(v, cx));
                            })
                            .into_any_element(),
                    )],
                    cx,
                ))
            })
            .child(section(
                "OPTIONS",
                vec![
                    r("Keep older versions", "Replaced and deleted files, in .rile-versions".into(), keep),
                    r("Skip", "Comma-separated names or patterns".into(), div().w(px(200.)).child(Input::new(&self.excludes).small()).into_any_element()),
                ],
                cx,
            ))
            .child(footer)
            .into_any_element()
    }
}
