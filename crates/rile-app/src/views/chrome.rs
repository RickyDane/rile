//! Native explorer chrome around the file views: the window toolbar,
//! folder tabs, the browser header (location, view switch, sort, options)
//! and the path bar.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Disableable, Sizable, h_flex,
    button::{Button, ButtonVariants},
    input::Input,
    menu::{DropdownMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::{Animation, AnimationExt};
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement,
    SharedString, Stateful, StatefulInteractiveElement, Div, Styled, Window, div, linear_color_stop, linear_gradient,
    prelude::FluentBuilder, px,
};
use rile_core::history::Location;
use rile_core::settings::ViewKind;
use rile_core::{SortKey, SortOrder};

use crate::actions::*;
use crate::pane::Pane;
use crate::theme::{Chrome, chrome};
use crate::workspace::{Workspace, item_count};

pub const TOOLBAR_HEIGHT: f32 = 52.;
pub const TAB_STRIP_HEIGHT: f32 = 45.;
/// Right edge (window x) of the macOS traffic lights plus breathing room;
/// they sit at x = 20 (see main.rs) and span 52px.
pub const TRAFFIC_LIGHTS_END: f32 = 86.;

/// Left padding of the toolbar: its normal inset, or enough to clear the
/// traffic lights where the visible sidebar is narrower than they are.
pub fn toolbar_inset(base: f32, sidebar_width: f32, traffic_lights: bool) -> f32 {
    if traffic_lights { base.max(TRAFFIC_LIGHTS_END - sidebar_width) } else { base }
}

/// Responsive breakpoints of the approved layout (window width in px).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Width {
    /// < 720: icon-rail sidebar, overlay inspector.
    Phone,
    /// < 920
    Narrow,
    /// < 1150
    Compact,
    Regular,
    /// >= 1600
    Wide,
}

/// Breakpoints for the content of one pane slot, from its own width (the
/// window minus sidebar, inspector and any second pane).
pub fn content_class(width: f32) -> Width {
    match width {
        w if w < 440. => Width::Phone,
        w if w < 640. => Width::Narrow,
        w if w < 900. => Width::Compact,
        w if w < 1250. => Width::Regular,
        _ => Width::Wide,
    }
}

pub fn width_class(window: &Window) -> Width {
    let w: f32 = window.viewport_size().width.into();
    match w {
        w if w < 720. => Width::Phone,
        w if w < 920. => Width::Narrow,
        w if w < 1150. => Width::Compact,
        w if w < 1600. => Width::Regular,
        _ => Width::Wide,
    }
}

/// Makes `el` move the window when dragged and zoom it on double-click,
/// like a native title bar (the app owns title bar dragging).
pub fn window_drag(el: Stateful<Div>, cx: &mut Context<Workspace>) -> Stateful<Div> {
    el.on_mouse_down(
        MouseButton::Left,
        cx.listener(|this, ev: &MouseDownEvent, window, _| {
            if ev.click_count >= 2 {
                this.drag_armed = false;
                window.titlebar_double_click();
            } else {
                this.drag_armed = true;
            }
        }),
    )
    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag_armed = false))
    .on_mouse_move(cx.listener(|this, _, window, _| {
        if std::mem::take(&mut this.drag_armed) {
            window.start_window_move();
        }
    }))
}

pub fn icon_button(id: impl Into<gpui_kit::ElementId>, icon: IconName, tip: impl Into<SharedString>, ch: &Chrome) -> Button {
    Button::new(id).cursor_pointer()
        .ghost()
        .small()
        .icon(crate::icons::icon(icon).size(px(17.)))
        .tooltip(tip)
        .w(px(30.))
        .h(px(30.))
        .rounded(px(7.))
        .text_color(ch.icon_button)
}

fn divider(ch: &Chrome) -> Div {
    div().w(px(1.)).h(px(17.)).mx(px(7.)).flex_none().bg(ch.hairline)
}

pub fn kbd(text: &'static str, ch: &Chrome) -> Div {
    div()
        .flex_none()
        .px(px(4.))
        .py(px(1.))
        .rounded(px(4.))
        .border_1()
        .border_color(ch.hairline)
        .text_size(px(9.))
        .text_color(ch.text_faint)
        .whitespace_nowrap()
        .child(text)
}

