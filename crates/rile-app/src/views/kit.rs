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
    let glyph = crate::icons::icon(icon).size(px(17.)).text_color(chrome(cx).accent).into_any_element();
    dialog_with_glyph(dialog, glyph, title, subtitle, width, cx)
}

/// [`dialog`] with any glyph in the icon tile, such as the Apple Intelligence symbol.
pub fn dialog_with_glyph(dialog: Dialog, glyph: AnyElement, title: impl Into<SharedString>, subtitle: Option<SharedString>, width: f32, cx: &App) -> Dialog {
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
                .child(header_with_glyph(glyph, title.into(), subtitle, cx))
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
    let glyph = crate::icons::icon(icon).size(px(17.)).text_color(chrome(cx).accent).into_any_element();
    header_with_glyph(glyph, title, subtitle, cx)
}

/// [`header`] with any glyph in the icon tile.
pub fn header_with_glyph(glyph: AnyElement, title: SharedString, subtitle: Option<SharedString>, cx: &App) -> AnyElement {
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
                .child(glyph),
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
    segment_base(id, active, cx).child(label.into())
}

/// A [`segment`] with a leading icon.
pub fn segment_with_icon(id: impl Into<ElementId>, icon: IconName, label: impl Into<SharedString>, active: bool, cx: &App) -> Stateful<Div> {
    segment_base(id, active, cx)
        .gap(px(5.))
        .child(crate::icons::icon(icon).size(px(13.)))
        .child(label.into())
}

fn segment_base(id: impl Into<ElementId>, active: bool, cx: &App) -> Stateful<Div> {
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

/// Button hierarchy from the design guide (docs/design-guide.md, "Buttons").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    /// The main action of a group: accent fill with dark text.
    Primary,
    /// Ordinary supporting actions: neutral control fill.
    Secondary,
    /// Low-emphasis or dismissive actions: no fill until hovered.
    Ghost,
    /// Removes or irreversibly changes data; pair with an explicit label.
    Destructive,
}

/// Fill, text, hover and pressed colors for a [`Tone`].
fn tone_colors(tone: Tone, ch: &Chrome) -> (gpui_kit::Hsla, gpui_kit::Hsla, gpui_kit::Hsla, gpui_kit::Hsla) {
    use gpui_kit::hsla;
    let a = ch.accent;
    let clear = gpui_kit::transparent_black();
    match (tone, ch.dark) {
        // #86baf5 on #15283d at the Blue accent; other accents keep the hue.
        (Tone::Primary, true) => (hsla(a.h, a.s.min(0.85), 0.74, 1.), hsla(a.h, 0.49, 0.16, 1.), hsla(a.h, a.s.min(0.85), 0.79, 1.), hsla(a.h, a.s.min(0.85), 0.68, 1.)),
        (Tone::Primary, false) => (a, hsla(0., 0., 1., 1.), hsla(a.h, a.s, (a.l + 0.05).min(0.9), 1.), hsla(a.h, a.s, (a.l - 0.06).max(0.1), 1.)),
        (Tone::Secondary, true) => (rgba(0x383b41ff).into(), ch.text_strong, rgba(0x43464dff).into(), rgba(0x31343aff).into()),
        (Tone::Secondary, false) => (ch.text.opacity(0.06), ch.text, ch.text.opacity(0.1), ch.text.opacity(0.14)),
        (Tone::Ghost, _) => (clear, ch.text, ch.hover_strong, ch.hover),
        (Tone::Destructive, true) => (rgba(0x533236ff).into(), rgba(0xffb4b4ff).into(), rgba(0x5f393eff).into(), rgba(0x482b2fff).into()),
        (Tone::Destructive, false) => (rgba(0xfde8e8ff).into(), rgba(0xb42318ff).into(), rgba(0xfbd5d5ff).into(), rgba(0xf8c4c4ff).into()),
    }
}

/// A button from the design guide: 34 px (27 px `compact`) high, 8/7 px
/// corners and a semibold 12 px label. Add `.pointer_unless(..)` for
/// disabled states and `.on_click(..)`.
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, tone: Tone, compact: bool, cx: &App) -> gpui_kit::component::button::Button {
    let label = label.into();
    styled_button(gpui_kit::component::button::Button::new(id).label(label), tone, compact, cx)
}

/// [`button`] with a leading glyph (16 px, 6 px from the label), such as the
/// Apple Intelligence symbol on AI actions.
pub fn button_with_glyph(
    id: impl Into<ElementId>,
    glyph: AnyElement,
    label: impl Into<SharedString>,
    tone: Tone,
    compact: bool,
    cx: &App,
) -> gpui_kit::component::button::Button {
    let label = label.into();
    let content = h_flex().gap(px(6.)).items_center().child(glyph).child(div().whitespace_nowrap().child(label.clone()));
    styled_button(gpui_kit::component::button::Button::new(id).accessibility_label(label).child(content), tone, compact, cx)
}

