//! Undo/Redo and the searchable operation history (⌥⌘Z), on top of
//! [`rile_services::journal`].

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{WindowExt, h_flex, input::{Input, InputEvent, InputState}, notification::Notification, v_flex};
use gpui_kit::{
    AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div, prelude::FluentBuilder, px,
};
use rile_core::history::Location;
use rile_services::journal::{self, Change, OpKind, Record, State};

use super::Workspace;
use crate::actions::*;
use crate::views::kit::{self, PointerUnless, Tone};

impl Workspace {
    pub(crate) fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        match journal::undo_candidate() {
            Some(r) => self.run_journal(r.id, false, window, cx),
            None => window.push_notification(Notification::info("Nothing to undo"), cx),
        }
    }

    pub(crate) fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        match journal::redo_candidate() {
            Some(r) => self.run_journal(r.id, true, window, cx),
            None => window.push_notification(Notification::info("Nothing to redo"), cx),
        }
    }

    /// Undoes (or redoes) history record `id` as a background operation.
    pub fn run_journal(&mut self, id: u64, redo: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rec) = journal::get(id) else { return };
        let label = format!("{} “{}”", if redo { "Redoing" } else { "Undoing" }, rec.label);
        let done = format!("{} “{}”", if redo { "Redid" } else { "Undid" }, rec.label);
        self.start_op_then(
            label,
            window,
            cx,
            move |_, _| if redo { journal::redo(id) } else { journal::undo(id) },
            move |this, window, cx| {
                // Other panes may show affected folders too (and servers or
                // filtered views have no watcher).
                for pane in this.all_panes() {
                    pane.update(cx, |p, cx| p.reload(cx));
                }
                window.push_notification(Notification::success(done), cx);
            },
        );
    }

    pub(crate) fn show_history(&mut self, _: &ShowHistory, window: &mut Window, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        let view = cx.new(|cx| HistoryView::new(ws, window, cx));
        let focus = view.read(cx).search.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let undo = crate::views::chrome::shortcut("mod-z");
            kit::dialog(dialog, IconName::ClockFading, "History", Some(format!("Everything Rile changed in your files · {undo} undoes the latest").into()), 640., cx)
                .child(view.clone())
        });
        focus.update(cx, |i, cx| i.focus(window, cx));
    }
}

pub struct HistoryView {
    ws: WeakEntity<Workspace>,
    pub search: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl HistoryView {
    fn new(ws: WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search history — names, folders, actions"));
        let mut subs = vec![cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify())];
        // Re-render when an undo/redo started from here finishes.
        if let Some(ws) = ws.upgrade() {
            subs.push(cx.observe(&ws, |_, _, cx| cx.notify()));
        }
        Self { ws, search, _subs: subs }
    }

    fn row(&self, rec: &Record, first: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let ch = crate::theme::chrome(cx);
        let id = rec.id;
        let undone = rec.state == State::Undone;
        let undoable = rec.undoable();
        let reveal = rec.changes.iter().map(|c| match c {
            Change::Moved { to, .. } => to.clone(),
            other => other.path().to_path_buf(),
        }).next();
        let detail = detail(rec);
        let busy = self.ws.upgrade().is_some_and(|ws| !ws.read(cx).ops.is_empty());
        h_flex()
            .id(("history-row", id as usize))
            .px(px(12.))
            .py(px(9.))
            .gap(px(12.))
            .when(!first, |d| d.border_t_1().border_color(ch.hairline))
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .bg(if undone { ch.hover } else { ch.accent_soft })
                    .child(crate::icons::icon(kind_icon(rec.kind)).size(px(14.)).text_color(if undone { ch.text_faint } else { ch.accent })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if undone { ch.text_muted } else { ch.text_strong })
                                    .when(undone, |d| d.line_through())
                                    .child(rec.label.clone()),
                            )
                            .when(undone, |d| d.child(pill("Undone", cx))),
                    )
                    .child(div().min_w_0().truncate().text_size(px(11.)).text_color(ch.text_faint).child(format!("{} · {detail}", ago(rec.time)))),
            )
            .when_some(reveal, |d, path| {
                d.child(
                    div()
                        .id(("history-reveal", id as usize))
                        .size(px(26.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(ch.hover_strong))
                        .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Show in folder").build(w, cx))
                        .child(crate::icons::icon(IconName::FolderOpen).size(px(14.)).text_color(ch.text_muted))
                        .on_click(cx.listener(move |this, _, window, cx| this.reveal(path.clone(), window, cx))),
                )
            })
            .when(undoable, |d| {
                d.child(
                    kit::button(("history-act", id as usize), if undone { "Redo" } else { "Undo" }, Tone::Secondary, true, cx)
                        .pointer_unless(busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(ws) = this.ws.upgrade() {
                                ws.update(cx, |ws, cx| ws.run_journal(id, undone, window, cx));
                            }
                        })),
                )
            })
    }

    fn reveal(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        window.close_dialog(cx);
        if path.exists() {
            ws.update(cx, |ws, cx| ws.reveal_in_pane(path, window, cx));
        } else if let Some(dir) = path.parent().filter(|d| d.is_dir()).map(PathBuf::from) {
            ws.update(cx, |ws, cx| ws.open_location(Location::Dir(dir), window, cx));
        }
    }
}