pub fn mod_key() -> &'static str {
    if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" }
}

// --- Window toolbar ------------------------------------------------------------

pub fn render_toolbar(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let width = width_class(window);
    let pane = ws.pane().read(cx);
    let (can_back, can_fwd) = (pane.history.can_go_back(), pane.history.can_go_forward());
    let location = pane.location.clone();
    let mac = cfg!(target_os = "macos");
    let lights = mac && !window.is_fullscreen();
    let sidebar_width = if ws.sidebar_visible { crate::sidebar::width(ws, window) } else { 0. };
    let inspector_on = ws.inspector_visible(window);
    let compact = width <= Width::Phone;

    let nav = h_flex()
        .flex_none()
        .when(!ws.sidebar_visible || compact, |d| {
            d.child(
                icon_button("sidebar-toggle", IconName::PanelLeft, format!("Toggle sidebar ({}⌥S)", mod_key()), &ch)
                    .debug_selector(|| "sidebar-toggle".into())
                    .on_click(cx.listener(|this, _, w, cx| this.toggle_sidebar(&ToggleSidebar, w, cx))),
            )
            .child(divider(&ch).when(compact, |d| d.mx(px(3.))))
        })
        .child(
            icon_button("back", IconName::ChevronLeft, format!("Back ({}[)", mod_key()), &ch)
                .disabled(!can_back)
                .on_click(cx.listener(|this, _, w, cx| {
                    this.pane().update(cx, |p, cx| {
                        p.go_back(cx);
                        p.focus(w, cx);
                    })
                })),
        )
        .child(
            Button::new("home-folder")
                .cursor_pointer()
                .ghost()
                .small()
                .tooltip("Home folder")
                .w(px(30.))
                .h(px(30.))
                .p_0()
                .rounded(px(7.))
                .text_color(ch.icon_button)
                // Button::icon overrides explicit icon sizes with the button's size.
                .child(crate::icons::icon(IconName::House).size(px(20.)).flex_none())
                .on_click(cx.listener(|this, _, w, cx| this.go_home_folder(&GoHomeFolder, w, cx))),
        )
        .child(
            icon_button("fwd", IconName::ChevronRight, format!("Forward ({}])", mod_key()), &ch)
                .disabled(!can_fwd)
                .on_click(cx.listener(|this, _, w, cx| {
                    this.pane().update(cx, |p, cx| {
                        p.go_forward(cx);
                        p.focus(w, cx);
                    })
                })),
        );

    let path: Option<AnyElement> = if width <= Width::Narrow && !ws.editing_path {
        None
    } else {
        Some(render_toolbar_path(ws, &location, width, cx))
    };

    let (focused, focus_serial) = ws.search_focus;
    let base_w = if width <= Width::Compact { 190. } else { 234. };
    let grow_w = base_w + 76.;
    let accent = ch.accent;
    let (search_border, search_bg) = (ch.search_border, ch.search_bg);
    let search = h_flex()
        .id("search")
        .flex_none()
        .when(!compact, |d| d.w(px(if focused { grow_w } else { base_w })))
        .when(compact, |d| d.flex_1().min_w(px(90.)))
        .h(px(if focused { 34. } else { 30. }))
        .pl(px(10.))
        .pr(px(6.))
        .gap(px(6.))
        .rounded(px(8.))
        .border_1()
        .border_color(ch.search_border)
        .bg(ch.search_bg)
        .text_size(px(11.))
        .child(crate::icons::icon(IconName::Search).size(px(14.)).text_color(if focused { accent } else { ch.text_faint }))
        .child(div().flex_1().min_w_0().child(Input::new(&ws.search_input).appearance(false).xsmall().cleanable(true)))
        .when(!compact, |d| d.child(kbd(if ws.settings.ai_enabled { "⇧ ↵" } else if mac { "⌘ F" } else { "Ctrl F" }, &ch)))
        .when(ws.settings.ai_enabled, |d| {
            // AI search lives in the search field: describe what you're
            // looking for and the assistant finds it.
            let glyph = crate::icons::apple_intelligence(16., ch.accent, cx);
            d.child(
                div()
                    .id("ai-search")
                    .flex_none()
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(ch.hover_strong))
                    .child(glyph)
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Intelligent search (Shift+Enter)").build(w, cx))
                    .on_click(cx.listener(|this, _, w, cx| this.ai_from_search(w, cx))),
            )
        });
    // Focus grows and highlights the field; leaving shrinks it back.
    let focus_style = move |el: gpui_kit::Stateful<gpui_kit::Div>, p: f32| {
        let el = if compact { el } else { el.w(px(base_w + (grow_w - base_w) * p)) };
        el.h(px(30. + 4. * p))
            .border_color(search_border.blend(accent.opacity(0.85 * p)))
            .bg(search_bg.blend(accent.opacity(0.06 * p)))
            .shadow(vec![gpui_kit::BoxShadow {
                color: accent.opacity(0.35 * p),
                offset: gpui_kit::point(px(0.), px(0.)),
                blur_radius: px(12. * p),
                spread_radius: px(2. * p),
                inset: false,
            }])
    };
    let search: AnyElement = if focus_serial == 0 {
        search.into_any_element()
    } else {
        search
            .with_animation(
                ("search-focus", focus_serial),
                Animation::new(std::time::Duration::from_millis(240)).with_easing(gpui_kit::ease_out_quint()),
                move |el, t| focus_style(el, if focused { t } else { 1. - t }),
            )
            .into_any_element()
    };

    h_flex()
        .id("toolbar")
        .h(px(TOOLBAR_HEIGHT))
        .flex_none()
        .pl(px(toolbar_inset(if compact { 9. } else { 14. }, sidebar_width, lights)))
        .pr(px(if compact { 9. } else { 20. }))
        .gap(px(if compact { 5. } else { 14. }))
        .bg(linear_gradient(180., linear_color_stop(ch.toolbar_top, 0.), linear_color_stop(ch.toolbar_bottom, 1.)))
        .border_b_1()
        .border_color(ch.toolbar_border)
        .child(nav)
        .children(path)
        // Free space moves the window, like a native title bar.
        .child(window_drag(div().id("toolbar-drag").flex_1().min_w(px(8.)).h_full(), cx))
        .when_some(ws.updates.available.as_ref().map(|r| r.version.clone()), |d, v| {
            d.child(
                Button::new("update").cursor_pointer()
                    .primary()
                    .xsmall()
                    .label(format!("Update {v}"))
                    .on_click(cx.listener(|this, _, w, cx| this.open_update_dialog(w, cx))),
            )
        })
        .child(search)
        .child(
            icon_button("inspector-toggle", IconName::PanelRight, format!("Inspector (⇧{}P)", mod_key()), &ch)
                .when(inspector_on, |b| b.text_color(ch.accent))
                .on_click(cx.listener(|this, _, w, cx| this.toggle_preview_panel(&TogglePreviewPanel, w, cx))),
        )
        .into_any_element()
}