fn styled_button(button: gpui_kit::component::button::Button, tone: Tone, compact: bool, cx: &App) -> gpui_kit::component::button::Button {
    use gpui_kit::component::{Sizable, button::{ButtonCustomVariant, ButtonVariants}};
    let ch = chrome(cx);
    let (bg, fg, hover, active) = tone_colors(tone, &ch);
    let border = match tone {
        Tone::Secondary if ch.dark => Some(gpui_kit::Hsla::from(rgba(0xffffff18))),
        Tone::Secondary => Some(ch.hairline),
        _ => None,
    };
    button
        .xsmall()
        .custom(ButtonCustomVariant::new(cx).color(bg).foreground(fg).hover(hover).active(active).shadow(false))
        // The custom variant paints only a 20% tint of `color`; set the full fill.
        .bg(bg)
        .h(px(if compact { 27. } else { 34. }))
        .px(px(if compact { 9. } else { 13. }))
        .rounded(px(if compact { 7. } else { 8. }))
        .font_weight(FontWeight::SEMIBOLD)
        .when_some(border, |b, color| b.border_1().border_color(color))
        .cursor_pointer()
}

/// Inset container for [`tab`]s (4 px padding, modest gap).
pub fn tabs(cx: &App) -> Div {
    let ch = chrome(cx);
    h_flex()
        .gap(px(4.))
        .p(px(4.))
        .rounded(px(9.))
        .bg(if ch.dark { gpui_kit::Hsla::from(rgba(0x202125ff)) } else { ch.segmented_bg })
        .border_1()
        .border_color(ch.hairline)
}

/// One tab of a [`tabs`] container: a raised neutral fill and bright text
/// when selected, transparent with muted text otherwise. Labels stay
/// visible next to the optional glyph.
pub fn tab(id: impl Into<ElementId>, glyph: Option<AnyElement>, label: impl Into<SharedString>, selected: bool, cx: &App) -> gpui_kit::component::button::Button {
    use gpui_kit::component::{Sizable, button::{ButtonCustomVariant, ButtonVariants}};
    let ch = chrome(cx);
    let label = label.into();
    let fill = if ch.dark { gpui_kit::Hsla::from(rgba(0x474b52ff)) } else { ch.segmented_active };
    let (bg, fg, hover) = if selected { (fill, ch.text_strong, fill) } else { (gpui_kit::transparent_black(), ch.text_muted, ch.hover_strong) };
    gpui_kit::component::button::Button::new(id)
        .xsmall()
        .accessibility_label(label.clone())
        .toggled(selected)
        .custom(ButtonCustomVariant::new(cx).color(bg).foreground(fg).hover(hover).active(if selected { fill } else { ch.hover }).shadow(false))
        .bg(bg)
        .h(px(28.))
        .px(px(10.))
        .rounded(px(5.))
        .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .when(selected && !ch.dark, |b| b.shadow_xs())
        .child(h_flex().gap(px(6.)).items_center().children(glyph).child(div().whitespace_nowrap().child(label)))
        .cursor_pointer()
}

/// A section heading (15 px semibold) with an optional supporting line.
pub fn heading(title: impl Into<SharedString>, description: Option<SharedString>, cx: &App) -> Div {
    let ch = chrome(cx);
    v_flex()
        .gap(px(3.))
        .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child(title.into()))
        .children(description.map(|d| div().text_size(px(12.)).line_height(px(17.)).text_color(ch.text_muted).child(d)))
}

/// State of a [`status`] line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Ready,
    Pending,
    Warning,
    Off,
}

/// A small status line: a colored dot and a label, so the state is not
/// carried by color alone.
pub fn status(text: impl Into<SharedString>, state: Status, cx: &App) -> Div {
    let ch = chrome(cx);
    let dot: gpui_kit::Hsla = match state {
        Status::Ready => ch.success,
        Status::Pending => ch.text_muted,
        Status::Warning => rgba(0xe5b567ff).into(),
        Status::Off => ch.text_faint,
    };
    h_flex()
        .gap(px(6.))
        .min_w_0()
        .text_size(px(11.))
        .text_color(if state == Status::Warning { ch.text } else { ch.text_muted })
        .child(div().flex_none().size(px(7.)).rounded_full().bg(dot))
        .child(div().min_w_0().truncate().child(text.into()))
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
            let mut button = close_button(gpui_kit::base::Button::new("close").cursor_pointer(), chrome(cx));
            assert_eq!(button.style().mouse_cursor, Some(CursorStyle::PointingHand));
        });
    }

    #[gpui_kit::test]
    fn action_cursor_tracks_disabled_state(cx: &mut TestAppContext) {
        use gpui_kit::component::button::{Button, ButtonVariants};
        cx.update(|cx| {
            gpui_kit::init(cx);
            for button in [Button::new("secondary").outline(), Button::new("primary").primary(), Button::new("ghost").ghost()] {
                let mut button = button.pointer_unless(false);
                assert_eq!(button.style().mouse_cursor, Some(CursorStyle::PointingHand));
                button = button.pointer_unless(true);
                assert_eq!(button.style().mouse_cursor, Some(CursorStyle::Arrow));
                button = button.pointer_unless(false);
                assert_eq!(button.style().mouse_cursor, Some(CursorStyle::PointingHand));
            }
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

/// The component library only gives link buttons the pointing hand, so every
/// control adds `.cursor_pointer()` when created; this keeps disabled controls
/// on the arrow.
pub trait PointerUnless: gpui_kit::component::Disableable + Styled + Sized {
    fn pointer_unless(self, disabled: bool) -> Self {
        let this = self.disabled(disabled);
        if disabled { this.cursor_default() } else { this.cursor_pointer() }
    }
}

impl<T: gpui_kit::component::Disableable + Styled> PointerUnless for T {}
