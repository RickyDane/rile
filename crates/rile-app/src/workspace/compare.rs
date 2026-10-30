//! Folder comparison and synchronization dialog.
//!
//! Compares two folders (the two panes, two selected folders, or the
//! current folder and a picked one), lists every difference with what a
//! sync would do with it, lets each action be changed by clicking it, then
//! synchronizes as one undoable operation (replaced and removed items go to
//! the trash).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::assets::IconName;
use gpui_kit::component::{WindowExt, checkbox::Checkbox, h_flex, notification::Notification, spinner::Spinner, v_flex};
use gpui_kit::{
    AppContext, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Task, WeakEntity, Window, div, prelude::FluentBuilder, px, rgba, uniform_list,
};
use rile_services::compare::{self, Action, Diff, Item, Method, Mode, Options};
use rile_services::journal::{self, OpKind};

use super::{Workspace, display_name, item_count};
use crate::actions::*;
use crate::views::kit::{self, PointerUnless, Tone};

impl Workspace {
    pub(crate) fn compare_folders(&mut self, _: &CompareFolders, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.pane().read(cx);
        if pane.conn().is_some() {
            window.push_notification(Notification::info("Comparing works with folders on this computer"), cx);
            return;
        }
        let selected: Vec<PathBuf> = pane.selected_entries().into_iter().filter(|e| e.is_dir).map(|e| e.path.clone()).collect();
        let current = pane.current_dir().map(Path::to_path_buf);
        let other = self.other_dir(cx).filter(|(_, conn)| conn.is_none()).map(|(d, _)| d);
        let pair = match (selected.as_slice(), current, other) {
            ([a, b], _, _) => Some((a.clone(), b.clone())),
            ([], Some(here), Some(there)) if here != there => {
                // Keep the panes' left/right order.
                Some(if self.active_pane == 0 { (here, there) } else { (there, here) })
            }
            ([one], _, _) => return self.pick_and_compare(one.clone(), window, cx),
            (_, Some(here), _) => return self.pick_and_compare(here, window, cx),
            _ => None,
        };
        match pair {
            Some((left, right)) => self.open_compare(left, right, window, cx),
            None => window.push_notification(Notification::info("Open a folder, or select two folders to compare"), cx),
        }
    }

    /// Asks for the folder to compare `left` with.
    fn pick_and_compare(&mut self, left: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_compare_root(left, window, cx);
    }

    pub fn open_compare(&mut self, left: PathBuf, right: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        let view = cx.new(|cx| CompareView::new(ws, left, right, cx));
        window.open_dialog(cx, move |dialog, _, cx| {
            kit::dialog(dialog, IconName::GitCompareArrows, "Compare & Sync Folders", None, 940., cx).child(view.clone())
        });
    }

