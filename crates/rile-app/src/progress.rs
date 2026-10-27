//! Animated progress indicators.

use std::time::Duration;

use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    Animation, AnimationExt, AnyElement, App, ElementId, IntoElement, ParentElement, Styled, div,
    ease_in_out, px, relative,
};

/// A progress bar. `Some(fraction)` fills determinately with a moving sheen
/// so long transfers visibly stay alive; `None` shows an indeterminate
/// sliding segment for work without a known total.
pub fn activity_bar(id: impl Into<ElementId>, fraction: Option<f32>, height: f32, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let id: ElementId = id.into();
    let track = div()
        .w_full()
        .h(px(height))
        .rounded_full()
        .overflow_hidden()
        .relative()
        .bg(theme.secondary_active);
    let accent = theme.primary;
    match fraction {
        Some(f) => track
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .h_full()
                    .w(relative(f.clamp(0., 1.)))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(accent)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .h_full()
                            .w(relative(0.35))
                            .bg(gpui_kit::white().opacity(0.28))
                            .with_animation(
                                (id.clone(), "sheen"),
                                Animation::new(Duration::from_millis(1400)).repeat().with_easing(ease_in_out),
                                |el, t| el.left(relative(t * 1.7 - 0.35)),
                            ),
                    ),
            )
            .into_any_element(),
        None => track
            .child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .w(relative(0.3))
                    .rounded_full()
                    .bg(accent)
                    .with_animation(
                        (id, "slide"),
                        Animation::new(Duration::from_millis(1100)).repeat().with_easing(ease_in_out),
                        |el, t| el.left(relative(t * 1.3 - 0.3)),
                    ),
            )
            .into_any_element(),
    }
}
