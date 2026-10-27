use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Sizable, h_flex,
    button::{Button, ButtonVariants},
    slider::Slider,
    spinner::Spinner,
};
use gpui_kit::{Context, IntoElement, ParentElement, Styled, Window, div, prelude::FluentBuilder, px};
use rile_core::history::Location;
use rile_core::settings::ViewKind;

use crate::theme::chrome;
use crate::views::chrome::{Width, width_class};
use crate::workspace::{Workspace, item_count};

pub fn render(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> impl IntoElement {
    let ch = chrome(cx);
    let width = width_class(window);
    let pane = ws.pane().read(cx);
    let busy = pane.loading || pane.search.as_ref().is_some_and(|s| s.running) || ws.plugins.loading || ws.overview.loading;
    let at_home = matches!(pane.location, Location::Home);
    let show_zoom = !at_home && pane.view == ViewKind::Grid && width > Width::Phone;
    let (summary, selection) = if at_home {
        let disks = ws.volumes.len();
        let items = ws.overview.stats.as_ref().map(|s| s.items).unwrap_or(0);
        (
            format!("{} in your places · {disks} disk{}", item_count(items), if disks == 1 { "" } else { "s" }),
            None,
        )
    } else {
        let mut s = item_count(pane.entries().len());
        if pane.loading {
            s.push_str(" — loading…");
        }
        let sel = pane.selected.len();
        let selection = (sel > 0).then(|| {
            let bytes: u64 = pane.selected_entries().iter().map(|e| e.size).sum();
            format!("{sel} selected · {}", humansize::format_size(bytes, humansize::DECIMAL))
        });
        (s, selection)
    };

    let ops = ws.ops.iter().map(|op| {
        let id = op.id;
        let p = &op.progress;
        let known = p.bytes_total > 0 || p.items_total > 0 || p.files_total > 0;
        let fraction = p.fraction();
        h_flex()
            .gap_2()
            .flex_none()
            .child(div().max_w(px(260.)).truncate().child(op.label.clone()))
            .child(div().w(px(140.)).child(crate::progress::activity_bar(("op-progress", id), known.then_some(fraction), 6., cx)))
            .child(div().w(px(34.)).child(if known { format!("{:.0}%", fraction * 100.) } else { "…".into() }))
            .child(
                Button::new(("op-cancel", id)).cursor_pointer()
                    .ghost()
                    .xsmall()
                    .icon(crate::icons::icon(IconName::X))
                    .tooltip("Cancel")
                    .on_click(cx.listener(move |this, _, _, _| this.cancel_op(id))),
            )
    });

    h_flex()
        .h(px(31.))
        .px(px(if width <= Width::Phone { 11. } else { 20. }))
        .gap(px(13.))
        .flex_none()
        .border_t_1()
        .border_color(ch.hairline)
        .bg(ch.statusbar)
        .text_size(px(10.))
        .text_color(ch.text_faint)
        .when(busy, |d| d.child(Spinner::new().xsmall()))
        .child(div().flex_none().whitespace_nowrap().child(summary))
        .when_some(selection, |d, sel| {
            d.child(div().flex_none().pl(px(13.)).border_l_1().border_color(ch.hairline).whitespace_nowrap().child(sel))
        })
        .child(div().flex_1().min_w_0())
        .children(ops)
        .when(show_zoom, |d| {
            d.child(
                h_flex()
                    .flex_none()
                    .gap(px(9.))
                    .child(crate::icons::icon(IconName::LayoutGrid).size(px(10.)).text_color(ch.text_faint))
                    .child(div().w(px(77.)).child(Slider::new(&ws.zoom).accessibility_label("Icon size")))
                    .child(crate::icons::icon(IconName::LayoutGrid).size(px(13.)).text_color(ch.text_faint)),
            )
        })
}