    fn start_sync(&mut self, view: WeakEntity<CompareView>, left: PathBuf, right: PathBuf, actions: Vec<(Item, Action)>, window: &mut Window, cx: &mut Context<Self>) {
        let label = format!("Synchronizing {} and {}", display_name(&left), display_name(&right));
        let past = format!("Synchronized {} and {}", display_name(&left), display_name(&right));
        self.start_op_then(
            label,
            window,
            cx,
            move |cancel, report| {
                let (changes, result) = compare::sync(&left, &right, &actions, cancel, report);
                journal::record(OpKind::Sync, past, changes);
                result.map(|_| Vec::new())
            },
            move |this, window, cx| {
                for pane in this.all_panes() {
                    pane.update(cx, |p, cx| p.reload(cx));
                }
                window.push_notification(Notification::success("Folders synchronized"), cx);
                view.update(cx, |v, cx| v.run(cx)).ok();
            },
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    Differences,
    All,
    LeftOnly,
    RightOnly,
    Changed,
}

pub struct CompareView {
    ws: WeakEntity<Workspace>,
    left: PathBuf,
    right: PathBuf,
    options: Options,
    mode: Mode,
    filter: Filter,
    items: Vec<(Item, Action)>,
    /// Indices into `items` that pass the filter.
    shown: Arc<Vec<usize>>,
    scanning: Option<Task<()>>,
    scanned: usize,
    error: Option<SharedString>,
    cancel: Arc<AtomicBool>,
}

impl CompareView {
    fn new(ws: WeakEntity<Workspace>, left: PathBuf, right: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            ws,
            left,
            right,
            options: Options::default(),
            mode: Mode::default(),
            filter: Filter::Differences,
            items: Vec::new(),
            shown: Arc::default(),
            scanning: None,
            scanned: 0,
            error: None,
            cancel: Arc::default(),
        };
        this.run(cx);
        this
    }

    /// (Re)compares in the background.
    fn run(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.error = None;
        self.scanned = 0;
        let (left, right, options) = (self.left.clone(), self.right.clone(), self.options);
        let (tx, rx) = smol::channel::unbounded::<usize>();
        let job = cx.background_spawn(async move { compare::compare(&left, &right, options, &cancel, |n| { tx.try_send(n).ok(); }) });
        self.scanning = Some(cx.spawn(async move |this, cx| {
            while let Ok(n) = rx.recv().await {
                if this.update(cx, |v, cx| { v.scanned = n; cx.notify(); }).is_err() {
                    return;
                }
            }
            let result = job.await;
            this.update(cx, |v, cx| {
                v.scanning = None;
                match result {
                    Ok(items) => {
                        let mode = v.mode;
                        v.items = items.into_iter().map(|i| { let a = compare::plan(&i, mode); (i, a) }).collect();
                        v.refilter();
                    }
                    Err(e) if e.to_string() == "Cancelled" => {}
                    Err(e) => v.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn refilter(&mut self) {
        let f = self.filter;
        self.shown = Arc::new(
            self.items
                .iter()
                .enumerate()
                .filter(|(_, (i, _))| match f {
                    Filter::All => true,
                    Filter::Differences => i.diff != Diff::Same,
                    Filter::LeftOnly => i.diff == Diff::LeftOnly,
                    Filter::RightOnly => i.diff == Diff::RightOnly,
                    Filter::Changed => matches!(i.diff, Diff::LeftNewer | Diff::RightNewer | Diff::Different | Diff::Conflict),
                })
                .map(|(ix, _)| ix)
                .collect(),
        );
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        for (item, action) in &mut self.items {
            *action = compare::plan(item, mode);
        }
        cx.notify();
    }

    fn cycle(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some((item, action)) = self.items.get_mut(ix) {
            let choices = Action::choices(item);
            let pos = choices.iter().position(|a| a == action).unwrap_or(0);
            *action = choices[(pos + 1) % choices.len()];
            cx.notify();
        }
    }

    fn swap(&mut self, cx: &mut Context<Self>) {
        std::mem::swap(&mut self.left, &mut self.right);
        self.run(cx);
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let actions: Vec<(Item, Action)> = self.items.iter().filter(|(_, a)| *a != Action::Skip).cloned().collect();
        if actions.is_empty() {
            return;
        }
        let deletes = actions.iter().filter(|(_, a)| matches!(a, Action::DeleteLeft | Action::DeleteRight)).count();
        let replaces = actions
            .iter()
            .filter(|(i, a)| matches!((a, i.left, i.right), (Action::CopyToRight, _, Some(_)) | (Action::CopyToLeft, Some(_), _)))
            .count();
        let Some(ws) = self.ws.upgrade() else { return };
        let view = cx.entity().downgrade();
        let (left, right) = (self.left.clone(), self.right.clone());
        let go = move |window: &mut Window, cx: &mut gpui_kit::App| {
            let (view, left, right, actions) = (view.clone(), left.clone(), right.clone(), actions.clone());
            ws.update(cx, |ws, cx| ws.start_sync(view, left, right, actions, window, cx));
        };
        if deletes + replaces > 0 {
            let trash = crate::platform_text::trash();
            let mut what = Vec::new();
            if replaces > 0 {
                what.push(format!("replace {}", item_count(replaces)));
            }
            if deletes > 0 {
                what.push(format!("remove {}", item_count(deletes)));
            }
            crate::dialogs::confirm(
                format!("Synchronizing will {}. Replaced and removed items go to the {trash}, and History can undo the whole sync.", what.join(" and ")),
                "Synchronize",
                false,
                window,
                cx,
                go,
            );
        } else {
            go(window, cx);
        }
    }

    fn row(&self, ix: usize, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let ch = crate::theme::chrome(cx);
        let (item, action) = &self.items[ix];
        let side = |m: Option<compare::Meta>, newer: bool| {
            let text = match m {
                None => "—".to_string(),
                Some(m) if m.is_dir => "Folder".to_string(),
                Some(m) => format!("{} · {}", humansize::format_size(m.size, humansize::DECIMAL), date(m.modified)),
            };
            div()
                .w(px(170.))
                .flex_none()
                .truncate()
                .text_size(px(11.))
                .text_color(if m.is_none() { ch.text_faint } else if newer { ch.text_strong } else { ch.text_muted })
                .when(newer, |d| d.font_weight(FontWeight::MEDIUM))
                .child(text)
        };
        let danger: gpui_kit::Hsla = rgba(0xf07178ff).into();
        let (badge_color, badge_bg) = match item.diff {
            Diff::Same => (ch.text_faint, ch.hover),
            Diff::LeftOnly | Diff::RightOnly => (ch.accent, ch.accent_soft),
            Diff::Conflict | Diff::Different => (danger, danger.opacity(0.12)),
            Diff::LeftNewer | Diff::RightNewer => (ch.text, ch.hover_strong),
        };
        let (act_icon, act_color) = match action {
            Action::Skip => (IconName::Minus, ch.text_faint),
            Action::CopyToRight => (IconName::ArrowRight, ch.accent),
            Action::CopyToLeft => (IconName::ArrowLeft, ch.accent),
            Action::DeleteLeft | Action::DeleteRight => (IconName::Trash, danger),
        };
        let name = item.rel.display().to_string();
        h_flex()
            .id(("cmp-row", ix))
            .w_full()
            .h(px(32.))
            .px(px(10.))
            .gap(px(10.))
            .border_b_1()
            .border_color(ch.hairline)
            .child(crate::icons::icon(if item.is_dir() { IconName::Folder } else { crate::icons::icon_for_path(&item.rel) }).size(px(14.)).text_color(ch.text_muted).flex_none())
            .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(ch.text).child(name))
            .child(side(item.left, item.diff == Diff::LeftNewer))
            .child(
                div()
                    .w(px(84.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(div().px(px(6.)).py(px(1.)).rounded(px(4.)).bg(badge_bg).text_size(px(10.)).text_color(badge_color).child(item.diff.label())),
            )
            .child(
                h_flex()
                    .id(("cmp-act", ix))
                    .w(px(108.))
                    .flex_none()
                    .h(px(24.))
                    .px(px(8.))
                    .gap(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(ch.hairline)
                    .cursor_pointer()
                    .hover(|s| s.bg(ch.hover_strong))
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Click to change what happens").build(w, cx))
                    .child(crate::icons::icon(act_icon).size(px(12.)).text_color(act_color))
                    .child(div().truncate().text_size(px(11.)).text_color(if *action == Action::Skip { ch.text_faint } else { ch.text }).child(action.label()))
                    .on_click(cx.listener(move |this, _, _, cx| this.cycle(ix, cx))),
            )
            .child(side(item.right, item.diff == Diff::RightNewer))
            .into_any_element()
    }
}

fn date(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.with_timezone(&chrono::Local).format("%-d %b %Y %H:%M").to_string()).unwrap_or_default()
}

impl Render for CompareView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ch = crate::theme::chrome(cx);
        let count = |d: &[Diff]| self.items.iter().filter(|(i, _)| d.contains(&i.diff)).count();
        let (only_l, only_r) = (count(&[Diff::LeftOnly]), count(&[Diff::RightOnly]));
        let changed = count(&[Diff::LeftNewer, Diff::RightNewer, Diff::Different, Diff::Conflict]);
        let same = count(&[Diff::Same]);
        let differences = only_l + only_r + changed;
        let scanning = self.scanning.is_some();

        let path_box = |path: &Path| {
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(px(8.))
                .px(px(10.))
                .h(px(34.))
                .rounded(px(8.))
                .border_1()
                .border_color(ch.hairline)
                .bg(ch.hover)
                .child(crate::icons::icon(IconName::Folder).size(px(15.)).text_color(ch.accent).flex_none())
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().truncate().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(ch.text_strong).child(display_name(path)))
                        .child(div().truncate().text_size(px(10.)).text_color(ch.text_faint).child(path.parent().map(|p| p.display().to_string()).unwrap_or_default())),
                )
                .into_any_element()
        };
        let left_box = path_box(&self.left);
        let right_box = path_box(&self.right);
        let header = h_flex()
            .gap(px(8.))
            .child(left_box)
            .child(
                div()
                    .id("cmp-swap")
                    .size(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .cursor_pointer()
                    .hover(|s| s.bg(ch.hover_strong))
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Swap sides").build(w, cx))
                    .child(crate::icons::icon(IconName::ArrowRightLeft).size(px(15.)).text_color(ch.text_muted))
                    .on_click(cx.listener(|this, _, _, cx| this.swap(cx))),
            )
            .child(right_box);

        let method = kit::segmented(cx)
            .child(kit::segment("cmp-m-date", "Size & date", self.options.method == Method::SizeAndDate, cx).on_click(cx.listener(|this, _, _, cx| {
                this.options.method = Method::SizeAndDate;
                this.run(cx);
            })))
            .child(kit::segment("cmp-m-content", "Content", self.options.method == Method::Content, cx).on_click(cx.listener(|this, _, _, cx| {
                this.options.method = Method::Content;
                this.run(cx);
            })));
        let hidden = Checkbox::new("cmp-hidden").cursor_pointer().label("Hidden files").checked(self.options.include_hidden).on_click(cx.listener(|this, v: &bool, _, cx| {
            this.options.include_hidden = *v;
            this.run(cx);
        }));
        let modes = kit::segmented(cx).children(Mode::ALL.into_iter().enumerate().map(|(i, m)| {
            kit::segment(("cmp-mode", i), m.label(), self.mode == m, cx).on_click(cx.listener(move |this, _, _, cx| this.set_mode(m, cx)))
        }));
        let filters = h_flex().gap(px(6.)).children(
            [
                (Filter::Differences, format!("Differences {differences}")),
                (Filter::LeftOnly, format!("Only left {only_l}")),
                (Filter::RightOnly, format!("Only right {only_r}")),
                (Filter::Changed, format!("Changed {changed}")),
                (Filter::All, format!("All {}", self.items.len())),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (f, label))| {
                let active = self.filter == f;
                div()
                    .id(("cmp-filter", i))
                    .px(px(9.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(if active { ch.accent.opacity(0.5) } else { ch.hairline })
                    .bg(if active { ch.accent_soft } else { gpui_kit::transparent_black() })
                    .text_size(px(11.))
                    .text_color(if active { ch.text_strong } else { ch.text_muted })
                    .cursor_pointer()
                    .hover(|s| s.bg(ch.hover_strong))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = f;
                        this.refilter();
                        cx.notify();
                    }))
            }),
        );

        let shown = self.shown.clone();
        let list: gpui_kit::AnyElement = if scanning {
            v_flex()
                .h_full()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(Spinner::new())
                .child(kit::hint(format!("Comparing… {} items looked at", self.scanned), cx))
                .into_any_element()
        } else if let Some(e) = &self.error {
            div().p(px(12.)).child(kit::notice(e.clone(), true, cx)).into_any_element()
        } else if shown.is_empty() {
            v_flex()
                .h_full()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .child(crate::icons::icon(IconName::Equal).size(px(22.)).text_color(ch.success))
                .child(div().text_size(px(13.)).text_color(ch.text).child(if differences == 0 { "The folders are identical" } else { "Nothing in this filter" }))
                .child(kit::hint(format!("{} identical", item_count(same)), cx))
                .into_any_element()
        } else {
            let head = |text: &'static str, w: Option<f32>| {
                div()
                    .when_some(w, |d, w| d.w(px(w)).flex_none())
                    .when(w.is_none(), |d| d.flex_1().min_w_0())
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ch.text_faint)
                    .child(text)
            };
            v_flex()
                .size_full()
                .child(
                    h_flex()
                        .h(px(28.))
                        .px(px(10.))
                        .gap(px(10.))
                        .flex_none()
                        .border_b_1()
                        .border_color(ch.hairline)
                        .child(div().w(px(14.)).flex_none())
                        .child(head("NAME", None))
                        .child(head("LEFT", Some(170.)))
                        .child(head("", Some(84.)))
                        .child(head("ACTION", Some(108.)))
                        .child(head("RIGHT", Some(170.))),
                )
                .child(
                    uniform_list("cmp-list", shown.len(), cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|i| this.row(shown[i], cx)).collect()
                    }))
                    .flex_1()
                    .w_full(),
                )
                .into_any_element()
        };

