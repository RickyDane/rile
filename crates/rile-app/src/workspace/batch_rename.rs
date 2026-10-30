//! Batch rename: find & replace (plain or regex), a name template with a
//! counter, and case changes, with a live preview of every new name.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    WindowExt, checkbox::Checkbox, h_flex,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::{
    AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div, prelude::FluentBuilder, px, rgba,
};
use rile_services::batch_rename::{self, CaseChange, Preview, Rules, Status};
use rile_services::journal::{self, OpKind};

use super::{Workspace, item_count};
use crate::actions::*;
use crate::views::kit::{self, PointerUnless, Tone};

impl Workspace {
    pub(crate) fn batch_rename(&mut self, _: &BatchRename, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane().read(cx).conn().is_some() {
            window.push_notification(Notification::info("Batch rename isn't available on servers"), cx);
            return;
        }
        let paths = self.selection(cx);
        if paths.is_empty() {
            window.push_notification(Notification::info("Select the items to rename first"), cx);
            return;
        }
        let ws = cx.entity().downgrade();
        let n = paths.len();
        let view = cx.new(|cx| BatchRenameForm::new(ws, paths, window, cx));
        let focus = view.read(cx).find.clone();
        let focus_pane = self.pane().read(cx).focus_handle.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let focus_pane = focus_pane.clone();
            kit::dialog(dialog, IconName::Replace, "Rename Items", Some(item_count(n).into()), 760., cx)
                .child(view.clone())
                .on_close(move |_, window, cx| focus_pane.focus(window, cx))
        });
        focus.update(cx, |i, cx| i.focus(window, cx));
    }

    /// Renames `pairs` in one go (undoable as one step).
    pub fn apply_batch_rename(&mut self, pairs: Vec<(PathBuf, PathBuf)>, window: &mut Window, cx: &mut Context<Self>) {
        let label = format!("Renaming {}", item_count(pairs.len()));
        self.start_op(label, window, cx, move |_, _| {
            let changes = journal::rename_all(&pairs)?;
            let n = changes.len();
            journal::record(OpKind::BatchRename, format!("Renamed {}", item_count(n)), changes);
            Ok(pairs.into_iter().map(|(_, to)| to).collect())
        });
    }
}

pub struct BatchRenameForm {
    ws: WeakEntity<Workspace>,
    paths: Vec<PathBuf>,
    pub find: Entity<InputState>,
    replace: Entity<InputState>,
    template: Entity<InputState>,
    start: Entity<InputState>,
    pad: Entity<InputState>,
    rules: Rules,
    preview: Result<Vec<Preview>, String>,
    _subs: Vec<Subscription>,
}

impl BatchRenameForm {
    fn new(ws: WeakEntity<Workspace>, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rules = Rules::default();
        let input = |placeholder: &str, value: String, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = SharedString::from(placeholder.to_string());
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
        };
        let find = input("Text to find", String::new(), window, cx);
        let replace = input("Replace with", String::new(), window, cx);
        let template = input("{name}", rules.template.clone(), window, cx);
        let start = input("1", rules.start.to_string(), window, cx);
        let pad = input("1", rules.pad.to_string(), window, cx);
        let subs = [&find, &replace, &template, &start, &pad]
            .into_iter()
            .map(|i| {
                cx.subscribe(i, |this, _, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.update_rules(cx);
                    }
                })
            })
            .collect();
        let preview = batch_rename::preview(&paths, &rules);
        Self { ws, paths, find, replace, template, start, pad, rules, preview, _subs: subs }
    }

    fn update_rules(&mut self, cx: &mut Context<Self>) {
        let text = |e: &Entity<InputState>, cx: &Context<Self>| e.read(cx).value().to_string();
        self.rules.find = text(&self.find, cx);
        self.rules.replace = text(&self.replace, cx);
        self.rules.template = text(&self.template, cx);
        self.rules.start = text(&self.start, cx).trim().parse().unwrap_or(1);
        self.rules.pad = text(&self.pad, cx).trim().parse::<usize>().unwrap_or(1).clamp(1, 12);
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.preview = batch_rename::preview(&self.paths, &self.rules);
        cx.notify();
    }

    fn insert_token(&mut self, token: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let mut value = self.template.read(cx).value().to_string();
        value.push_str(token);
        self.template.update(cx, |i, cx| {
            i.set_value(value, window, cx);
            i.focus(window, cx);
        });
        self.update_rules(cx);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(preview) = &self.preview else { return };
        if preview.iter().any(|p| matches!(p.status, Status::Conflict | Status::Invalid)) {
            return;
        }
        let pairs: Vec<(PathBuf, PathBuf)> = preview.iter().filter(|p| p.status == Status::Ok).map(|p| (p.from.clone(), p.to())).collect();
        if pairs.is_empty() {
            return;
        }
        window.close_dialog(cx);
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |ws, cx| ws.apply_batch_rename(pairs, window, cx));
        }
    }
}