/// Volume that contains `path` (longest matching mount point).
pub fn volume_for<'a>(ws: &'a Workspace, path: &Path) -> Option<&'a rile_core::places::Volume> {
    ws.volumes.iter().filter(|v| path.starts_with(&v.mount_point)).max_by_key(|v| v.mount_point.as_os_str().len())
}

/// Crumbs for a location: (label, target, icon). The root is the volume
/// (or server) name.
pub fn crumbs(ws: &Workspace, location: &Location) -> Vec<(String, Option<Location>, IconName)> {
    match location {
        Location::Home => vec![("Home".into(), Some(Location::Home), IconName::House)],
        Location::Gallery => vec![("Photos".into(), Some(Location::Gallery), IconName::Images)],
        Location::Remote { conn, path } => {
            let dir = PathBuf::from(path);
            let mut v: Vec<_> = dir
                .ancestors()
                .map(|p| {
                    let (name, icon) = match p.file_name() {
                        Some(n) => (n.to_string_lossy().into_owned(), IconName::Folder),
                        None => (crate::remote::label(conn), IconName::Globe),
                    };
                    (name, Some(Location::Remote { conn: conn.clone(), path: crate::remote::path_str(p) }), icon)
                })
                .collect();
            v.reverse();
            v
        }
        Location::Dir(dir) => {
            let volume = volume_for(ws, dir);
            let home = dirs::home_dir();
            let mut v = Vec::new();
            for p in dir.ancestors() {
                if let Some(vol) = volume.filter(|vol| vol.mount_point == p) {
                    let icon = if vol.removable { IconName::Usb } else { IconName::HardDrive };
                    v.push((vol.name.clone(), Some(Location::Dir(p.to_path_buf())), icon));
                    break;
                }
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
                let icon = if home.as_deref() == Some(p) { IconName::House } else { IconName::Folder };
                v.push((name, Some(Location::Dir(p.to_path_buf())), icon));
            }
            v.reverse();
            v
        }
    }
}

