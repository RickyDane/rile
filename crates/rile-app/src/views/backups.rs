//! The Backups page (`Location::Backups`): every backup job with its state,
//! live progress, recent runs and quick actions. Jobs are set up in
//! Settings → Backups.


use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, h_flex,
    button::{Button, ButtonVariant, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, WeakEntity, Window, div, prelude::FluentBuilder, px,
};
use rile_backup::manage::{RestoreFrom, RestoreTo};
use rile_backup::{BackupJob, Destination, Outcome, RunRecord, Trigger};

use super::chrome::content_class;
use super::home::{PAGE_MAX_WIDTH, PAGE_TITLE_GAP, card, page_padding, page_title, page_title_top};
use crate::dialogs::Choice;
use crate::icons::icon;
use crate::views::kit::PointerUnless;
use crate::theme::{Chrome, chrome};
use crate::workspace::Workspace;
use crate::workspace::backups::describe;

/// Recent runs listed per job.
const RECENT_RUNS: usize = 5;

fn text(value: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().text_size(px(size)).text_color(color).child(value.into())
}

fn size(bytes: u64) -> String {
    humansize::format_size(bytes, humansize::DECIMAL)
}

/// "5 min ago" for a Unix timestamp in the past.
fn ago(ts: i64) -> String {
    let d = (chrono::Utc::now().timestamp() - ts).max(0);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", d / 60),
        3600..86_400 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86_400),
    }
}

/// "in 3 h" for a Unix timestamp, or "soon" when overdue.
fn until(ts: i64) -> String {
    let d = ts - chrono::Utc::now().timestamp();
    match d {
        ..60 => "soon".into(),
        60..3600 => format!("in {} min", d / 60),
        3600..86_400 => format!("in {} h", d / 3600),
        _ => format!("in {} days", d / 86_400),
    }
}

fn when(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(ts, 0).single().map(|t| t.format("%b %-d, %H:%M").to_string()).unwrap_or_default()
}

pub fn destination_label(dest: &Destination) -> String {
    match dest {
        Destination::Local { path } if path.as_os_str().is_empty() => "No destination".into(),
        Destination::Local { path } => path.display().to_string(),
        Destination::Remote { conn, path } => format!("{}:{}", crate::remote::label(conn), path),
    }
}

fn sources_label(job: &BackupJob) -> String {
    let names: Vec<String> = job
        .sources
        .iter()
        .map(|s| s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| s.display().to_string()))
        .collect();
    match names.len() {
        0 => "No folders".into(),
        1..=3 => names.join(", "),
        n => format!("{}, and {} more", names[..2].join(", "), n - 2),
    }
}

fn chip(label: impl Into<SharedString>, ic: IconName, ch: &Chrome) -> Div {
    h_flex()
        .gap(px(5.))
        .px(px(7.))
        .py(px(2.))
        .rounded(px(5.))
        .bg(ch.hover)
        .text_size(px(10.))
        .text_color(ch.text_muted)
        .child(icon(ic).size(px(11.)))
        .child(label.into())
}

fn status_pill(label: &'static str, color: Hsla) -> Div {
    text(label, 10., color).font_weight(FontWeight::MEDIUM).px(px(6.)).py(px(1.)).rounded(px(4.)).bg(color.opacity(0.12)).flex_none()
}

