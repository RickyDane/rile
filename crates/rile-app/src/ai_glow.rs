//! Apple-Intelligence-style rainbow effects shown while an AI search runs:
//! a calm glow along the bottom of the file list.

use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt, AnyElement, ElementId, Hsla, IntoElement, ParentElement, Styled, div, hsla,
    linear_color_stop, linear_gradient, px,
};

/// Segments across the bottom edge; more = smoother hue flow.
const SEGMENTS: usize = 16;
/// Vertical steps of the glow's fade.
const ROWS: usize = 16;
/// One slow drift of the colors along the edge.
const CYCLE: Duration = Duration::from_millis(9000);

/// Apple Intelligence palette: orange → pink → violet → blue → cyan, then
/// back the same way so the loop never passes through green.
const STOPS: [(f32, f32, f32); 8] = [
    (0.07, 1.0, 0.62),  // orange
    (0.95, 0.95, 0.66), // pink
    (0.82, 0.85, 0.66), // magenta-violet
    (0.70, 0.9, 0.66),  // indigo
    (0.55, 0.95, 0.6),  // cyan-blue
    (0.62, 0.95, 0.62), // blue
    (0.76, 0.88, 0.66), // violet
    (0.90, 0.95, 0.66), // rose
];

/// Smoothly interpolated color at position `p` (wraps at 1.0) along the palette.
pub fn rainbow(p: f32, alpha: f32) -> Hsla {
    let p = p.rem_euclid(1.0) * STOPS.len() as f32;
    let i = p.floor() as usize % STOPS.len();
    let j = (i + 1) % STOPS.len();
    let f = p.fract();
    let (h0, s0, l0) = STOPS[i];
    let (mut h1, s1, l1) = STOPS[j];
    // Take the short way around the hue wheel.
    if (h1 - h0).abs() > 0.5 {
        h1 += if h1 < h0 { 1.0 } else { -1.0 };
    }
    hsla((h0 + (h1 - h0) * f).rem_euclid(1.0), s0 + (s1 - s0) * f, l0 + (l1 - l0) * f, alpha)
}

/// Click-through overlay: a calm rainbow glow rising from the bottom edge,
/// colors drifting slowly sideways and the glow gently breathing.
pub fn bottom_glow(id: impl Into<ElementId>) -> AnyElement {
    let id = id.into();
    let glow = div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(56.))
        .flex()
        .with_animation(ElementId::NamedChild(std::sync::Arc::new(id.clone()), "flow".into()), Animation::new(CYCLE).repeat(), |el, t| {
            let breathe = 0.5 + 0.5 * (t * std::f32::consts::TAU).sin();
            let depth = 30. + 10. * breathe;
            // Each row is a strip of horizontal gradients (smooth across the
            // width); rows stack upwards with an eased alpha falloff, so there
            // are no flat-colored columns.
            let strip = move |alpha: f32, h: f32| {
                div().flex().flex_row().flex_none().w_full().h(px(h)).children((0..SEGMENTS).map(move |k| {
                    let a = k as f32 / SEGMENTS as f32 - t;
                    let b = (k + 1) as f32 / SEGMENTS as f32 - t;
                    div().flex_1().h_full().bg(linear_gradient(
                        90.,
                        linear_color_stop(rainbow(a, alpha), 0.),
                        linear_color_stop(rainbow(b, alpha), 1.),
                    ))
                }))
            };
            let peak = 0.14 + 0.06 * breathe;
            let row_h = depth / ROWS as f32;
            el.flex_col()
                .justify_end()
                .children((0..ROWS).map(move |r| {
                    // r = 0 is the top (faintest) row.
                    let x = (r as f32 + 0.5) / ROWS as f32;
                    strip(peak * x * x, row_h)
                }))
                .child(strip(0.7, 1.5))
        })
        ;
    // Fades in when the search starts instead of popping up.
    div()
        .absolute()
        .inset_0()
        .child(glow)
        .with_animation(
            ElementId::NamedChild(std::sync::Arc::new(id), "fade".into()),
            Animation::new(Duration::from_millis(900)).with_easing(gpui_kit::ease_in_out),
            |el, t| el.opacity(t),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::rainbow;

    #[test]
    fn rainbow_wraps_and_is_continuous() {
        let a = rainbow(0.0, 1.0);
        let b = rainbow(1.0, 1.0);
        assert!((a.h - b.h).abs() < 1e-4);
        for i in 0..1000 {
            let p = i as f32 / 1000.;
            let (x, y) = (rainbow(p, 1.), rainbow(p + 0.001, 1.));
            let dh = (x.h - y.h).abs();
            assert!(dh.min(1. - dh) < 0.01, "jump at {p}");
        }
    }
}
