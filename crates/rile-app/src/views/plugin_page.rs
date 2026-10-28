//! Generic renderer for plugin-contributed pages (`Location::PluginPage`).
//!
//! Plugins describe their UI as a flat node tree (see `rile_plugins::PageView`);
//! this module turns it into GPUI elements and routes clicks back to the plugin
//! as page events.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Sizable, h_flex,
    button::{Button, ButtonVariants},
    spinner::Spinner,
    v_flex,
};
use gpui_kit::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, linear_color_stop, linear_gradient, prelude::FluentBuilder, px, relative, rgb,
};
use rile_plugins::{Bubble, NodeKind, PageView};

use crate::views::kit::PointerUnless;
use crate::theme::{Chrome, chrome};
use crate::workspace::Workspace;

/// Bubbles drawn per chart; the rest are too small to read anyway.
const MAX_DRAWN_BUBBLES: usize = 80;

/// Selection mode above the real explorer: sidebar, navigation and file views
/// continue working normally, with an explicit confirm/cancel at the top.
pub fn root_picker_bar(ws: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    if !ws.plugin_root_picker_active() { return None; }
    let ch = chrome(cx);
    let path = ws.plugin_root_picker_path(cx);
    Some(h_flex().id("plugin-root-picker").debug_selector(|| "plugin-root-picker".into())
        .gap_3().px_4().py_3().flex_wrap().border_b_1().border_color(ch.card_border).bg(ch.accent.opacity(0.05))
        .child(crate::icons::icon(IconName::FolderOpen).size(px(22.)).text_color(ch.accent))
        .child(v_flex().flex_1().min_w(px(140.)).gap_1()
            .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child("Choose a folder in Rile"))
            .child(div().truncate().text_size(px(11.)).text_color(ch.text_muted)
                .child(path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Open a local folder using the explorer or sidebar.".into()))))
        .child(Button::new("plugin-root-cancel").cursor_pointer().small().outline().label("Cancel").on_click(cx.listener(|ws, _, w, cx| ws.cancel_plugin_root(w, cx))))
        .child(Button::new("plugin-root-confirm").cursor_pointer().small().primary().icon(IconName::ChartPie).label("Analyze this folder").pointer_unless(path.is_none())
            .on_click(cx.listener(|ws, _, w, cx| ws.confirm_plugin_root(w, cx))))
        .into_any_element())
}

pub fn render(ws: &mut Workspace, slot: usize, plugin: &str, page: &str, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let key = (plugin.to_string(), page.to_string());
    let width = ws.slot_width(slot, window);
    let Some(session) = ws.plugins.sessions.get(&key) else {
        let msg = if ws.plugins.pages.iter().any(|(p, pg)| p == plugin && pg.id == page) {
            "Loading…"
        } else {
            "This plugin page is not available. Is the plugin enabled?"
        };
        return centered(msg, &ch).into_any_element();
    };

    let (pid, pgid) = (key.0.clone(), key.1.clone());
    let root = session.root.as_ref().map(|r| r.display().to_string()).unwrap_or_else(|| "No folder".into());
    let header = h_flex()
        .gap_2()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(ch.hairline)
        .child(div().flex_1().min_w_0().truncate().text_sm().text_color(ch.text_muted).child(root))
        .when(session.busy, |d| d.child(Spinner::new().xsmall()).child(div().text_sm().text_color(ch.text_muted).child("Working…")))
        .child(
            Button::new(("plugin-page-refresh", slot)).cursor_pointer()
                .small()
                .ghost()
                .icon(IconName::RefreshCw)
                .on_click(cx.listener(move |ws, _, w, cx| ws.refresh_plugin_page(&pid, &pgid, w, cx))),
        );

    let body: AnyElement = match (&session.view, &session.error) {
        _ if session.busy => v_flex()
            .id(("plugin-page-progress", slot))
            .debug_selector(|| "plugin-page-progress".into())
            .gap_3()
            .p_8()
            .items_center()
            .child(Spinner::new())
            .child(div().text_sm().text_color(ch.text_strong).child(if session.root.is_some() { "Analyzing folder…" } else { "Preparing page…" }))
            .when_some(session.root.as_ref(), |d, root| d.child(div().text_xs().text_color(ch.text_muted).child(root.display().to_string())))
            .child(div().w(px((width - 64.).clamp(80., 320.))).child(crate::progress::activity_bar(("plugin-scan-progress", slot), None, 5., cx)))
            .child(div().text_xs().text_color(ch.text_muted).child("Results appear when analysis is complete. Large folders can take a while."))
            .into_any_element(),
        (_, Some(err)) => centered(err.clone(), &ch).into_any_element(),
        (None, None) => centered("Loading…", &ch).into_any_element(),
        (Some(view), None) => {
            let mut r = Renderer { view, plugin: &key.0, page: &key.1, ch: &ch, width: (width - 32.).max(100.), chart_height: 350., zoom: session.bubble_zoom, next_id: 0 };
            let nodes: Vec<AnyElement> = view.children(None).map(|ix| r.node(ix, cx)).collect();
            v_flex().gap_3().p_4().children(nodes).into_any_element()
        }
    };

    v_flex()
        .size_full()
        .child(header)
        .child(div().id(("plugin-page-body", slot)).flex_1().min_h_0().overflow_y_scroll().child(body))
        .into_any_element()
}

fn centered(msg: impl Into<SharedString>, ch: &Chrome) -> impl IntoElement {
    div().size_full().flex().items_center().justify_center().p_8().text_color(ch.text_muted).child(msg.into())
}

struct Renderer<'a> {
    view: &'a PageView,
    plugin: &'a str,
    page: &'a str,
    ch: &'a Chrome,
    width: f32,
    chart_height: f32,
    zoom: u8,
    next_id: usize,
}

impl Renderer<'_> {
    fn id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    /// A click handler that sends `event` to the page.
    fn on_event(&self, event: String, cx: &mut Context<Workspace>) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static {
        let (plugin, page) = (self.plugin.to_string(), self.page.to_string());
        cx.listener(move |ws, _, w, cx| ws.plugin_page_event(&plugin, &page, event.clone(), w, cx))
    }

    fn node(&mut self, ix: usize, cx: &mut Context<Workspace>) -> AnyElement {
        let ch = self.ch;
        match &self.view.nodes[ix].kind {
            NodeKind::Column | NodeKind::Row => {
                let row = matches!(self.view.nodes[ix].kind, NodeKind::Row);
                let children: Vec<AnyElement> = self.view.children(Some(ix)).collect::<Vec<_>>().into_iter().map(|c| self.node(c, cx)).collect();
                if row {
                    h_flex().gap_2().flex_wrap().children(children).into_any_element()
                } else {
                    v_flex().gap_2().children(children).into_any_element()
                }
            }
            NodeKind::Heading(t) => div().text_xl().font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child(t.clone()).into_any_element(),
            NodeKind::Text(t) => div().text_sm().text_color(ch.text).child(t.clone()).into_any_element(),
            NodeKind::Muted(t) => div().text_sm().text_color(ch.text_muted).child(t.clone()).into_any_element(),
            NodeKind::Spacer => div().h(px(12.)).into_any_element(),
            NodeKind::Progress(v) => div()
                .h(px(6.))
                .w_full()
                .rounded_full()
                .bg(ch.hover_strong)
                .child(div().h_full().rounded_full().bg(ch.accent).w(relative(*v)))
                .into_any_element(),
            NodeKind::Button(b) => {
                let id = self.id();
                Button::new(("plugin-btn", id)).cursor_pointer().small().outline().label(b.label.clone()).on_click(self.on_event(b.event.clone(), cx)).into_any_element()
            }
            NodeKind::ListItem(item) => {
                let id = self.id();
                h_flex()
                    .id(("plugin-item", id))
                    .gap_3()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().text_color(ch.text_strong).child(item.title.clone()))
                            .when(!item.detail.is_empty(), |d| d.child(div().text_xs().truncate().text_color(ch.text_muted).child(item.detail.clone()))),
                    )
                    .child(div().text_sm().text_color(ch.text_muted).child(item.trailing.clone()))
                    .when_some(item.event.clone(), |d, ev| d.cursor_pointer().hover(|s| s.bg(ch.hover)).on_click(self.on_event(ev, cx)))
                    .into_any_element()
            }
            NodeKind::Bubbles(bubbles) => self.bubbles(bubbles, cx),
        }
    }

    fn bubbles(&mut self, bubbles: &[Bubble], cx: &mut Context<Workspace>) -> AnyElement {
        let ch = self.ch;
        let mut sorted: Vec<&Bubble> = bubbles.iter().filter(|b| b.value.is_finite() && b.value > 0.).collect();
        sorted.sort_by(|a, b| b.value.total_cmp(&a.value));
        sorted.truncate(MAX_DRAWN_BUBBLES);
        let max_value = sorted.first().map_or(1., |b| b.value);
        let radii: Vec<f32> = sorted.iter().map(|b| (b.value / max_value).sqrt() as f32 * 130.).collect();
        let circles = pack(&radii);
        let (min_x, min_y, max_x, max_y) = circles.iter().zip(&radii).fold(
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
            |(a, b, c, d), (&(x, y), &r)| (a.min(x - r), b.min(y - r), c.max(x + r), d.max(y + r)),
        );
        if circles.is_empty() {
            return div().p_4().text_sm().text_color(ch.text_muted).child("No non-empty items to display.").into_any_element();
        }
        let available = (self.width - 48.).max(1.);
        let scale = (available / (max_x - min_x)).min(self.chart_height / (max_y - min_y)) * (1. + self.zoom as f32 * 0.25);
        let height = (max_y - min_y) * scale;
        let width = (max_x - min_x) * scale;
        let mut chart = div().relative().w(px(width)).h(px(height)).mx_auto();
        for ((b, &(x, y)), &r) in sorted.iter().zip(&circles).zip(&radii) {
            let d = 2. * r * scale;
            let color: Hsla = b.color.map(|c| rgb(c).into()).unwrap_or(ch.accent);
            let id = self.id();
            let tip = format!("{} · {}", b.label, b.detail);
            let mut bubble = v_flex()
                .id(("plugin-bubble", id))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .absolute()
                .left(px((x - r - min_x) * scale))
                .top(px((y - r - min_y) * scale))
                .size(px(d))
                .rounded_full()
                .border_1()
                .border_color(color.opacity(0.28))
                .bg(linear_gradient(145., linear_color_stop(color.opacity(0.22), 0.), linear_color_stop(color.opacity(0.07), 1.)))
                .items_center()
                .justify_center()
                .overflow_hidden()
                .text_color(ch.text_strong)
                .when(d > 76., |el| {
                    el.child(crate::icons::icon(if b.event.as_deref().is_some_and(|e| e.starts_with("open:")) { IconName::Folder } else { IconName::File }).size(px(if d > 180. { 32. } else { 24. })).text_color(color))
                        .gap_1()
                })
                .when(d > 60., |el| {
                    el.child(div().text_size(px(if d > 180. { 14. } else { 12. })).font_weight(FontWeight::SEMIBOLD).truncate().max_w(px(d * 0.8)).child(b.label.clone()))
                        .when(d > 76. && !b.detail.is_empty(), |el| el.child(div().text_xs().text_color(ch.text_muted).child(b.detail.clone())))
                });
            if let Some(ev) = b.event.clone() {
                bubble = bubble.cursor_pointer().hover(|s| s.bg(color.opacity(0.30)).border_color(color.opacity(0.75))).on_click(self.on_event(ev, cx));
            }
            chart = chart.child(bubble);
        }
        let mut controls = h_flex().gap_1();
        for (label, step) in [("−".to_string(), self.zoom.saturating_sub(1)), (format!("{}%", 100 + self.zoom as u32 * 25), 0), ("+".to_string(), (self.zoom + 1).min(8))] {
            let key = (self.plugin.to_string(), self.page.to_string());
            let id = self.id();
            controls = controls.child(Button::new(("plugin-chart-zoom", id)).cursor_pointer().small().ghost().label(label).on_click(cx.listener(move |ws, _, _, cx| {
                if let Some(session) = ws.plugins.sessions.get_mut(&key) {
                    session.bubble_zoom = step;
                    cx.notify();
                }
            })));
        }
        v_flex().gap_3().p_4().rounded(px(12.)).border_1().border_color(ch.card_border)
            .bg(linear_gradient(145., linear_color_stop(ch.card_top, 0.), linear_color_stop(ch.card_bottom, 1.)))
            .child(h_flex().justify_between().gap_2()
                .child(v_flex().gap_1().child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Space map"))
                    .child(div().text_xs().text_color(ch.text_muted).child("Bigger bubbles take up more space")))
                .child(controls))
            .child(div().id(("plugin-chart-scroll", self.id())).overflow_x_scroll().child(chart))
            .child(div().text_xs().text_color(ch.text_muted).child(format!("{} of {} non-empty items · click a folder to explore · area represents size", sorted.len(), bubbles.len())))
            .into_any_element()
    }
}

