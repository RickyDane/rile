use std::ops::Range;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, Window, div,
    prelude::FluentBuilder, px, uniform_list,
};
use rile_core::Entry;

use super::{empty_message, entry_interactions, placeholder, prefs};
use crate::pane::Pane;
use crate::theme::chrome;

/// Icon-size range of the zoom slider (the setting's S…XL presets).
pub const MIN_ICON: f32 = 48.;
pub const MAX_ICON: f32 = 96.;
/// Space around the grid content (the file area's padding).
pub const GRID_PADDING_X: f32 = 19.;
pub const GRID_PADDING_Y: f32 = 6.;
/// Inset of a tile inside its cell (half the gap between tiles).
pub const TILE_INSET: f32 = 3.;

/// Minimum cell width and the cell height for an icon size. Cells stretch
/// to fill the row, like the approved `minmax(--icon-size, 1fr)` grid.
pub fn tile_metrics(icon_size: f32) -> (f32, f32) {
    let icon = icon_size.clamp(MIN_ICON, MAX_ICON);
    (icon + 70., object_size(icon) + 82.)
}

/// Height of the file object box for an icon size (88px at the default 64).
pub fn object_size(icon_size: f32) -> f32 {
    icon_size.clamp(MIN_ICON, MAX_ICON) * 1.375
}

pub fn render(pane: &mut Pane, _window: &mut Window, cx: &mut Context<Pane>) -> AnyElement {
    let (min_w, h) = tile_metrics(prefs(cx).icon_size);
    pane.tile_min_width = min_w;
    pane.tile_height = h;
    if let Some(msg) = empty_message(pane) {
        return placeholder(msg, cx);
    }
    let per_row = pane.columns_per_row();
    let rows = pane.entries().len().div_ceil(per_row);
    uniform_list("file-grid", rows, cx.processor(render_rows))
        .track_scroll(&pane.scroll)
        .size_full()
        .py(px(GRID_PADDING_Y))
        .into_any_element()
}

fn render_rows(pane: &mut Pane, range: Range<usize>, _window: &mut Window, cx: &mut Context<Pane>) -> Vec<AnyElement> {
    let per_row = pane.columns_per_row();
    let tile_w = pane.tile_width();
    range
        .map(|row| {
            let start = row * per_row;
            let end = (start + per_row).min(pane.entries().len());
            let tiles: Vec<AnyElement> = (start..end)
                .map(|ix| {
                    let entry = pane.entries()[ix].clone();
                    render_tile(pane, ix, &entry, tile_w, cx)
                })
                .collect();
            h_flex().id(("grid-row", row)).h(px(pane.tile_height)).px(px(GRID_PADDING_X)).children(tiles).into_any_element()
        })
        .collect()
}

fn render_tile(pane: &Pane, ix: usize, entry: &Entry, tile_w: f32, cx: &mut Context<Pane>) -> AnyElement {
    let ch = chrome(cx);
    let prefs = prefs(cx);
    let selected = pane.selected.contains(&entry.path);
    let dimmed = entry.hidden || pane.cut_paths.contains(&entry.path);
    let object = object_size(prefs.icon_size);
    let visual = crate::art::file_object(entry, object, prefs.thumbnails, pane.conn().is_none(), cx);
    let meta: SharedString = if entry.is_dir { entry.kind_label().into() } else { super::list::format_size(entry).into() };
    let tile = v_flex()
        .id(ix)
        .w(px(tile_w - TILE_INSET * 2.))
        .h(px(pane.tile_height - TILE_INSET * 2. - 11.))
        .m(px(TILE_INSET))
        .px(px(8.))
        .pt(px(12.))
        .items_center()
        .rounded(px(8.))
        .border_1()
        .border_color(gpui_kit::transparent_black())
        .when(dimmed, |d| d.opacity(0.55))
        .when(selected, |d| {
            d.bg(if pane.active { ch.selection } else { ch.selection_inactive }).border_color(ch.selection_border)
        })
        .when(!selected, |d| d.hover(|s| s.bg(ch.hover)));
    entry_interactions(tile, ix, entry, pane.conn(), cx)
        .child(div().h(px(object)).w_full().flex().items_center().justify_center().child(visual))
        .child(
            div()
                .mt(px(12.))
                .w_full()
                .truncate()
                .text_center()
                .text_size(px(11.))
                .text_color(ch.text)
                .child(SharedString::from(entry.name.clone())),
        )
        .child(div().mt(px(4.)).text_size(px(10.)).text_color(if selected { ch.text } else { ch.text_muted }).child(meta))
        .into_any_element()
}
