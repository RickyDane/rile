//! Design system glue: maps Rile's appearance settings (mode, accent,
//! density) onto the gpui-component theme.

use gpui_kit::component::{Theme, ThemeMode as UiThemeMode};
use gpui_kit::{App, Global, Hsla, Rgba, Window, hsla, rgba};
use rile_core::settings::{Accent, Settings, ThemeMode};

use crate::views::ViewPrefs;

/// Capacity usage color: the normal color below 80%, amber at 80%, red at 95%.
pub fn usage_color(used: f32, normal: Hsla) -> Hsla {
    if used >= 0.95 {
        rgba(0xf07178ff).into()
    } else if used >= 0.8 {
        rgba(0xe5a54bff).into()
    } else {
        normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_usage_colors_match_thresholds() {
        let normal = hsla(0.6, 0.5, 0.5, 1.);
        for used in [0., 0.77, 0.7999] {
            assert_eq!(usage_color(used, normal), normal);
        }
        for used in [0.8, 0.9, 0.9499] {
            assert_eq!(usage_color(used, normal), rgba(0xe5a54bff).into());
        }
        for used in [0.95, 0.98, 1.] {
            assert_eq!(usage_color(used, normal), rgba(0xf07178ff).into());
        }
    }
}

pub fn apply(settings: &Settings, window: &mut Window, cx: &mut App) {
    match settings.theme {
        ThemeMode::System => Theme::sync_system_appearance(Some(window), cx),
        ThemeMode::Light => Theme::change(UiThemeMode::Light, Some(window), cx),
        ThemeMode::Dark => Theme::change(UiThemeMode::Dark, Some(window), cx),
    }
    apply_accent(settings.accent, cx);
    crate::app_icon::install(settings.app_icon, settings.accent);
    crate::icons::set_icon_set(settings.icon_set);
    crate::art::set_symbol_style(settings.symbol_style);
    crate::art::set_thumbnail_quality(settings.thumbnail_quality);
    let chrome = Chrome::new(settings, cx);
    cx.set_global(chrome);
    apply_native(&chrome, cx);
    // Glass: the window itself blurs what's behind it; translucent chrome
    // (sidebar and toolbar) lets that show through. Floating surfaces stay
    // opaque: window blur does not blur content drawn inside this window.
    window.set_background_appearance(if settings.glass {
        // Windows 11 draws Mica; Windows 10 only has the older blur.
        if windows_11() { gpui_kit::WindowBackgroundAppearance::MicaBackdrop } else { gpui_kit::WindowBackgroundAppearance::Blurred }
    } else {
        gpui_kit::WindowBackgroundAppearance::Opaque
    });
    cx.set_global(ViewPrefs {
        row_height: settings.density.row_height(),
        thumbnails: settings.show_thumbnails,
        icon_size: settings.grid_icon_size,
    });
    // Both spellings of each share, so symlinked paths (/tmp → /private/tmp) match too.
    let shared = settings.nearby.shares.iter().flat_map(|s| [Some(s.path.clone()), std::fs::canonicalize(&s.path).ok()]).flatten().collect();
    cx.set_global(crate::icons::SharedFolders(shared));
    window.refresh();
}

pub fn accent_color(accent: Accent, dark: bool) -> Hsla {
    let (h, s) = accent.hue_sat();
    let l = if dark { 0.62 } else { 0.50 };
    hsla(h / 360., s, l, 1.)
}

fn apply_accent(accent: Accent, cx: &mut App) {
    let dark = Theme::global(cx).is_dark();
    let base = accent_color(accent, dark);
    let shade = |dl: f32, a: f32| Hsla { l: (base.l + dl).clamp(0., 1.), a, ..base };
    let fg = if base.l > 0.6 && accent != Accent::Graphite { hsla(0., 0., 0.08, 1.) } else { hsla(0., 0., 1., 1.) };
    // `Theme::update` (not `global_mut`) so the component tokens buttons,
    // dialogs and tabs paint with follow these colors.
    Theme::update(cx, |theme| {
    let c = &mut theme.colors;
    c.primary = base;
    c.primary_hover = shade(0.06, 1.);
    c.primary_active = shade(-0.06, 1.);
    c.primary_foreground = fg;
    c.button_primary = base;
    c.button_primary_hover = shade(0.06, 1.);
    c.button_primary_active = shade(-0.06, 1.);
    c.button_primary_foreground = fg;
    c.ring = base;
    c.caret = base;
    c.link = base;
    c.link_hover = shade(0.08, 1.);
    c.link_active = shade(-0.08, 1.);
    c.progress_bar = base;
    c.slider_bar = base;
    // `switch` is the unchecked track; the checked one uses `primary`.
    c.switch = if dark { hsla(0., 0., 1., 0.16) } else { hsla(0., 0., 0., 0.14) };
    c.switch_thumb = hsla(0., 0., 1., 1.);
    c.selection = shade(0., 0.35);
    c.list_active = shade(0., if dark { 0.30 } else { 0.18 });
    c.list_active_border = shade(0., 0.7);
    c.table_active = shade(0., if dark { 0.30 } else { 0.18 });
    c.table_active_border = shade(0., 0.7);
    c.drop_target = shade(0., 0.25);
    c.sidebar_primary = base;
    c.sidebar_accent = shade(0., if dark { 0.22 } else { 0.14 });
    });
}

/// Aligns gpui-component surfaces (lists, menus, dialogs, inputs) with the
/// Native chrome so popovers and dialogs read as part of the same app.
fn apply_native(chrome: &Chrome, cx: &mut App) {
    Theme::update(cx, |theme| apply_native_colors(chrome, &mut theme.colors));
    // The window root paints this token over everything; with glass it must
    // let the window's blur through (floating panels paint solid surfaces).
    let root = if chrome.glass { gpui_kit::transparent_black() } else { Theme::global(cx).colors.background };
    let theme = gpui_kit::component::Theme::global_mut(cx);
    theme.tokens.background = root.into();
    // Toasts sit in the bottom-right corner, just above the status bar.
    theme.notification.placement = gpui_kit::Anchor::BottomRight;
    theme.notification.margins.bottom = gpui_kit::px(31. + 12.);
}

fn apply_native_colors(chrome: &Chrome, c: &mut gpui_kit::component::ThemeColor) {
    c.list_active = chrome.selection;
    c.list_active_border = chrome.selection_border;
    c.table_active = chrome.selection;
    c.list_hover = chrome.hover_strong;
    c.popover = chrome.surface_colors().0;
    if !chrome.dark {
        return;
    }
    c.background = chrome.browser;
    c.border = rgba(0xffffff14).into();
    c.sidebar = chrome.sidebar_bottom;
    c.title_bar = chrome.toolbar_bottom;
    c.status_bar = chrome.statusbar;
    c.input = rgba(0xffffff1a).into();
    c.secondary = rgba(0xffffff0d).into();
    c.secondary_hover = rgba(0xffffff16).into();
    c.list = chrome.browser;
    c.list_head = chrome.browser;
    c.overlay = rgba(0x0000008c).into();
    c.muted = rgba(0xffffff0d).into();
    c.accent = rgba(0xffffff12).into();
    c.accent_foreground = chrome.text_strong;
}

/// The Native explorer chrome: the approved dark palette (graphite panels,
/// blue selection), with a light counterpart derived from the system theme.
#[derive(Clone, Copy)]
pub struct Chrome {
    pub dark: bool,
    /// Native chrome is translucent over the blurred window; raised surfaces
    /// remain opaque so in-window content cannot show through them.
    pub glass: bool,
    pub window: Hsla,
    pub sidebar_top: Hsla,
    pub sidebar_bottom: Hsla,
    /// Frosted band behind the sidebar footer (glass only).
    pub sidebar_footer: Option<Hsla>,
    pub sidebar_border: Hsla,
    pub sidebar_text: Hsla,
    pub sidebar_heading: Hsla,
    pub sidebar_icon: Hsla,
    pub sidebar_active: Hsla,
    pub sidebar_active_icon: Hsla,
    pub toolbar_top: Hsla,
    pub toolbar_bottom: Hsla,
    pub toolbar_border: Hsla,
    pub toolbar_text: Hsla,
    pub toolbar_strong: Hsla,
    pub search_bg: Hsla,
    pub search_border: Hsla,
    pub tabstrip: Hsla,
    pub tabstrip_border: Hsla,
    pub tab_text: Hsla,
    pub tab_active: Hsla,
    pub tab_active_hover: Hsla,
    pub tab_active_border: Hsla,
    pub tab_icon: Hsla,
    pub browser: Hsla,
    pub browser_toolbar: Hsla,
    pub hairline: Hsla,
    pub segmented_bg: Hsla,
    pub segmented_active: Hsla,
    pub icon_button: Hsla,
    pub hover: Hsla,
    pub hover_strong: Hsla,
    pub zebra: Hsla,
    pub selection: Hsla,
    pub selection_border: Hsla,
    pub selection_inactive: Hsla,
    pub text: Hsla,
    pub text_strong: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub list_header: Hsla,
    pub inspector: Hsla,
    pub stage_inner: Hsla,
    pub stage_outer: Hsla,
    pub statusbar: Hsla,
    pub card_top: Hsla,
    pub card_bottom: Hsla,
    pub card_border: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub success: Hsla,
}
impl Global for Chrome {}

fn c(hex: u32) -> Hsla {
    rgba(hex).into()
}

fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }.into()
}

