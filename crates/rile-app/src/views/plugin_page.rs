//! Generic renderer for plugin-contributed pages (`Location::PluginPage`).
//!
//! Plugins describe their UI as a flat node tree (see `rile_plugins::PageView`);
//! this module turns it into GPUI elements and routes clicks back to the plugin
//! as page events.

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Sizable, h_flex,
    button::{Button, ButtonVariants},
    v_flex,
};
use gpui_kit::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, relative, rgb,
};
use rile_plugins::{Bubble, NodeKind, PageView};

use crate::theme::{Chrome, chrome};
use crate::workspace::Workspace;

/// Bubbles drawn per chart; the rest are too small to read anyway.
const MAX_DRAWN_BUBBLES: usize = 80;
const PALETTE: [u32; 8] = [0x4f8ef7, 0xf2994a, 0x27ae60, 0xbb6bd9, 0xeb5757, 0x2d9cdb, 0xf2c94c, 0x6fcf97];

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
        .when(session.busy, |d| d.child(div().text_sm().text_color(ch.text_muted).child("Working…")))
        .child(
            Button::new(("plugin-page-refresh", slot))
                .small()
                .ghost()
                .icon(IconName::RefreshCw)
                .on_click(cx.listener(move |ws, _, w, cx| ws.refresh_plugin_page(&pid, &pgid, w, cx))),
        );

    let body: AnyElement = match (&session.view, &session.error) {
        (_, Some(err)) => centered(err.clone(), &ch).into_any_element(),
        (None, None) => centered("Loading…", &ch).into_any_element(),
        (Some(view), None) => {
            let mut r = Renderer { view, plugin: &key.0, page: &key.1, ch: &ch, width: (width - 32.).max(100.), next_id: 0 };
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
                Button::new(("plugin-btn", id)).small().outline().label(b.label.clone()).on_click(self.on_event(b.event.clone(), cx)).into_any_element()
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
        let mut sorted: Vec<&Bubble> = bubbles.iter().collect();
        sorted.sort_by(|a, b| b.value.total_cmp(&a.value));
        sorted.truncate(MAX_DRAWN_BUBBLES);
        let radii: Vec<f32> = sorted.iter().map(|b| (b.value as f32).sqrt()).collect();
        let circles = pack(&radii);
        let (min_x, min_y, max_x, max_y) = circles.iter().zip(&radii).fold(
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
            |(a, b, c, d), (&(x, y), &r)| (a.min(x - r), b.min(y - r), c.max(x + r), d.max(y + r)),
        );
        if circles.is_empty() {
            return div().into_any_element();
        }
        let size = self.width.min(640.);
        let scale = size / (max_x - min_x).max(max_y - min_y).max(f32::EPSILON);
        let height = (max_y - min_y) * scale;
        let width = (max_x - min_x) * scale;
        let mut chart = div().relative().w(px(width)).h(px(height)).mx_auto();
        for (i, ((b, &(x, y)), &r)) in sorted.iter().zip(&circles).zip(&radii).enumerate() {
            let d = 2. * r * scale;
            let color: Hsla = rgb(b.color.unwrap_or(PALETTE[i % PALETTE.len()])).into();
            let id = self.id();
            let mut bubble = v_flex()
                .id(("plugin-bubble", id))
                .absolute()
                .left(px((x - r - min_x) * scale))
                .top(px((y - r - min_y) * scale))
                .size(px(d))
                .rounded_full()
                .bg(color.opacity(0.85))
                .items_center()
                .justify_center()
                .overflow_hidden()
                .text_color(gpui_kit::white())
                .when(d > 48., |el| {
                    el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).truncate().max_w(px(d * 0.8)).child(b.label.clone()))
                        .when(d > 72. && !b.detail.is_empty(), |el| el.child(div().text_xs().opacity(0.85).child(b.detail.clone())))
                });
            if let Some(ev) = b.event.clone() {
                bubble = bubble.cursor_pointer().hover(|s| s.bg(color)).on_click(self.on_event(ev, cx));
            }
            chart = chart.child(bubble);
        }
        chart.into_any_element()
    }
}

/// Greedy circle packing: each circle (largest first) is placed tangent to an
/// already placed one, at the free spot closest to the origin. Returns centers.
fn pack(radii: &[f32]) -> Vec<(f32, f32)> {
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
                let score = x.hypot(y);
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
        let radii: Vec<f32> = (1..40).map(|i| (100.0 / i as f32).sqrt()).collect();
        let c = pack(&radii);
        for i in 0..c.len() {
            for j in 0..i {
                let d = (c[i].0 - c[j].0).hypot(c[i].1 - c[j].1);
                assert!(d >= radii[i] + radii[j] - 1e-3, "{i} overlaps {j}");
            }
        }
    }
}