pub fn render(ws: &mut Workspace, slot: usize, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let width = ws.slot_width(slot, window);
    let class = content_class(width);
    let (pad_x, _) = page_padding(class);

    let title = h_flex()
        .mb(px(PAGE_TITLE_GAP))
        .justify_between()
        .gap_2()
        .child(page_title("Backups", class, &ch))
        .when(!ws.backups.config.job.is_empty(), |d| {
            d.child(Button::new("backup-new").cursor_pointer().outline().icon(IconName::Plus).label("New Backup").on_click(cx.listener(
                |ws, _, w, cx| {
                    let id = ws.add_backup_job(Vec::new(), cx);
                    crate::dialogs::open_backup_settings(cx.entity(), ws.settings.clone(), Some(id), w, cx);
                },
            )))
        });
    let mut page = v_flex().gap(px(18.));
    if ws.backups.config.job.is_empty() {
        page = page.child(empty_state(&ch, cx));
    } else {
        for (ix, job) in ws.backups.config.job.clone().iter().enumerate() {
            page = page.child(job_card(ws, ix, job, &ch, cx));
        }
    }
    page = page.child(
        h_flex()
            .gap_2()
            .text_color(ch.text_faint)
            .child(icon(IconName::Info).size(px(12.)))
            .child(text(
                "Backups are plain folders: browse or restore them like any other files. Replaced and deleted files are kept in .rile-versions.",
                11.,
                ch.text_faint,
            )),
    );
    div()
        .id(("backups-page", slot))
        .debug_selector(|| "backups-page".into())
        .size_full()
        .bg(ch.browser)
        .overflow_y_scroll()
        .child(v_flex().w_full().max_w(px(PAGE_MAX_WIDTH)).mx_auto().px(px(pad_x)).pt(px(page_title_top(class))).pb(px(28.)).child(title).child(page))
        .into_any_element()
}