fn render_toolbar_path(ws: &Workspace, location: &Location, width: Width, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    if ws.editing_path {
        return div().flex_1().min_w(px(160.)).max_w(px(560.)).child(Input::new(&ws.path_input).small()).into_any_element();
    }
    let mut all = crumbs(ws, location);
    if matches!(location, Location::Home) {
        // The Overview sits above any folder: "This Mac > Home".
        let root = ws.volumes.first().map(|v| v.name.clone()).unwrap_or_else(|| "This Mac".into());
        all.insert(0, (root, ws.volumes.first().map(|v| Location::Dir(v.mount_point.clone())), IconName::HardDrive));
    }
    // Root, then the deepest folders: "Macintosh HD › Projects › rile".
    let keep = if width <= Width::Compact { 2 } else { 3 };
    let shown: Vec<_> = if all.len() > keep { std::iter::once(all[0].clone()).chain(all[all.len() - (keep - 1)..].iter().cloned()).collect() } else { all };
    let skip_root = width <= Width::Compact && shown.len() > 2;
    let last = shown.len().saturating_sub(1);
    h_flex()
        .id("toolbar-path")
        .min_w_0()
        .flex_shrink(1.)
        .h(px(30.))
        .gap(px(6.))
        .text_size(px(11.))
        .overflow_hidden()
        .cursor_text()
        .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(format!("Go to folder ({}L)", mod_key())).build(w, cx))
        .on_click(cx.listener(|this, _, w, cx| this.edit_path(&EditPath, w, cx)))
        .children(shown.into_iter().enumerate().filter(|(ix, _)| !(skip_root && *ix == 0)).map(|(ix, (name, target, _))| {
            let current = ix == last;
            h_flex()
                .flex_none()
                .gap(px(6.))
                .when(ix > 0 && !(skip_root && ix == 1), |d| d.child(crate::icons::icon(IconName::ChevronRight).size(px(12.)).text_color(ch.text_faint)))
                .child(
                    div()
                        .id(("crumb", ix))
                        .px(px(2.))
                        .py(px(5.))
                        .rounded(px(4.))
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .text_color(if current { ch.toolbar_strong } else { ch.toolbar_text })
                        .when(current, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .hover(|s| s.text_color(ch.toolbar_strong))
                        .child(name)
                        .when_some(target, |d, target| {
                            d.on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.open_location(target.clone(), w, cx)
                            }))
                        }),
                )
        }))
        .into_any_element()
}

// --- Tabs ----------------------------------------------------------------------