impl Chrome {
    /// GPUI's window blur only affects content behind the native window,
    /// not file rows behind an in-window popup. Use solid raised surfaces
    /// for notifications, menus, dialogs and floating panels in every mode.
    pub fn surface_colors(&self) -> (Hsla, Hsla) {
        if self.dark { (c(0x2f2f32ff), c(0x28282bff)) } else { (c(0xffffffff), c(0xf6f7f9ff)) }
    }

    fn new(settings: &Settings, cx: &App) -> Self {
        let mut chrome = Self::solid(settings.accent, cx);
        chrome.glass = settings.glass;
        if settings.glass {
            // Frosted glass: the window blurs what's behind it; panels add a
            // milky tint (lighter than the solid palette) so they read as
            // frosted rather than merely see-through.
            let frost = |c: Hsla, a: f32| {
                let milk = if settings.glass_hue > 0 {
                    hsla(settings.glass_hue.min(360) as f32 / 360., 0.35, if chrome.dark { 0.3 } else { 0.85 }, 1.)
                } else if chrome.dark { hsla(c.h, c.s * 0.6, (c.l + 0.06).min(1.), 1.) } else { hsla(c.h, c.s * 0.5, 0.97, 1.) };
                milk.opacity(a)
            };
            let a = 1. - settings.glass_translucency.min(100) as f32 / 100.;
            chrome.window = chrome.window.opacity(0.);
            chrome.sidebar_top = frost(chrome.sidebar_top, a);
            // Milky veil of the footer's frosted glass, over the faint copies
            // of the items beneath it (see `sidebar::render`).
            let b = chrome.sidebar_bottom;
            chrome.sidebar_footer = Some(frost(hsla(b.h, b.s, if chrome.dark { (b.l + 0.05).min(1.) } else { b.l }, 1.), (a * 0.45).min(0.4)));
            chrome.sidebar_bottom = frost(chrome.sidebar_bottom, a);
            // Muted headings disappear over brighter desktops showing through glass.
            chrome.sidebar_heading = chrome.text_strong;
            chrome.toolbar_top = frost(chrome.toolbar_top, a);
            chrome.toolbar_bottom = frost(chrome.toolbar_bottom, a);
            chrome.tabstrip = frost(chrome.tabstrip, a);
            // Inspector and status bar are not native vibrancy regions.
            // Keep their solid palette rather than exposing sharp content.
        }
        chrome
    }