impl Render for BatchRenameForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ch = crate::theme::chrome(cx);
        let danger: gpui_kit::Hsla = rgba(0xf07178ff).into();
        let check = |id: &'static str, label: &'static str, value: bool, set: fn(&mut Rules, bool), cx: &mut Context<Self>| {
            Checkbox::new(id).cursor_pointer().label(label).checked(value).on_click(cx.listener(move |this, v: &bool, _, cx| {
                set(&mut this.rules, *v);
                this.refresh(cx);
            }))
        };
        let tokens = h_flex().gap(px(4.)).flex_wrap().children(
            [("{name}", "Name"), ("{n}", "Counter"), ("{ext}", "Extension"), ("{date}", "Date modified"), ("{parent}", "Folder")]
                .into_iter()
                .map(|(token, tip)| {
                    div()
                        .id(token)
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(ch.hairline)
                        .bg(ch.hover)
                        .text_size(px(11.))
                        .font_family("monospace")
                        .text_color(ch.text)
                        .cursor_pointer()
                        .hover(|s| s.bg(ch.hover_strong))
                        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
                        .child(token)
                        .on_click(cx.listener(move |this, _, window, cx| this.insert_token(token, window, cx)))
                }),
        );
        let cases = kit::segmented(cx).children(CaseChange::ALL.into_iter().enumerate().map(|(i, c)| {
            kit::segment(("case", i), c.label(), self.rules.case == c, cx).on_click(cx.listener(move |this, _, _, cx| {
                this.rules.case = c;
                this.refresh(cx);
            }))
        }));

        let (rows, summary, error, blocked, count) = match &self.preview {
            Err(e) => (Vec::new(), String::new(), Some(format!("Pattern error: {e}")), true, 0),
            Ok(preview) => {
                let changed = preview.iter().filter(|p| p.status == Status::Ok).count();
                let conflicts = preview.iter().filter(|p| p.status == Status::Conflict).count();
                let invalid = preview.iter().filter(|p| p.status == Status::Invalid).count();
                let mut summary = format!("{changed} of {} will be renamed", preview.len());
                if conflicts > 0 {
                    summary.push_str(&format!(" · {conflicts} name{} taken", if conflicts == 1 { "" } else { "s" }));
                }
                if invalid > 0 {
                    summary.push_str(&format!(" · {invalid} invalid"));
                }
                let rows = preview
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let (color, note) = match p.status {
                            Status::Unchanged => (ch.text_faint, None),
                            Status::Ok => (ch.text_strong, None),
                            Status::Conflict => (danger, Some("Name taken")),
                            Status::Invalid => (danger, Some("Invalid name")),
                        };
                        h_flex()
                            .px(px(12.))
                            .py(px(6.))
                            .gap(px(10.))
                            .when(i > 0, |d| d.border_t_1().border_color(ch.hairline))
                            .text_size(px(12.))
                            .child(div().flex_1().min_w_0().truncate().text_color(ch.text_muted).child(p.old_name.clone()))
                            .child(crate::icons::icon(IconName::ArrowRight).size(px(12.)).text_color(ch.text_faint).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(color)
                                    .when(p.status == Status::Ok, |d| d.font_weight(FontWeight::MEDIUM))
                                    .child(if p.new_name.is_empty() { "(empty)".to_string() } else { p.new_name.clone() }),
                            )
                            .children(note.map(|n| div().flex_none().text_size(px(10.)).text_color(danger).child(n)))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();
                (rows, summary, None, conflicts + invalid > 0 || changed == 0, changed)
            }
        };

        v_flex()
            .gap(px(14.))
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(div().flex_1().child(kit::field("Find", Input::new(&self.find), cx)))
                    .child(div().flex_1().child(kit::field("Replace with", Input::new(&self.replace), cx))),
            )
            .child(
                h_flex()
                    .gap(px(16.))
                    .child(check("br-regex", "Regular expression", self.rules.regex, |r, v| r.regex = v, cx))
                    .child(check("br-case", "Match case", self.rules.case_sensitive, |r, v| r.case_sensitive = v, cx))
                    .child(check("br-ext", "Include extension", self.rules.include_ext, |r, v| r.include_ext = v, cx)),
            )
            .child(
                h_flex()
                    .gap(px(10.))
                    .items_end()
                    .child(div().flex_1().child(kit::field("Name template", Input::new(&self.template), cx)))
                    .child(div().w(px(80.)).child(kit::field("Start at", Input::new(&self.start), cx)))
                    .child(div().w(px(80.)).child(kit::field("Digits", Input::new(&self.pad), cx))),
            )
            .child(h_flex().gap(px(8.)).child(kit::hint("Insert:", cx)).child(tokens))
            .child(v_flex().gap(px(6.)).child(kit::caption("CASE", cx)).child(cases))
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(h_flex().justify_between().child(kit::caption("PREVIEW", cx)).child(kit::hint(summary, cx)))
                    .child(kit::card(cx).id("br-preview").max_h(px(260.)).overflow_y_scroll().children(rows)),
            )
            .children(error.map(|e| kit::notice(e, true, cx)))
            .child(
                kit::footer()
                    .child(kit::button("br-cancel", "Cancel", Tone::Ghost, false, cx).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        kit::button("br-go", if count > 0 { format!("Rename {}", item_count(count)) } else { "Rename".into() }, Tone::Primary, false, cx)
                            .debug_selector(|| "br-go".into())
                            .pointer_unless(blocked)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