pub fn render_tab_strip(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let phone = width_class(window) <= Width::Phone;
    let active = ws.active_tab();
    let tabs: Vec<(Location, bool)> = ws.slot_tabs().iter().map(|p| (p.read(cx).location.clone(), p.read(cx).loading)).collect();
    let count = tabs.len();
    let items = tabs.into_iter().enumerate().map(|(ix, (location, _))| {
        let is_active = ix == active;
        let icon = match &location {
            Location::Home => IconName::House,
            Location::Gallery => IconName::Images,
            Location::Remote { .. } => IconName::Globe,
            Location::Dir(_) => IconName::Folder,
        };
        let title = crate::workspace::tab_title(&location);
        let group: SharedString = format!("tab-{ix}").into();
        h_flex()
            .id(("tab", ix))
            .group(group.clone())
            .h_full()
            .flex_none()
            .min_w(px(if phone { 123. } else { 160. }))
            .max_w(px(if phone { 150. } else { 210. }))
            .pl(px(if phone { 8. } else { 11. }))
            .pr(px(4.))
            .gap(px(8.))
            .rounded(px(7.))
            .border_1()
            .cursor_pointer()
            .text_size(px(11.))
            .when(is_active, |d| {
                d.bg(ch.tab_active)
                    .border_color(ch.tab_active_border)
                    .shadow_xs()
                    .text_color(ch.text_strong)
                    .hover(|s| s.bg(ch.tab_active_hover))
            })
            .when(!is_active, |d| d.border_color(gpui_kit::transparent_black()).text_color(ch.tab_text).hover(|s| s.bg(ch.hover)))
            .tooltip({
                let tip = match &location {
                    Location::Dir(p) => p.display().to_string(),
                    other => crate::workspace::tab_title(other),
                };
                move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
            })
            .on_click(cx.listener(move |this, _, w, cx| this.activate_tab(ix, w, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    this.close_tab(ix, w, cx);
                }),
            )
            .child(crate::icons::icon(icon).size(px(14.)).text_color(ch.tab_icon))
            .child(div().flex_1().min_w_0().truncate().child(title))
            .child(
                div()
                    .id(("tab-close", ix))
                    .cursor_pointer()
                    .size(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .when(!is_active && !phone, |d| d.invisible().group_hover(group.clone(), |s| s.visible()))
                    .hover(|s| s.bg(ch.hover_strong))
                    .child(crate::icons::icon(IconName::X).size(px(11.)).text_color(ch.text_muted))
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if count > 1 { "Close tab (⌘W)" } else { "Back to Home" }).build(w, cx)
                    })
                    .on_click(cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.close_tab(ix, w, cx);
                    })),
            )
    });
    h_flex()
        .id("tab-strip")
        .h(px(TAB_STRIP_HEIGHT))
        .flex_none()
        .px(px(6.)) // Same inset as top and bottom.
        .py(px(6.))
        .gap(px(6.))
        .bg(ch.tabstrip)
        .border_b_1()
        .border_color(ch.tabstrip_border)
        .child(h_flex().id("tabs").h_full().min_w_0().flex_shrink(1.).gap(px(6.)).overflow_x_scroll().children(items))
        .child(
            Button::new("new-tab").cursor_pointer()
                .ghost()
                .small()
                .icon(crate::icons::icon(IconName::Plus).size(px(16.)))
                .tooltip(format!("New tab ({}T)", mod_key()))
                .size(px(32.))
                .rounded(px(8.))
                .text_color(ch.icon_button)
                .on_click(cx.listener(|this, _, w, cx| this.new_tab(&NewTab, w, cx))),
        )
        .child(window_drag(div().id("tabs-drag").flex_1().h_full(), cx))
        .into_any_element()
}

// --- Browser (one pane slot) ----------------------------------------------------