        let planned: Vec<&(Item, Action)> = self.items.iter().filter(|(_, a)| *a != Action::Skip).collect();
        let copies = planned.iter().filter(|(_, a)| matches!(a, Action::CopyToLeft | Action::CopyToRight)).count();
        let deletes = planned.len() - copies;
        let bytes = compare::bytes_to_copy(&self.items);
        let mut summary = if planned.is_empty() { "Nothing to do".to_string() } else { format!("{} to copy ({})", item_count(copies), humansize::format_size(bytes, humansize::DECIMAL)) };
        if deletes > 0 {
            summary.push_str(&format!(" · {} to the {}", item_count(deletes), crate::platform_text::trash()));
        }
        let busy = self.ws.upgrade().is_some_and(|ws| !ws.read(cx).ops.is_empty());

        v_flex()
            .gap(px(12.))
            .child(header)
            .child(
                h_flex()
                    .justify_between()
                    .gap(px(12.))
                    .child(h_flex().gap(px(12.)).child(kit::caption("COMPARE BY", cx)).child(method).child(hidden))
                    .child(
                        div()
                            .id("cmp-refresh")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(7.))
                            .cursor_pointer()
                            .hover(|s| s.bg(ch.hover_strong))
                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Compare again").build(w, cx))
                            .child(crate::icons::icon(IconName::RotateCw).size(px(14.)).text_color(ch.text_muted))
                            .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                    ),
            )
            .child(v_flex().gap(px(6.)).child(h_flex().gap(px(12.)).child(kit::caption("SYNC", cx)).child(modes)).child(kit::hint(self.mode.description(), cx)))
            .child(filters)
            .child(kit::card(cx).h(px(340.)).overflow_hidden().child(list))
            .child(
                h_flex()
                    .justify_between()
                    .child(kit::hint(summary, cx))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(kit::button("cmp-close", "Close", Tone::Ghost, false, cx).on_click(|_, window, cx| window.close_dialog(cx)))
                            .child(
                                kit::button("cmp-sync", "Synchronize", Tone::Primary, false, cx)
                                    .debug_selector(|| "cmp-sync".into())
                                    .pointer_unless(planned.is_empty() || scanning || busy)
                                    .on_click(cx.listener(|this, _, window, cx| this.sync(window, cx))),
                            ),
                    ),
            )
    }
}
