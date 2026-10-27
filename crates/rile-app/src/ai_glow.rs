//! Apple-Intelligence-style rainbow effects shown while an AI search runs:
//! a flowing edge glow around the pane and a shimmering search field border.

use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt, AnyElement, BoxShadow, ElementId, Hsla, IntoElement, ParentElement, Styled, div, hsla,
    linear_color_stop, linear_gradient, point, px,
};

/// Segments across the bottom edge; more = smoother hue flow.
const SEGMENTS: usize = 24;
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
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(56.))
        .flex()
        .flex_row()
        .items_end()
        .with_animation(id, Animation::new(CYCLE).repeat(), |el, t| {
            let breathe = 0.5 + 0.5 * (t * std::f32::consts::TAU).sin();
            let depth = 30. + 10. * breathe;
            el.children((0..SEGMENTS).map(move |k| {
                // The whole palette spans the width, so neighbours differ little.
                let a = k as f32 / SEGMENTS as f32 - t;
                let b = (k + 1) as f32 / SEGMENTS as f32 - t;
                let line = linear_gradient(90., linear_color_stop(rainbow(a, 0.7), 0.), linear_color_stop(rainbow(b, 0.7), 1.));
                let glow = linear_gradient(
                    0.,
                    linear_color_stop(rainbow((a + b) / 2., 0.14 + 0.06 * breathe), 0.),
                    linear_color_stop(rainbow(b, 0.), 1.),
                );
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h(px(depth))
                    .child(div().flex_1().bg(glow))
                    .child(div().flex_none().h(px(1.5)).bg(line))
            }))
        })
        .into_any_element()
}

/// Rainbow border + colored halo for the search field while AI works.
pub fn search_halo<E: Styled + IntoElement + 'static>(el: E, id: impl Into<ElementId>) -> impl IntoElement {
    el.with_animation(id, Animation::new(Duration::from_millis(6000)).repeat(), |el, t| {
        let pulse = 0.5 + 0.5 * (t * std::f32::consts::TAU).sin();
        el.border_color(rainbow(t, 0.75)).shadow(vec![
            BoxShadow { color: rainbow(t, 0.22), offset: point(px(-2.), px(0.)), blur_radius: px(8. + 3. * pulse), spread_radius: px(0.), inset: false },
            BoxShadow { color: rainbow(t + 0.33, 0.22), offset: point(px(2.), px(0.)), blur_radius: px(8. + 3. * pulse), spread_radius: px(0.), inset: false },
            BoxShadow { color: rainbow(t + 0.66, 0.18), offset: point(px(0.), px(2.)), blur_radius: px(10.), spread_radius: px(0.), inset: false },
        ])
    })
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