/// A pane slot: location header with view controls, the file view and the
/// path bar.
pub fn render_browser(ws: &mut Workspace, pane: Entity<Pane>, slot: usize, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let width = content_class(ws.slot_width(slot, window));
    // Narrow slots keep the title, view switch and "…"; the rest moves into "…".
    let tight = width <= Width::Narrow;
    let p = pane.read(cx);
    let location = p.location.clone();
    let (view, sort_key, sort_order) = (p.view, p.sort_key, p.sort_order);
    let count = p.search.as_ref().map(|_| format!("{} found", p.total_count())).unwrap_or_else(|| item_count(p.total_count()));
    let can_create = p.dir_path().is_some() && p.search.is_none();
    let focus = p.focus_handle.clone();
    let slot_active = slot == ws.active_pane || !ws.dual;
    let title = crate::workspace::tab_title(&location);
    let heading_icon = if matches!(location, Location::Remote { .. }) { IconName::Globe } else { IconName::Folder };
    let weak = pane.downgrade();
    let show_hidden = ws.settings.show_hidden;
    let dual = ws.dual;

    let view_button = |id: &'static str, icon: IconName, kind: ViewKind, tip: String, cx: &mut Context<Workspace>| {
        let active = view == kind;
        div()
            .id(id)
            .w(px(if width <= Width::Phone { 25. } else { 28. }))
            .h(px(25.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .when(active, |d| d.bg(ch.segmented_active).shadow_xs())
            .when(!active, |d| d.hover(|s| s.bg(ch.hover_strong)))
            .child(crate::icons::icon(icon).size(px(15.)).text_color(if active { ch.text_strong } else { ch.icon_button }))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx))
            .on_click(cx.listener(move |this, _, w, cx| {
                this.set_active_pane(slot, w, cx);
                this.set_view(kind, w, cx)
            }))
    };
    let m = mod_key();
    let switch = h_flex()
        .flex_none()
        .gap(px(2.))
        .p(px(3.))
        .rounded(px(8.))
        .bg(ch.segmented_bg)
        .border_1()
        .border_color(ch.hairline)
        .child(view_button("view-grid", IconName::LayoutGrid, ViewKind::Grid, format!("Icons ({m}2)"), cx))
        .child(view_button("view-list", IconName::LayoutList, ViewKind::List, format!("List ({m}1)"), cx))
        .child(view_button("view-cols", IconName::Columns3, ViewKind::Columns, format!("Columns ({m}3)"), cx));

    let sort_menu = {
        let weak = weak.clone();
        icon_button(("sort", slot), IconName::ArrowDownWideNarrow, "Sort", &ch).dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
            let item = |label: &'static str, key: SortKey| {
                let weak = weak.clone();
                PopupMenuItem::new(label).checked(sort_key == key).on_click(move |_, _, cx| {
                    weak.update(cx, |p, cx| {
                        if p.sort_key != key {
                            p.set_sort(key, cx)
                        }
                    })
                    .ok();
                })
            };
            let flip = {
                let weak = weak.clone();
                PopupMenuItem::new(if sort_order == SortOrder::Ascending { "Ascending ↑" } else { "Descending ↓" }).on_click(move |_, _, cx| {
                    weak.update(cx, |p, cx| {
                        let key = p.sort_key;
                        p.set_sort(key, cx)
                    })
                    .ok();
                })
            };
            menu.label("Sort by")
                .item(item("Name", SortKey::Name))
                .item(item("Date Modified", SortKey::Modified))
                .item(item("Size", SortKey::Size))
                .item(item("Kind", SortKey::Kind))
                .separator()
                .item(flip)
        })
    };

    let more_menu = {
        let focus = focus.clone();
        let weak = weak.clone();
        icon_button(("more", slot), IconName::Ellipsis, "More options", &ch).dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
            let mut menu = menu.action_context(focus.clone());
            if tight {
                menu = menu.menu_with_disabled("New Folder", Box::new(NewFolder), !can_create).separator().label("Sort by");
                for (label, key) in [("Name", SortKey::Name), ("Date Modified", SortKey::Modified), ("Size", SortKey::Size), ("Kind", SortKey::Kind)] {
                    let weak = weak.clone();
                    menu = menu.item(PopupMenuItem::new(label).checked(sort_key == key).on_click(move |_, _, cx| {
                        weak.update(cx, |p, cx| p.set_sort(key, cx)).ok();
                    }));
                }
                menu = menu.separator();
            }
            menu.menu("Copy Folder Path", Box::new(CopyPath))
                .menu("Open in New Tab", Box::new(OpenInNewTab))
                .menu_with_check("Show Hidden Files", show_hidden, Box::new(ToggleHidden))
                .menu_with_check("Dual Pane", dual, Box::new(ToggleDualPane))
                .separator()
                .menu("Search Subfolders…", Box::new(AdvancedSearch))
                .menu("Folder Properties", Box::new(ShowProperties))
                .menu("Refresh", Box::new(Refresh))
                .separator()
                .menu("Toggle Light / Dark", Box::new(ToggleTheme))
                .menu("Keyboard Shortcuts", Box::new(ShowShortcuts))
                .menu("Check for Updates…", Box::new(CheckForUpdates))
                .menu("Settings…", Box::new(OpenSettings))
        })
    };

    let header = h_flex()
        .id(("browser-toolbar", slot))
        .h(px(if width <= Width::Narrow { 46. } else { 54. }))
        .flex_none()
        .px(px(match width {
            Width::Phone | Width::Narrow => 13.,
            Width::Compact => 18.,
            _ => 26.,
        }))
        .gap(px(12.))
        .justify_between()
        .bg(ch.browser_toolbar)
        .border_b_1()
        .border_color(ch.hairline)
        .child(
            h_flex()
                .flex_1()
                .min_w(px(64.))
                .gap(px(10.))
                .when(width > Width::Phone, |d| d.child(crate::icons::icon(heading_icon).size(px(19.)).flex_none().text_color(ch.accent)))
                .child(
                    div()
                        .min_w(px(40.))
                        .flex_shrink(1.)
                        .truncate()
                        .text_size(px(if width <= Width::Narrow { 15. } else { 17. }))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if slot_active { ch.text_strong } else { ch.text_muted })
                        .child(title),
                )
                .when(width > Width::Narrow, |d| {
                    d.child(div().flex_none().text_size(px(10.)).text_color(ch.text_faint).whitespace_nowrap().child(count))
                }),
        )
        .child(
            h_flex()
                .flex_none()
                .gap(px(if tight { 2. } else { 10. }))
                .when(!tight, |d| {
                    d.child(
                        icon_button(("new-folder", slot), IconName::FolderPlus, format!("New folder (⇧{m}N)"), &ch)
                            .disabled(!can_create)
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.set_active_pane(slot, w, cx);
                                this.new_folder(&NewFolder, w, cx)
                            })),
                    )
                })
                .child(switch)
                .when(!tight, |d| d.child(sort_menu))
                .child(more_menu),
        );

    v_flex()
        .id(("browser", slot))
        .size_full()
        .bg(ch.browser)
        // Any press in a slot activates it first, so menus and buttons
        // (whose actions target the active pane) act on this slot.
        .capture_any_mouse_down(cx.listener(move |this, _, w, cx| {
            if this.active_pane != slot {
                this.set_active_pane(slot, w, cx);
            }
        }))
        .child(header)
        .child(div().flex_1().min_h_0().child(pane))
        .child(render_path_bar(ws, &location, slot, width, cx))
        .into_any_element()
}

