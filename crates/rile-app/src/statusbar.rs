use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Sizable, h_flex,
    button::{Button, ButtonVariants},
    slider::Slider,
    spinner::Spinner,
};
use gpui_kit::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px,
};
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

    // Background jobs: the panel opens above the button, at the right edge.
    const JOBS_INSET: f32 = (crate::views::chrome::FOOTER_HEIGHT - 22.) / 2.;
    let jobs = ws.ops.len();
    let jobs_active = ws.jobs_open || jobs > 0;
    let jobs_button = div()
        .id("statusbar-jobs")
        .relative()
        .flex_none()
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        // It sits in the window's bottom-right corner, inset by `JOBS_INSET` on
        // both axes: round that corner concentric with the window's (macOS
        // window radius ≈ 16 px) so the gap follows the curve.
        .when(cfg!(target_os = "macos"), |d| d.rounded_br(px(16. - JOBS_INSET)))
        .cursor_pointer()
        .when(jobs_active, |d| d.bg(ch.sidebar_active))
        .hover(|s| s.bg(ch.hover_strong))
        .child(crate::icons::icon(IconName::Activity).size(px(14.)).text_color(if jobs_active { ch.sidebar_active_icon } else { ch.sidebar_icon }))
        .when(jobs > 0, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(-4.))
                    .right(px(-5.))
                    .min_w(px(13.))
                    .h(px(13.))
                    .px(px(3.))
                    .rounded_full()
                    .bg(ch.accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(8.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(gpui_kit::white())
                    .child(jobs.to_string()),
            )
        })
        .tooltip(move |w, cx| {
            let tip = if jobs == 0 { "Background jobs".to_string() } else { format!("Background jobs: {jobs} running") };
            gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            let just_closed = this.jobs_closed_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400));
            this.jobs_open = !this.jobs_open && !just_closed;
            cx.notify();
        }));

    h_flex()
        .h(px(crate::views::chrome::FOOTER_HEIGHT))
        .pl(px(if width <= Width::Phone { 11. } else { 20. }))
        .pr(px(JOBS_INSET))
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
        .child(jobs_button)
}
