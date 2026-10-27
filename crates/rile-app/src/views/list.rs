use std::ops::Range;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, uniform_list,
};
use rile_core::{Entry, SortKey, SortOrder};

use super::{empty_message, entry_interactions, placeholder, prefs};
use crate::icons::entry_icon;
use crate::pane::Pane;
use crate::theme::chrome;

const COL_MODIFIED: f32 = 170.;
const COL_SIZE: f32 = 90.;
const COL_KIND: f32 = 130.;
const COL_LOCATION: f32 = 220.;

/// Horizontal inset of the rows (the file area's padding).
const INSET: f32 = 19.;
/// Vertical gap between rows.
const ROW_GAP: f32 = 2.;

/// Distance between consecutive rows (row height plus gap).
pub fn row_pitch(cx: &gpui_kit::App) -> f32 {
    prefs(cx).row_height + 2. + ROW_GAP
}

/// Optional columns that fit the pane's width: (modified, kind). Search
/// results also show a Location column, which needs its own room; the Name
/// column always keeps at least `MIN_NAME`.
fn visible_columns(pane: &Pane) -> (bool, bool) {
    const MIN_NAME: f32 = 200.;
    let mut room = pane.viewport_width - 2. * INSET - 18. - MIN_NAME - COL_SIZE;
    if pane.search.is_some() {
        room -= COL_LOCATION;
    }
    let show_mod = room >= COL_MODIFIED;
    if show_mod {
        room -= COL_MODIFIED;
    }
    (show_mod, room >= COL_KIND)
}

pub fn render(pane: &mut Pane, _window: &mut Window, cx: &mut Context<Pane>) -> AnyElement {
    let body: AnyElement = match empty_message(pane) {
        Some(msg) => placeholder(msg, cx),
        None => uniform_list("file-list", pane.entries().len(), cx.processor(render_rows))
            .track_scroll(&pane.scroll)
            .size_full()
            .into_any_element(),
    };
    v_flex()
        .size_full()
        .child(render_header(pane, cx))
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

fn render_header(pane: &Pane, cx: &mut Context<Pane>) -> impl IntoElement {
    let ch = chrome(cx);
    let searching = pane.search.is_some();
    let (show_mod, show_kind) = visible_columns(pane);
    let column = |id: &'static str, label: &'static str, key: SortKey, cx: &mut Context<Pane>| {
        let active = pane.sort_key == key;
        let arrow = match pane.sort_order {
            SortOrder::Ascending => " ↑",
            SortOrder::Descending => " ↓",
        };
        div()
            .id(id)
            .h_full()
            .flex()
            .items_center()
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .hover(|s| s.text_color(ch.text))
            .when(active, |d| d.font_weight(FontWeight::MEDIUM).text_color(ch.text))
            .child(format!("{label}{}", if active { arrow } else { "" }))
            .on_click(cx.listener(move |this, _, _, cx| this.set_sort(key, cx)))
    };
    h_flex()
        .h(px(34.))
        .mx(px(INSET))
        .px(px(9.))
        .gap_2()
        .flex_none()
        .border_b_1()
        .border_color(ch.hairline)
        .text_size(px(10.))
        .text_color(ch.list_header)
        .child(div().flex_1().h_full().child(column("col-name", "Name", SortKey::Name, cx)))
        .when(searching, |d| d.child(div().w(px(COL_LOCATION)).h_full().flex().items_center().child("Location")))
        .when(show_mod, |d| d.child(div().w(px(COL_MODIFIED)).h_full().child(column("col-mod", "Date Modified", SortKey::Modified, cx))))
        .child(div().w(px(COL_SIZE)).h_full().flex().justify_end().child(column("col-size", "Size", SortKey::Size, cx)))
        .when(show_kind, |d| d.child(div().w(px(COL_KIND)).h_full().child(column("col-kind", "Kind", SortKey::Kind, cx))))
}

fn render_rows(pane: &mut Pane, range: Range<usize>, _window: &mut Window, cx: &mut Context<Pane>) -> Vec<AnyElement> {
    let ch = chrome(cx);
    let now = Local::now();
    let row_height = prefs(cx).row_height + 2.;
    let searching = pane.search.is_some();
    let (show_mod, show_kind) = visible_columns(pane);
    let entries: Vec<Entry> = range.clone().filter_map(|ix| pane.entries().get(ix).cloned()).collect();
    range
        .zip(entries)
        .map(|(ix, entry)| {
            let selected = pane.selected.contains(&entry.path);
            let is_cursor = pane.cursor == Some(ix);
            let dimmed = entry.hidden || pane.cut_paths.contains(&entry.path);
            let muted = if selected { ch.text } else { ch.text_muted };
            let row = h_flex()
                .id(ix)
                .w_full()
                .h(px(row_height))
                .px(px(9.))
                .gap_2()
                .rounded(px(8.))
                .text_size(px(11.))
                .text_color(ch.text)
                .when(dimmed, |d| d.opacity(0.55))
                .when(!selected && ix % 2 == 1, |d| d.bg(ch.zebra))
                .when(selected, |d| d.bg(if pane.active { ch.selection } else { ch.selection_inactive }))
                .when(selected && is_cursor && pane.selected.len() > 1, |d| d.border_1().border_color(ch.selection_border))
                .when(!selected, |d| d.hover(|s| s.bg(ch.hover_strong)));
            let icon_color = if entry.is_dir { ch.accent } else if selected { ch.text } else { ch.sidebar_icon };
            let row = entry_interactions(row, ix, &entry, pane.conn(), cx)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(10.))
                        .child(entry_icon(&entry, 16., icon_color, cx))
                        .child(div().flex_1().min_w_0().truncate().child(SharedString::from(entry.name.clone()))),
                )
                .when(searching, |d| {
                    let loc = entry.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
                    d.child(div().w(px(COL_LOCATION)).text_size(px(10.)).text_color(muted).truncate().child(loc))
                })
                .when(show_mod, |d| d.child(div().w(px(COL_MODIFIED)).text_size(px(10.)).text_color(muted).truncate().child(format_time(entry.modified, &now))))
                .child(div().w(px(COL_SIZE)).text_size(px(10.)).text_color(muted).flex().justify_end().child(format_size(&entry)))
                .when(show_kind, |d| d.child(div().w(px(COL_KIND)).text_size(px(10.)).text_color(muted).truncate().child(entry.kind_label())));
            div().h(px(row_height + ROW_GAP)).px(px(INSET)).pt(px(ROW_GAP)).child(row).into_any_element()
        })
        .collect()
}

pub fn format_size(entry: &Entry) -> String {
    if entry.is_dir {
        "—".into()
    } else {
        humansize::format_size(entry.size, humansize::DECIMAL)
    }
}

pub fn format_time(t: Option<SystemTime>, now: &DateTime<Local>) -> String {
    let Some(t) = t else { return "—".into() };
    let dt: DateTime<Local> = t.into();
    if dt.date_naive() == now.date_naive() {
        format!("Today, {}", dt.format("%H:%M"))
    } else if Some(dt.date_naive()) == now.date_naive().pred_opt() {
        format!("Yesterday, {}", dt.format("%H:%M"))
    } else {
        dt.format("%b %-d, %Y, %H:%M").to_string()
    }
}