fn empty_state(ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    card(ch)
        .id("backup-welcome")
        .debug_selector(|| "backup-welcome".into())
        .child(
            h_flex().gap(px(24.)).p(px(26.)).items_center()
                .child(div().size(px(64.)).flex_none().rounded(px(16.)).bg(ch.accent.opacity(0.12)).flex().items_center().justify_center()
                    .child(icon(IconName::DatabaseBackup).size(px(30.)).text_color(ch.accent)))
                .child(v_flex().flex_1().min_w_0().gap(px(10.))
                    .child(text("Keep a copy of what matters.", 18., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
                    .child(text(
                        "Choose folders and a destination — another drive, a folder or one of your servers. After the first copy, Rile only copies what changed and keeps older versions of replaced or deleted files.",
                        12.,
                        ch.text_muted,
                    ).max_w(px(560.)))
                    .child(h_flex().mt(px(6.)).child(
                        Button::new("backup-create").cursor_pointer().primary().icon(IconName::Plus).label("Create a Backup").on_click(cx.listener(|ws, _, w, cx| {
                            let id = ws.add_backup_job(Vec::new(), cx);
                            crate::dialogs::open_backup_settings(cx.entity(), ws.settings.clone(), Some(id), w, cx);
                        })),
                    ))),
        )
        .into_any_element()
}

fn job_card(ws: &Workspace, ix: usize, job: &BackupJob, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let theme = cx.theme().clone();
    let history = ws.backups.history(&job.id);
    let running = ws.backups.running.get(&job.id);
    let live = ws.backups.live(&job.id);
    let last = history.first();
    let (status, status_color) = if live.is_some() {
        ("Backing up", ch.accent)
    } else if ws.backup_task_running(&job.id) {
        ("Working…", ch.accent)
    } else if job.problem().is_some() {
        ("Needs setup", theme.warning)
    } else if !job.enabled {
        ("Paused", ch.text_muted)
    } else {
        match last.map(|r| &r.outcome) {
            None => ("Not backed up yet", ch.text_muted),
            Some(Outcome::Success) => ("Up to date", ch.success),
            Some(Outcome::Partial(_)) => ("Some files skipped", theme.warning),
            Some(Outcome::Failed(_)) => ("Failed", theme.danger),
            Some(Outcome::Cancelled) => ("Cancelled", ch.text_muted),
        }
    };

    let id = job.id.clone();
    let busy = ws.backup_task_running(&job.id);
    let has_dest = job.destination.is_set();
    let weak = cx.entity().downgrade();
    let actions = if live.is_some() {
        h_flex().gap_2().child(Button::new(("backup-cancel", ix)).cursor_pointer().outline().icon(IconName::Square).label("Stop").on_click(
            cx.listener(move |ws, _, _, cx| ws.cancel_backup(&id, cx)),
        ))
    } else {
        let (run, edit, restore) = (id.clone(), id.clone(), id.clone());
        let name = job.name.clone();
        h_flex()
            .gap_2()
            .child(
                Button::new(("backup-run", ix)).cursor_pointer().primary().icon(IconName::Play).label("Back Up Now")
                    .pointer_unless(job.problem().is_some() || busy)
                    .on_click(cx.listener(move |ws, _, w, cx| ws.run_backup(&run, Trigger::Manual, false, w, cx))),
            )
            .child(
                Button::new(("backup-restore", ix)).debug_selector(move || format!("backup-restore-{ix}")).cursor_pointer().outline().icon(IconName::ArchiveRestore).label("Restore…")
                    .pointer_unless(!has_dest || busy || !history.iter().any(|r| r.outcome.is_success()))
                    .on_click({
                        let weak = weak.clone();
                        move |_, window, cx| {
                            confirm_restore(weak.clone(), restore.clone(), RestoreFrom::Latest,
                                format!("Restore “{name}”?"),
                                "Copies the files of the latest backup back to this Mac. Files that are already identical are skipped; files that differ are replaced, and their current versions are moved to the Trash.".into(),
                                window, cx)
                        }
                    }),
            )
            .child(Button::new(("backup-edit", ix)).cursor_pointer().outline().icon(IconName::SlidersHorizontal).label("Edit").on_click(
                cx.listener(move |ws, _, w, cx| crate::dialogs::open_backup_settings(cx.entity(), ws.settings.clone(), Some(edit.clone()), w, cx)),
            ))
            .child(more_menu(ix, job, weak.clone(), busy))
    };

    let header = h_flex()
        .gap(px(12.))
        .items_start()
        .child(div().size(px(36.)).flex_none().rounded(px(9.)).bg(ch.accent.opacity(0.10)).flex().items_center().justify_center()
            .child(icon(IconName::DatabaseBackup).size(px(17.)).text_color(ch.accent)))
        .child(v_flex().flex_1().min_w_0().gap(px(3.))
            .child(h_flex().gap_2().child(text(job.name.clone(), 14., ch.text_strong).font_weight(FontWeight::SEMIBOLD).truncate()).child(status_pill(status, status_color)))
            .child(h_flex().gap(px(6.)).min_w_0().text_size(px(11.)).text_color(ch.text_muted)
                .child(div().flex_shrink_0().max_w(px(260.)).truncate().child(sources_label(job)))
                .child(icon(IconName::ArrowRight).size(px(11.)).flex_none())
                .child(div().truncate().child(destination_label(&job.destination)))))
        .child(actions);

    // Live progress, or the last result and the next run.
    let body: AnyElement = if let Some(status) = &live {
        let copying = status.files_total > 0 || status.bytes_total > 0;
        let detail = if copying {
            format!("{} of {} changes · {} of {}", status.files_done, status.files_total, size(status.bytes_done), size(status.bytes_total))
        } else if status.scanned > 0 {
            format!("{} files found", status.scanned)
        } else {
            String::new()
        };
        let label = if running.is_some() { format!("{}…", status.phase) } else { format!("{}… (in the background)", status.phase) };
        let rates = rile_backup::format_rates(status.bytes_per_sec, status.items_per_sec);
        v_flex()
            .gap(px(6.))
            .child(h_flex().justify_between().gap_2()
                .child(text(label, 11., ch.text).font_weight(FontWeight::MEDIUM))
                .child(h_flex().gap(px(10.))
                    .when(!rates.is_empty(), |d| d.child(text(rates, 11., ch.accent).font_weight(FontWeight::MEDIUM)))
                    .child(text(detail, 11., ch.text_muted))))
            .child(crate::progress::activity_bar(("backup-bar", ix), status.fraction(), 5., cx))
            .when(!status.current.is_empty(), |d| d.child(text(status.current.clone(), 10., ch.text_faint).truncate()))
            .into_any_element()
    } else {
        let mut lines = v_flex().gap(px(3.));
        if let Some(problem) = job.problem() {
            lines = lines.child(text(problem, 11., theme.warning));
        } else if let Some(r) = last {
            let color = match r.outcome {
                Outcome::Failed(_) => theme.danger,
                Outcome::Partial(_) => theme.warning,
                _ => ch.text_muted,
            };
            lines = lines.child(text(format!("Last backup {} — {}", ago(r.finished), describe(r)), 11., color).truncate());
        }
        let next = match rile_backup::next_run(job, history) {
            Some(t) => Some(format!("Next backup {}", until(t))),
            None if job.watch => Some("Backs up after files change".into()),
            None => None,
        };
        let pending = job.watch && ws.backups.has_pending_changes(&job.id);
        lines
            .when_some(next.filter(|_| job.enabled && job.problem().is_none()), |d, n| d.child(text(n, 11., ch.text_faint)))
            .when(pending, |d| d.child(text("Changes detected — backing up shortly", 11., ch.accent)))
            .into_any_element()
    };

    let mut chips = h_flex().gap(px(6.)).flex_wrap().child(chip(
        match job.schedule {
            rile_backup::Schedule::Manual => "Manual".to_string(),
            s => s.label().to_string(),
        },
        IconName::Clock,
        ch,
    ));
    if job.watch {
        chips = chips.child(chip("On changes", IconName::FolderSync, ch));
    }
    if job.background {
        chips = chips.child(chip("Also when Rile is closed", IconName::Moon, ch));
    }
    chips = chips.child(chip(
        if job.keep_versions_days == 0 { "Keeps all versions".to_string() } else { format!("Keeps versions {} days", job.keep_versions_days) },
        IconName::ArchiveRestore,
        ch,
    ));
    if let Some(r) = history.iter().find(|r| r.outcome.is_success()) {
        chips = chips.child(chip(format!("{} files", r.files), IconName::Files, ch));
    }

    let expanded = ws.backups.expanded.contains(&job.id);
    let shown = if expanded { history.len() } else { RECENT_RUNS };
    let recent = history.iter().take(shown).enumerate().fold(v_flex().gap(px(2.)), |list, (i, r)| {
        list.child(run_row(ix, i, job, r, live.is_some() || busy, weak.clone(), ch, &theme))
    });
    let toggle = (history.len() > RECENT_RUNS).then(|| {
        let id = job.id.clone();
        Button::new(("backup-runs-toggle", ix)).cursor_pointer().ghost()
            .label(if expanded { "Show fewer runs".to_string() } else { format!("Show all {} runs", history.len()) })
            .on_click(cx.listener(move |ws, _, _, cx| {
                if !ws.backups.expanded.remove(&id) {
                    ws.backups.expanded.insert(id.clone());
                }
                cx.notify();
            }))
    });

    card(ch)
        .id(("backup-job", ix))
        .p(px(16.))
        .child(
            v_flex()
                .gap(px(12.))
                .child(header)
                .child(body)
                .child(chips)
                .when(!history.is_empty(), |d| {
                    d.child(v_flex().gap(px(4.)).pt(px(8.)).border_t_1().border_color(ch.hairline)
                        .child(text(if expanded { "All runs" } else { "Recent runs" }, 10., ch.text_faint).font_weight(FontWeight::SEMIBOLD))
                        .child(recent)
                        .children(toggle))
                }),
        )
        .into_any_element()
}

type MenuAction = Box<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

/// The ⋯ menu of a job: checks, folders and deletions.
fn more_menu(ix: usize, job: &BackupJob, weak: WeakEntity<Workspace>, busy: bool) -> impl IntoElement {
    let (id, name, can_check, has_dest) = (job.id.clone(), job.name.clone(), job.problem().is_none() && !busy, job.destination.is_set());
    let dest = destination_label(&job.destination);
    Button::new(("backup-more", ix)).cursor_pointer().ghost().icon(IconName::Ellipsis).dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
        let item = |label: &'static str, disabled: bool, f: MenuAction| {
            let weak = weak.clone();
            PopupMenuItem::new(label).disabled(disabled).on_click(move |_, window, cx| {
                weak.update(cx, |ws, cx| f(ws, window, cx)).ok();
            })
        };
        let (check, show, older) = (id.clone(), id.clone(), id.clone());
        let (weak_v, weak_d, id_v, id_d, name_v, name_d, dest_d) = (weak.clone(), weak.clone(), id.clone(), id.clone(), name.clone(), name.clone(), dest.clone());
        menu.item(item("Check & Repair", !can_check, Box::new(move |ws, w, cx| ws.run_backup(&check, Trigger::Manual, true, w, cx))))
            .item(item("Show Backup", !has_dest, Box::new(move |ws, w, cx| ws.open_backup_location(&show, false, w, cx))))
            .item(item("Show Older Versions", !has_dest, Box::new(move |ws, w, cx| ws.open_backup_location(&older, true, w, cx))))
            .separator()
            .item(PopupMenuItem::new("Delete Older Versions…").disabled(busy || !has_dest).on_click(move |_, window, cx| {
                let (weak, id) = (weak_v.clone(), id_v.clone());
                crate::dialogs::confirm(
                    format!("Delete all older versions kept by “{name_v}”? Replaced and deleted files can then no longer be restored."),
                    "Delete Versions",
                    true,
                    window,
                    cx,
                    move |window, cx| {
                        weak.update(cx, |ws, cx| ws.delete_backup_versions(&id, window, cx)).ok();
                    },
                );
            }))
            .item(PopupMenuItem::new("Delete Backup…").disabled(busy).on_click(move |_, window, cx| {
                let (weak_job, weak_all, id_job, id_all) = (weak_d.clone(), weak_d.clone(), id_d.clone(), id_d.clone());
                crate::dialogs::choose(
                    format!("Delete “{name_d}”?"),
                    format!("Remove only the backup job from Rile, or also delete everything it stored in {dest_d} — the backed up files and all older versions. Deleting the files can't be undone."),
                    true,
                    vec![
                        Choice::new("Remove Job Only", ButtonVariant::Secondary, move |window, cx| {
                            weak_job.update(cx, |ws, cx| ws.delete_backup(&id_job, false, window, cx)).ok();
                        }),
                        Choice::new("Delete Job and Files", ButtonVariant::Danger, move |window, cx| {
                            weak_all.update(cx, |ws, cx| ws.delete_backup(&id_all, true, window, cx)).ok();
                        }),
                    ],
                    window,
                    cx,
                );
            }))
    })
}

