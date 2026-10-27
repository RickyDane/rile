//! Native dialog kit: the shared surface, header, segmented controls,
//! labelled fields and grouped rows every popup is built from, so dialogs
//! match the explorer chrome (graphite surfaces, hairlines, accent icons).

use gpui_kit::assets::IconName;
use gpui_kit::component::{dialog::{Dialog, DialogEntrance}, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Div, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, Stateful,
    Styled, div, linear_color_stop, linear_gradient, prelude::FluentBuilder, px, rgba,
};

use crate::theme::{Chrome, chrome};

/// Applies the Native surface to a dialog: raised graphite card, hairline
/// border, large radius and a soft shadow.
pub fn surface<D: Styled>(dialog: D, cx: &App) -> D {
    let ch = chrome(cx);
    let (top, bottom) = ch.surface_colors();
    dialog
        .bg(linear_gradient(180., linear_color_stop(top, 0.), linear_color_stop(bottom, 1.)))
        .border_1()
        .border_color(match (ch.glass, ch.dark) {
            (true, true) => gpui_kit::Hsla::from(rgba(0xffffff2a)),
            (true, false) => gpui_kit::Hsla::from(rgba(0xffffffb0)),
            (false, true) => gpui_kit::Hsla::from(rgba(0xffffff17)),
            (false, false) => ch.hairline,
        })
        .rounded(px(14.))
        .shadow_2xl()
}

/// A Native dialog of `width` with an icon header, centered in the window.
pub fn dialog(dialog: Dialog, icon: IconName, title: impl Into<SharedString>, subtitle: Option<SharedString>, width: f32, cx: &App) -> Dialog {
    let ch = chrome(cx);
    surface(dialog, cx)
        .centered()
        .entrance(DialogEntrance::Fade)
        .w(px(width))
        // The library's built-in close button uses the default arrow cursor.
        // Keep its accessible, dialog-scoped activation with our own styling.
        .close_button(false)
        .title(
            h_flex()
                .justify_between()
                .gap(px(12.))
                .child(header(icon, title.into(), subtitle, cx))
                .child(gpui_kit::base::DialogClose::new().trigger(|button| close_button(button, ch))),
        )
}

fn close_button(button: gpui_kit::base::Button, ch: Chrome) -> gpui_kit::base::Button {
    button
        .debug_selector(|| "dialog-close".into())
        .cursor_pointer()
        .size(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .text_color(ch.text_muted)
        .hover(|style| style.bg(ch.hover_strong).text_color(ch.text))
        .child(crate::icons::icon(IconName::X).size(px(16.)))
}

/// Icon tile, title and an optional one-line subtitle.
pub fn header(icon: IconName, title: SharedString, subtitle: Option<SharedString>, cx: &App) -> AnyElement {
    let ch = chrome(cx);
    h_flex()
        .gap(px(12.))
        .pb(px(4.))
        .child(
            div()
                .size(px(34.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(9.))
                .bg(ch.accent_soft)
                .border_1()
                .border_color(ch.accent.opacity(0.18))
                .child(crate::icons::icon(icon).size(px(17.)).text_color(ch.accent)),
        )
        .child(
            v_flex()
                .min_w_0()
                .gap(px(3.))
                .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child(title))
                .children(subtitle.map(|s| div().text_size(px(11.)).font_weight(FontWeight::NORMAL).text_color(ch.text_muted).child(s))),
        )
        .into_any_element()
}

/// Container for [`segment`]s, styled like the explorer's view switch.
pub fn segmented(cx: &App) -> Div {
    let ch = chrome(cx);
    h_flex().flex_none().gap(px(2.)).p(px(3.)).rounded(px(8.)).bg(ch.segmented_bg).border_1().border_color(ch.hairline)
}

/// One option of a segmented control; attach `on_click` to it.
pub fn segment(id: impl Into<ElementId>, label: impl Into<SharedString>, active: bool, cx: &App) -> Stateful<Div> {
    let ch = chrome(cx);
    div()
        .id(id)
        .h(px(24.))
        .px(px(11.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .text_size(px(12.))
        .when(active, |d| d.bg(ch.segmented_active).shadow_xs().text_color(ch.text_strong).font_weight(FontWeight::MEDIUM))
        .when(!active, |d| d.text_color(ch.text_muted).hover(|s| s.bg(ch.hover_strong).text_color(ch.text)))
        .child(label.into())
}

/// A small label above a control.
pub fn field(label: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    let ch = chrome(cx);
    v_flex()
        .gap(px(6.))
        .child(div().text_size(px(11.)).font_weight(FontWeight::MEDIUM).text_color(ch.text_muted).child(label.into()))
        .child(control)
}

/// Secondary explanatory text.
pub fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    let ch = chrome(cx);
    div().text_size(px(11.)).line_height(px(16.)).text_color(ch.text_faint).child(text.into())
}

/// A grouped card for related rows or content.
pub fn card(cx: &App) -> Div {
    let ch = chrome(cx);
    v_flex()
        .rounded(px(10.))
        .border_1()
        .border_color(ch.hairline)
        .bg(if ch.dark { rgba(0xffffff05) } else { rgba(0xffffffff) })
}

/// A settings-style row inside a [`card`]: title and hint on the left, the
/// control on the right; rows after the first get a hairline divider.
pub fn row(label: impl Into<SharedString>, hint_text: impl Into<SharedString>, control: impl IntoElement, first: bool, ch: &Chrome) -> Div {
    h_flex()
        .gap(px(16.))
        .px(px(14.))
        .py(px(11.))
        .justify_between()
        .when(!first, |d| d.border_t_1().border_color(ch.hairline))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(3.))
                .child(div().text_size(px(13.)).text_color(ch.text).child(label.into()))
                .child(div().text_size(px(11.)).text_color(ch.text_faint).child(hint_text.into())),
        )
        .child(div().flex_none().child(control))
}

