use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, uniform_list,
};
use rile_core::Entry;

use super::{empty_message, entry_interactions, placeholder, prefs};
use gpui_kit::assets::IconName;
use crate::icons::{entry_icon, icon_for_entry};
use crate::pane::Pane;

const COLUMN_WIDTH: f32 = 240.;
/// Ancestor columns shown to the left of the current folder.
const ANCESTORS: usize = 2;

/// Miller columns: up to two ancestor folders, the current folder (the live,
/// selectable listing) and a trailing column previewing the cursor item.
pub fn render(pane: &mut Pane, _window: &mut Window, cx: &mut Context<Pane>) -> AnyElement {
    let theme = crate::theme::chrome(cx);
    let Some(dir) = pane.dir_path() else {
        return placeholder("".into(), cx);
    };
    // Show as many ancestors as fit beside the current + preview columns.
    let fit = (pane.viewport_width / COLUMN_WIDTH).floor() as usize;
    let n_anc = fit.saturating_sub(2).min(ANCESTORS);
    let mut ancestors: Vec<PathBuf> = dir.ancestors().skip(1).take(n_anc).map(Path::to_path_buf).collect();
    ancestors.reverse();

    let mut cols: Vec<AnyElement> = Vec::new();
    for (i, anc) in ancestors.iter().enumerate() {
        // The child of `anc` that leads to the current folder is highlighted.
        let next = if i + 1 < ancestors.len() { ancestors[i + 1].clone() } else { dir.clone() };
        let listing = pane.side_listing(anc, cx);
        cols.push(side_column(("anc", i), listing, Some(next), true, cx));
    }

    // Current folder: interactive column.
    let current: AnyElement = match empty_message(pane) {
        Some(msg) => placeholder(msg, cx),
        None => uniform_list("miller-current", pane.entries().len(), cx.processor(|pane: &mut Pane, range: std::ops::Range<usize>, _w, cx| {
            let theme = cx.theme().clone();
            let ch = crate::theme::chrome(cx);
            let row_h = prefs(cx).row_height + 4.;
            range
                .filter_map(|ix| pane.entries().get(ix).cloned().map(|e| (ix, e)))
                .map(|(ix, entry)| {
                    let selected = pane.selected.contains(&entry.path);
                    let row = h_flex()
                        .id(ix)
                        .w_full()
                        .h(px(row_h - 2.))
                        .px(px(9.))
                        .gap_2()
                        .rounded(px(5.))
                        .text_size(px(11.))
                        .text_color(if selected { ch.text_strong } else { ch.text })
                        .when(entry.hidden || pane.cut_paths.contains(&entry.path), |d| d.opacity(0.55))
                        .when(selected, |d| d.bg(if pane.active { ch.selection } else { ch.selection_inactive }))
                        .when(!selected, |d| d.hover(|s| s.bg(ch.hover_strong)));
                    let _ = &theme;
                    let row = entry_interactions(row, ix, &entry, pane.conn(), cx)
                        .child(crate::icons::with_shared_badge(entry_icon(&entry, 16., if entry.is_dir { ch.accent } else { ch.sidebar_icon }, cx), &entry, 16., pane.conn().is_none(), cx))
                        .child(div().flex_1().min_w_0().truncate().child(SharedString::from(entry.name.clone())))
                        .when(entry.is_dir, |d| d.child(crate::icons::icon(IconName::ChevronRight).size(px(10.)).text_color(ch.text_faint)));
                    div().h(px(row_h)).px(px(8.)).pt(px(2.)).child(row).into_any_element()
                })
                .collect::<Vec<_>>()
        }))
        .track_scroll(&pane.scroll)
        .size_full()
        .into_any_element(),
    };
    cols.push(column_shell("miller-cur", current, cx));

    // Preview column for the cursor entry.
    if let Some(entry) = pane.cursor_entry().cloned() {
        if entry.is_dir {
            let listing = pane.side_listing(&entry.path, cx);
            cols.push(side_column("child", listing, None, false, cx));
        } else {
            cols.push(column_shell("miller-info", file_info(&entry, cx), cx));
        }
    }

    h_flex()
        .id("miller")
        .size_full()
        .overflow_x_scroll()
        .bg(theme.browser)
        .children(cols)
        .into_any_element()
}

fn column_shell(id: impl Into<gpui_kit::ElementId>, content: AnyElement, cx: &mut Context<Pane>) -> AnyElement {
    let theme = crate::theme::chrome(cx);
    div()
        .id(id)
        .w(px(COLUMN_WIDTH))
        .flex_none()
        .h_full()
        .border_r_1()
        .border_color(theme.hairline)
        .child(content)
        .into_any_element()
}

/// Read-only column for a neighbour folder; clicking an item navigates there.
fn side_column(
    id: impl Into<gpui_kit::ElementId> + Clone,
    listing: Option<Arc<Vec<Entry>>>,
    highlight: Option<PathBuf>,
    clickable: bool,
    cx: &mut Context<Pane>,
) -> AnyElement {
    let ch = crate::theme::chrome(cx);
    let row_h = prefs(cx).row_height + 2.;
    let content = match listing {
        None => placeholder("…".into(), cx),
        Some(l) if l.is_empty() => placeholder("Empty".into(), cx),
        Some(listing) => v_flex()
            .id("side-list")
            .size_full()
            .overflow_y_scroll()
            .children(listing.iter().take(2000).enumerate().map(|(i, e)| {
                let hl = highlight.as_ref() == Some(&e.path);
                let path = e.path.clone();
                let is_dir = e.is_dir;
                h_flex()
                    .id(("side", i))
                    .w_full()
                    .h(px(row_h))
                    .mx(px(8.))
                    .my(px(1.))
                    .px(px(9.))
                    .gap_2()
                    .flex_none()
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .text_color(if hl { ch.text_strong } else { ch.text_muted })
                    .when(hl, |d| d.bg(ch.selection_inactive))
                    .cursor_pointer()
                    .hover(|s| s.bg(ch.hover_strong))
                    .child(crate::icons::icon(icon_for_entry(e)).size_4().text_color(if e.is_dir { ch.accent } else { ch.sidebar_icon }))
                    .child(div().flex_1().min_w_0().truncate().child(SharedString::from(e.name.clone())))
                    .when(clickable || is_dir, |d| {
                        d.cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                this.activate(window, cx);
                                let target = if is_dir { path.clone() } else { path.parent().map(Path::to_path_buf).unwrap_or_default() };
                                let loc = this.location_for(target);
                                this.navigate(loc, cx);
                            }),
                        )
                    })
            }))
            .into_any_element(),
    };
    column_shell(id, content, cx)
}

fn file_info(entry: &Entry, cx: &mut Context<Pane>) -> AnyElement {
    let theme = cx.theme().clone();
    let now = chrono::Local::now();
    v_flex()
        .size_full()
        .p_4()
        .gap_2()
        .items_center()
        .child(div().pt(px(18.)).pb(px(12.)).child(crate::art::file_object(entry, 96., true, true, cx)))
        .child(div().text_sm().text_center().child(SharedString::from(entry.name.clone())))
        .child(div().text_xs().text_color(theme.muted_foreground).child(entry.kind_label()))
        .child(div().text_xs().text_color(theme.muted_foreground).child(super::list::format_size(entry)))
        .child(div().text_xs().text_color(theme.muted_foreground).child(super::list::format_time(entry.modified, &now)))
        .child(div().pt_2().text_xs().text_color(theme.muted_foreground).child(format!("Space: {}", crate::platform_text::preview())))
        .into_any_element()
}