/// Asks where to restore to, then restores.
fn confirm_restore(weak: WeakEntity<Workspace>, id: String, from: RestoreFrom, title: String, detail: String, window: &mut Window, cx: &mut gpui_kit::App) {
    let (weak_here, id_here, from_here) = (weak.clone(), id.clone(), from.clone());
    crate::dialogs::choose(
        title,
        detail,
        false,
        vec![
            Choice::new("Restore to Folder…", ButtonVariant::Secondary, move |window, cx| {
                let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Restore Here".into()) });
                let (weak, id, from, handle) = (weak.clone(), id.clone(), from.clone(), window.window_handle());
                cx.spawn(async move |cx| {
                    if let Ok(Ok(Some(paths))) = rx.await
                        && let Some(dir) = paths.into_iter().next()
                    {
                        handle
                            .update(cx, |_, window, cx| {
                                weak.update(cx, |ws, cx| ws.restore_backup(&id, from, RestoreTo::Folder(dir), window, cx)).ok();
                            })
                            .ok();
                    }
                })
                .detach();
            }),
            Choice::new("Restore to Original Locations", ButtonVariant::Primary, move |window, cx| {
                weak_here.update(cx, |ws, cx| ws.restore_backup(&id_here, from_here.clone(), RestoreTo::Original, window, cx)).ok();
            }),
        ],
        window,
        cx,
    );
}