/// Rows with dividers, in a card.
pub fn rows(items: Vec<(SharedString, SharedString, AnyElement)>, cx: &App) -> Div {
    let ch = chrome(cx);
    card(cx).children(items.into_iter().enumerate().map(|(i, (l, h, c))| row(l, h, c, i == 0, &ch)))
}

/// A section caption above a card.
pub fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    let ch = chrome(cx);
    div().pt(px(4.)).text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_faint).child(text.into())
}

/// An inline error or warning message.
pub fn notice(text: impl Into<SharedString>, error: bool, cx: &App) -> Div {
    let ch = chrome(cx);
    let color = if error { rgba(0xf07178ff) } else { rgba(0xe5b567ff) };
    h_flex()
        .gap(px(8.))
        .px(px(12.))
        .py(px(9.))
        .rounded(px(8.))
        .bg(gpui_kit::Hsla::from(color).opacity(0.1))
        .border_1()
        .border_color(gpui_kit::Hsla::from(color).opacity(0.25))
        .text_size(px(12.))
        .text_color(ch.text)
        .child(crate::icons::icon(if error { IconName::CircleX } else { IconName::TriangleAlert }).size(px(14.)).text_color(color))
        .child(div().flex_1().min_w_0().child(text.into()))
}

/// Right-aligned action row at the bottom of a dialog body.
pub fn footer() -> Div {
    h_flex().pt(px(6.)).gap(px(8.)).justify_end()
}

/// Keyboard shortcut chip.
pub fn kbd(text: impl Into<SharedString>, cx: &App) -> Div {
    let ch = chrome(cx);
    div()
        .flex_none()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(5.))
        .border_1()
        .border_color(ch.hairline)
        .bg(ch.hover)
        .text_size(px(11.))
        .text_color(ch.text)
        .whitespace_nowrap()
        .child(text.into())
}

/// Key/value rows in a card (properties, details).
pub fn details(items: Vec<(&'static str, String)>, cx: &App) -> Div {
    details_with_values(items.into_iter().map(|(k, v)| (k, div().child(v).into_any_element())).collect(), cx)
}

/// Key/value rows with custom value elements, such as progress indicators.
pub fn details_with_values(items: Vec<(&'static str, AnyElement)>, cx: &App) -> Div {
    let ch = chrome(cx);
    card(cx).children(items.into_iter().enumerate().map(|(i, (k, v))| {
        h_flex()
            .px(px(14.))
            .py(px(9.))
            .gap(px(16.))
            .items_start()
            .when(i > 0, |d| d.border_t_1().border_color(ch.hairline))
            .child(div().w(px(110.)).flex_none().text_size(px(12.)).text_color(ch.text_faint).child(k))
            .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(ch.text).child(v))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::{Root, WindowExt};
    use gpui_kit::{AppContext, Context, CursorStyle, Render, TestAppContext, Window};
    use std::{cell::Cell, rc::Rc};

    struct DialogHost;

    impl Render for DialogHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    #[gpui_kit::test]
    fn shared_close_button_has_pointer_cursor(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let mut button = close_button(gpui_kit::base::Button::new("close"), chrome(cx));
            assert_eq!(button.style().mouse_cursor, Some(CursorStyle::PointingHand));
        });
    }

    #[gpui_kit::test]
    fn shared_close_button_cancels_dialog(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| DialogHost);
            Root::new(view, window, cx)
        });
        let cancellations = Rc::new(Cell::new(0));
        let count = cancellations.clone();
        cx.update(|window, cx| {
            window.open_dialog(cx, move |base, _, cx| {
                let count = count.clone();
                dialog(base, IconName::Settings, "Settings", None, 500., cx)
                    .on_cancel(move |_, _, _| {
                        count.set(count.get() + 1);
                        true
                    })
            });
        });
        cx.run_until_parked();
        let close = cx.debug_bounds("dialog-close").expect("dialog has a Close button").center();
        cx.simulate_click(close, gpui_kit::Modifiers::default());
        cx.run_until_parked();
        assert_eq!(cancellations.get(), 1);
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn confirmation_buttons_cancel_or_confirm_once(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| DialogHost);
            Root::new(view, window, cx)
        });
        for destructive in [false, true] {
            let confirmations = Rc::new(Cell::new(0));
            for (selector, expected) in [("confirm-cancel", 0), ("confirm-ok", 1)] {
                let count = confirmations.clone();
                cx.update(|window, cx| {
                    crate::dialogs::confirm("Continue?".into(), "Continue", destructive, window, cx, move |_, _| {
                        count.set(count.get() + 1);
                    });
                });
                cx.run_until_parked();
                let button = cx.debug_bounds(selector).expect("confirmation action is visible").center();
                cx.simulate_click(button, gpui_kit::Modifiers::default());
                cx.run_until_parked();
                assert_eq!(confirmations.get(), expected);
                cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
            }
        }
    }
}