/// Greedy circle packing: each circle (largest first) is placed tangent to an
/// already placed one, at the free spot closest to the origin. Returns centers.
pub(super) fn pack(radii: &[f32]) -> Vec<(f32, f32)> {
    const ANGLES: usize = 24;
    let mut placed: Vec<(f32, f32, f32)> = Vec::with_capacity(radii.len());
    for &r in radii {
        if placed.is_empty() {
            placed.push((0., 0., r));
            continue;
        }
        let mut best: Option<(f32, f32, f32)> = None;
        for &(px_, py, pr) in &placed {
            for k in 0..ANGLES {
                let a = k as f32 / ANGLES as f32 * std::f32::consts::TAU;
                let dist = pr + r + 1e-3 * (pr + r);
                let (x, y) = (px_ + dist * a.cos(), py + dist * a.sin());
                if placed.iter().any(|&(qx, qy, qr)| (qx - x).hypot(qy - y) < qr + r) {
                    continue;
                }
                let score = x.hypot(y * 1.5);
                if best.is_none_or(|(_, _, s)| score < s) {
                    best = Some((x, y, score));
                }
            }
        }
        // Always found: the outermost angle around any circle is free eventually;
        // fall back to stacking far out just in case.
        let (x, y, _) = best.unwrap_or((placed.iter().map(|p| p.0.abs() + p.2).fold(0., f32::max) + r, 0., 0.));
        placed.push((x, y, r));
    }
    placed.into_iter().map(|(x, y, _)| (x, y)).collect()
}

#[cfg(test)]
mod tests {
    use super::pack;

    #[test]
    fn packed_circles_do_not_overlap() {
        for radii in [vec![], vec![130.], vec![30.; 80], vec![130., 0.01, 0.001], (1..40).map(|i| (100.0 / i as f32).sqrt()).collect()] {
            let c = pack(&radii);
            assert_eq!(c.len(), radii.len());
            for i in 0..c.len() {
                assert!(c[i].0.is_finite() && c[i].1.is_finite());
                for j in 0..i {
                    let d = (c[i].0 - c[j].0).hypot(c[i].1 - c[j].1);
                    assert!(d >= radii[i] + radii[j] - 1e-3, "{i} overlaps {j}");
                }
            }
        }
    }
}