#[allow(clippy::too_many_arguments)]
fn run_row(card: usize, i: usize, job: &BackupJob, r: &RunRecord, busy: bool, weak: WeakEntity<Workspace>, ch: &Chrome, theme: &gpui_kit::component::Theme) -> AnyElement {
    let (ic, color) = match r.outcome {
        Outcome::Success => (IconName::CircleCheck, ch.success),
        Outcome::Partial(_) => (IconName::TriangleAlert, theme.warning),
        Outcome::Failed(_) => (IconName::CircleX, theme.danger),
        Outcome::Cancelled => (IconName::CircleX, ch.text_faint),
    };
    let trigger = match r.trigger {
        Trigger::Manual => "manual",
        Trigger::Schedule => "scheduled",
        Trigger::Change => "after changes",
        Trigger::Background => "while closed",
    };
    let group = SharedString::from(format!("backup-run-{card}-{i}"));
    let date = when(r.finished);
    let restore = r.versions.clone().map(|v| {
        let (weak, id, date) = (weak.clone(), job.id.clone(), date.clone());
        Button::new(("backup-run-restore", card * 1000 + i)).cursor_pointer().ghost().icon(IconName::ArchiveRestore).label("Restore")
            .tooltip("Restore the files this run replaced or removed")
            .pointer_unless(busy)
            .on_click(move |_, window, cx| {
                confirm_restore(weak.clone(), id.clone(), RestoreFrom::Run(v.clone()),
                    format!("Restore files from the run of {date}?"),
                    "Restores the older versions this run kept — the files it replaced or removed — as they were before it. Identical local files are skipped; others are replaced, with their current versions moved to the Trash.".into(),
                    window, cx)
            })
    });
    let delete = {
        let (weak, id, run, date) = (weak.clone(), job.id.clone(), r.clone(), date.clone());
        Button::new(("backup-run-delete", card * 1000 + i)).debug_selector(move || format!("backup-run-delete-{card}-{i}")).cursor_pointer().ghost().icon(IconName::Trash)
            .tooltip("Delete this run")
            .pointer_unless(busy)
            .on_click(move |_, window, cx| {
                let what = if run.versions.is_some() {
                    format!("Delete the run of {date}? The older file versions it kept are deleted from the backup and can't be restored anymore.")
                } else {
                    format!("Delete the run of {date} from the history?")
                };
                let (weak, id, run) = (weak.clone(), id.clone(), run.clone());
                crate::dialogs::confirm(what, "Delete Run", true, window, cx, move |window, cx| {
                    weak.update(cx, |ws, cx| ws.delete_backup_run(&id, run.clone(), window, cx)).ok();
                });
            })
    };
    h_flex()
        .id(("backup-run-row", card * 1000 + i))
        .group(group.clone())
        .gap(px(8.))
        .px(px(6.))
        .rounded(px(6.))
        .hover(|d| d.bg(ch.hover))
        .child(icon(ic).size(px(13.)).text_color(color).flex_none())
        .child(text(date, 12., ch.text).w(px(110.)).flex_none())
        .child(text(trigger, 11., ch.text_faint).w(px(90.)).flex_none())
        .child(text(describe(r), 12., ch.text_muted).flex_1().min_w_0().truncate())
        .child(h_flex().flex_none().gap_1().opacity(0.).group_hover(group, |s| s.opacity(1.)).children(restore).child(delete))
        .into_any_element()
}