    /// The opaque palette.
    fn solid(accent: Accent, cx: &App) -> Self {
        let theme = Theme::global(cx);
        let dark = theme.is_dark();
        let base = accent_color(accent, dark);
        // The approved selection blue (#385b83) follows the accent's hue.
        let selection = if dark {
            hsla(base.h, (base.s * 0.42).min(0.42), 0.37, 1.)
        } else {
            hsla(base.h, (base.s * 0.75).min(0.75), 0.86, 1.)
        };
        if dark {
            return Self {
                dark,
                glass: false,
                sidebar_footer: None,
                window: c(0x202020ff),
                sidebar_top: c(0x2d2d30ff),
                sidebar_bottom: c(0x26272bff),
                sidebar_border: c(0xffffff0d),
                sidebar_text: c(0xd2d2d6ff),
                sidebar_heading: c(0xbdbdc1ff),
                // Sidebar/tab icons follow the accent (approved blues at the Blue accent).
                sidebar_icon: hsla(base.h, (base.s * 0.49).min(0.45), 0.70, 1.),
                sidebar_active: c(0x64666d80),
                sidebar_active_icon: hsla(base.h, (base.s * 0.87).min(0.80), 0.79, 1.),
                toolbar_top: c(0x373739ff),
                toolbar_bottom: c(0x303032ff),
                toolbar_border: c(0x171719ff),
                toolbar_text: c(0xbdbdc1ff),
                toolbar_strong: c(0xf3f3f4ff),
                search_bg: c(0x1d1d1f80),
                search_border: c(0xffffff16),
                tabstrip: c(0x29292bff),
                tabstrip_border: c(0x121214ff),
                tab_text: c(0xa6a6aaff),
                tab_active: c(0x505054ff),
                tab_active_hover: c(0x5a5a5eff),
                tab_active_border: c(0xffffff0c),
                tab_icon: hsla(base.h, (base.s * 0.51).min(0.47), 0.80, 1.),
                browser: c(0x202022ff),
                browser_toolbar: c(0x262628ff),
                hairline: c(0xffffff0b),
                segmented_bg: c(0x171719ff),
                segmented_active: c(0x55555aff),
                icon_button: c(0xd1d1d3ff),
                hover: c(0xffffff09),
                hover_strong: c(0xffffff10),
                zebra: c(0xffffff06),
                selection,
                selection_border: hsla(base.h, (base.s * 0.42).min(0.42), 0.61, 0.18),
                selection_inactive: c(0x4a4a50ff),
                text: c(0xe2e2e4ff),
                text_strong: c(0xf1f1f2ff),
                text_muted: c(0x9a9a9eff),
                text_faint: c(0x77777cff),
                list_header: c(0xa1a1a5ff),
                inspector: c(0x29292bff),
                stage_inner: c(0x424247ff),
                stage_outer: c(0x303033ff),
                statusbar: c(0x272729ff),
                card_top: c(0xffffff09),
                card_bottom: c(0xffffff04),
                card_border: c(0xffffff0d),
                accent: hsla(base.h, base.s, 0.68, 1.),
                accent_soft: hsla(base.h, base.s, 0.68, 0.14),
                success: c(0x64ba9dff),
            };
        }
        let t = &theme.colors;
        let fg = t.foreground;
        Self {
            dark,
            glass: false,
            sidebar_footer: None,
            window: t.background,
            sidebar_top: mix(t.sidebar, t.background, 0.15),
            sidebar_bottom: t.sidebar,
            sidebar_border: t.sidebar_border,
            sidebar_text: t.sidebar_foreground,
            sidebar_heading: mix(t.muted_foreground, t.sidebar_foreground, 0.65),
            sidebar_icon: hsla(base.h, (base.s * 0.49).min(0.45), 0.48, 1.),
            sidebar_active: fg.opacity(0.09),
            sidebar_active_icon: base,
            toolbar_top: mix(t.background, fg, 0.015),
            toolbar_bottom: mix(t.background, fg, 0.04),
            toolbar_border: t.border,
            toolbar_text: t.muted_foreground,
            toolbar_strong: fg,
            search_bg: fg.opacity(0.04),
            search_border: t.border,
            tabstrip: mix(t.background, fg, 0.035),
            tabstrip_border: t.border,
            tab_text: t.muted_foreground,
            tab_active: mix(t.background, hsla(0., 0., 0., 1.), 0.075),
            tab_active_hover: mix(mix(t.background, hsla(0., 0., 0., 1.), 0.12), fg, 0.02),
            tab_active_border: t.border,
            tab_icon: hsla(base.h, (base.s * 0.49).min(0.45), 0.48, 1.),
            browser: t.background,
            browser_toolbar: mix(t.background, fg, 0.015),
            hairline: t.border,
            segmented_bg: fg.opacity(0.06),
            segmented_active: t.background,
            icon_button: mix(fg, t.background, 0.25),
            hover: fg.opacity(0.04),
            hover_strong: fg.opacity(0.07),
            zebra: fg.opacity(0.025),
            selection,
            selection_border: base.opacity(0.3),
            selection_inactive: fg.opacity(0.1),
            text: fg,
            text_strong: fg,
            text_muted: t.muted_foreground,
            text_faint: mix(t.muted_foreground, t.background, 0.3),
            list_header: t.muted_foreground,
            inspector: mix(t.background, fg, 0.02),
            stage_inner: mix(t.background, fg, 0.03),
            stage_outer: mix(t.background, fg, 0.08),
            statusbar: mix(t.background, fg, 0.025),
            card_top: fg.opacity(0.03),
            card_bottom: fg.opacity(0.015),
            card_border: t.border,
            accent: base,
            accent_soft: base.opacity(0.12),
            success: hsla(160. / 360., 0.4, 0.45, 1.),
        }
    }
}

/// The current chrome palette (falls back to deriving one before `apply`).
pub fn chrome(cx: &App) -> Chrome {
    cx.try_global::<Chrome>().copied().unwrap_or_else(|| Chrome::new(&Settings::default(), cx))
}

/// Windows 11 is Windows 10.0 with build 22000 or later.
fn windows_11() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};
        let mut info = OSVERSIONINFOW { dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32, ..Default::default() };
        #[allow(deprecated)]
        let ok = unsafe { GetVersionExW(&mut info) }.is_ok();
        ok && info.dwMajorVersion >= 10 && info.dwBuildNumber >= 22000
    }
    #[cfg(not(windows))]
    false
}