fn render_path_bar(ws: &Workspace, location: &Location, slot: usize, width: Width, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let mut crumbs = crumbs(ws, location);
    // Long paths keep the volume and the deepest folders: "HD › … › a › b › c";
    // narrow slots keep only the deepest two.
    let keep = match width {
        Width::Phone => 2,
        Width::Narrow => 3,
        _ => 5,
    };
    if width <= Width::Phone && crumbs.len() > keep {
        crumbs.drain(..crumbs.len() - keep);
    } else if crumbs.len() > keep {
        let tail = crumbs.split_off(crumbs.len() - (keep - 2).max(1));
        let collapsed = crumbs.last().and_then(|c| c.1.clone());
        crumbs.truncate(1);
        crumbs.push(("…".into(), collapsed, IconName::Ellipsis));
        crumbs.extend(tail);
    }
    let last = crumbs.len().saturating_sub(1);
    h_flex()
        .id(("path-bar", slot))
        .h(px(35.))
        .flex_none()
        .px(px(20.))
        .gap(px(8.))
        .bg(ch.statusbar)
        .border_t_1()
        .border_color(ch.hairline)
        .overflow_x_scroll()
        .text_size(px(10.))
        .children(crumbs.into_iter().enumerate().map(|(ix, (name, target, icon))| {
            let current = ix == last;
            h_flex()
                .flex_none()
                .gap(px(8.))
                .when(ix > 0, |d| d.child(crate::icons::icon(IconName::ChevronRight).size(px(10.)).text_color(ch.text_faint)))
                .child(
                    h_flex()
                        .id(("path-crumb", ix))
                        .gap(px(6.))
                        .py(px(3.))
                        .px(px(3.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .whitespace_nowrap()
                        .text_color(if current { ch.text } else { ch.text_muted })
                        .hover(|s| s.bg(ch.hover_strong).text_color(ch.text))
                        .when(icon != IconName::Ellipsis, |d| d.child(crate::icons::icon(icon.clone()).size(px(12.))))
                        .child(name)
                        .when_some(target, |d, target| {
                            d.on_click(cx.listener(move |this, _, w, cx| {
                                this.set_active_pane(slot, w, cx);
                                this.open_location(target.clone(), w, cx)
                            }))
                        }),
                )
        }))
        .into_any_element()
}