impl Render for HistoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ch = crate::theme::chrome(cx);
        let query = self.search.read(cx).value().to_string();
        let all = journal::list();
        let total = all.len();
        let records: Vec<Record> = all.into_iter().filter(|r| r.matches(&query)).collect();
        let shown = records.len();
        let rows: Vec<_> = records.iter().enumerate().map(|(i, r)| self.row(r, i == 0, cx).into_any_element()).collect();
        let body = if rows.is_empty() {
            v_flex()
                .py(px(36.))
                .items_center()
                .gap(px(6.))
                .child(crate::icons::icon(IconName::ClockFading).size(px(22.)).text_color(ch.text_faint))
                .child(div().text_size(px(12.)).text_color(ch.text_muted).child(if total == 0 {
                    "Copies, moves, renames and deletions show up here."
                } else {
                    "Nothing in the history matches."
                }))
                .into_any_element()
        } else {
            kit::card(cx)
                .id("history-list")
                .max_h(px(420.))
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };
        v_flex()
            .gap(px(12.))
            .child(Input::new(&self.search).prefix(crate::icons::icon(IconName::Search).size(px(14.)).text_color(ch.text_faint)))
            .child(body)
            .child(
                h_flex()
                    .justify_between()
                    .child(kit::hint(
                        if query.trim().is_empty() { operations(total) } else { format!("{shown} of {}", operations(total)) },
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .when(total > 0, |d| {
                                d.child(kit::button("history-clear", "Clear History", Tone::Ghost, true, cx).on_click(cx.listener(|_, _, _, cx| {
                                    journal::clear();
                                    cx.notify();
                                })))
                            })
                            .child(kit::button("history-done", "Done", Tone::Primary, true, cx).on_click(|_, window, cx| window.close_dialog(cx))),
                    ),
            )
    }
}

fn operations(n: usize) -> String {
    if n == 1 { "1 operation".into() } else { format!("{n} operations") }
}

fn pill(text: &'static str, cx: &gpui_kit::App) -> gpui_kit::Div {
    let ch = crate::theme::chrome(cx);
    div().flex_none().px(px(6.)).py(px(1.)).rounded(px(4.)).bg(ch.hover_strong).text_size(px(10.)).text_color(ch.text_muted).child(text)
}

pub fn kind_icon(kind: OpKind) -> IconName {
    match kind {
        OpKind::Copy => IconName::Copy,
        OpKind::Move => IconName::FolderInput,
        OpKind::Rename | OpKind::BatchRename => IconName::Pencil,
        OpKind::Duplicate => IconName::Files,
        OpKind::NewFolder => IconName::FolderPlus,
        OpKind::NewFile => IconName::FilePlus,
        OpKind::Trash => IconName::Trash,
        OpKind::Delete => IconName::CircleX,
        OpKind::Compress => IconName::Archive,
        OpKind::Extract => IconName::PackageOpen,
        OpKind::Sync => IconName::FolderSync,
        OpKind::Other => IconName::ClockFading,
    }
}

/// Where the record happened: the folder of its items, with a count.
fn detail(rec: &Record) -> SharedString {
    let folder = |p: &std::path::Path| p.parent().map(super::display_name).unwrap_or_default();
    let first = rec.changes.first();
    let place = match first {
        Some(Change::Moved { from, to }) if from.parent() != to.parent() => format!("{} → {}", folder(from), folder(to)),
        Some(c) => format!("in {}", folder(c.path())),
        None => String::new(),
    };
    let names: Vec<String> = rec.changes.iter().take(3).map(|c| super::display_name(c.path())).collect();
    let more = rec.changes.len().saturating_sub(names.len());
    let mut s = names.join(", ");
    if more > 0 {
        s.push_str(&format!(" +{more}"));
    }
    if !place.is_empty() {
        s.push_str(&format!(" · {place}"));
    }
    s.into()
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "12 Mar".
pub fn ago(time: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let secs = (now - time).max(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => chrono::DateTime::from_timestamp(time, 0)
            .map(|d| d.with_timezone(&chrono::Local).format("%-d %b %Y, %H:%M").to_string())
            .unwrap_or_default(),
    }
}
