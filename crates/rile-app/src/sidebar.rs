use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, menu::{ContextMenuExt, PopupMenu, PopupMenuItem}, spinner::Spinner, v_flex};
use gpui_kit::{
    AnyElement, AppContext, Context, ElementId, Render, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, linear_color_stop, linear_gradient, prelude::FluentBuilder, px,
};
use rile_core::history::Location;
use rile_core::settings::SidebarSection;

use std::time::Duration;

use gpui_kit::{Animation, AnimationExt, DragMoveEvent, ExternalPaths, pulsating_between};

use crate::icons::{icon_for_place, menu_icon};
use crate::pane::{DraggedFiles, Pane};
use crate::theme::{chrome, usage_color};
use crate::views::chrome::{Width, icon_button, width_class, window_drag};
use crate::workspace::{Workspace, display_name};

/// Limits for a sidebar width set by dragging its edge.
pub const MIN_WIDTH: f32 = 160.;
pub const MAX_WIDTH: f32 = 420.;

/// Use explicit destinations, not pane actions: a right-click must never act
/// on the current file selection or navigate before an action is chosen.
fn navigation_menu(
    menu: PopupMenu,
    workspace: gpui_kit::WeakEntity<Workspace>,
    open: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + Clone + 'static,
) -> PopupMenu {
    let this = workspace.clone();
    let open_here = open.clone();
    menu.item(PopupMenuItem::new("Open").icon(menu_icon(IconName::FolderOpen)).on_click(move |_, w, cx| {
        this.update(cx, |ws, cx| open_here(ws, w, cx)).ok();
    }))
    .item(PopupMenuItem::new("Open in New Tab").icon(menu_icon(IconName::Plus)).on_click(move |_, w, cx| {
        workspace.update(cx, |ws, cx| {
            // Preserve the browsing context for pages that initialize from it.
            let location = ws.pane().read(cx).location.clone();
            ws.open_tab(location, w, cx);
            open(ws, w, cx);
        }).ok();
    }))
}

fn path_menu(
    menu: PopupMenu,
    workspace: gpui_kit::WeakEntity<Workspace>,
    path: PathBuf,
    ejectable: bool,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let favorite = workspace.upgrade().is_some_and(|ws| ws.read(cx).settings.favorites.contains(&path));
    let ai = cfg!(feature = "ai") && workspace.upgrade().is_some_and(|ws| ws.read(cx).settings.ai_enabled);
    let directory = path.is_dir();
    let action = |label: &'static str, icon: gpui_kit::component::Icon, run: fn(&mut Workspace, PathBuf, &mut Window, &mut Context<Workspace>)| {
        let this = workspace.clone();
        let target = path.clone();
        PopupMenuItem::new(label).icon(icon).on_click(move |_, w, cx| {
            this.update(cx, |ws, cx| run(ws, target.clone(), w, cx)).ok();
        })
    };
    let open = path.clone();
    let menu = if directory {
        navigation_menu(menu, workspace.clone(), move |ws, w, cx| ws.open_path(open.clone(), w, cx))
            .item(action("Open in Other Pane", menu_icon(IconName::Columns2), |ws, path, w, cx| {
                if !ws.dual {
                    ws.set_dual(true, w, cx);
                }
                if let Some(other) = ws.other_pane().cloned() {
                    other.update(cx, |pane, cx| pane.navigate(Location::Dir(path), cx));
                }
            }))
    } else {
        let this = workspace.clone();
        menu.item(PopupMenuItem::new("Open").icon(menu_icon(IconName::ExternalLink)).on_click(move |_, w, cx| {
            this.update(cx, |ws, cx| ws.open_path(open.clone(), w, cx)).ok();
        }))
    };
    let menu = menu
        .when(directory, |menu| {
            menu.separator()
                .item(action("Open in Terminal", menu_icon(IconName::SquareTerminal), |ws, path, w, cx| ws.open_terminal_at(path, w, cx)))
                .item(action("Analyze Space", menu_icon(IconName::ChartPie), |ws, path, w, cx| ws.analyze_folder(path, w, cx)))
        })
        .when(ai, |menu| {
            menu.separator().item(action("Summarize with AI", crate::icons::ai_icon(), |ws, path, w, cx| ws.summarize_paths(vec![path], w, cx)))
        });
    let copy = path.clone();
    let toggle = path.clone();
    let this = workspace.clone();
    menu.separator()
        .item(PopupMenuItem::new("Copy Path").icon(menu_icon(IconName::Link)).on_click(move |_, _, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copy.to_string_lossy().into_owned()));
        }))
        .item(PopupMenuItem::new(if favorite { "Remove from Favorites" } else { "Add to Favorites" })
            .icon(menu_icon(IconName::Star)).on_click(move |_, w, cx| {
                this.update(cx, |ws, cx| ws.toggle_favorite_path(toggle.clone(), w, cx)).ok();
            }))
        .when(ejectable, |menu| {
            let workspace = workspace.clone();
            let path = path.clone();
            menu.separator().item(PopupMenuItem::new("Eject").icon(menu_icon(IconName::Eject)).on_click(move |_, w, cx| {
                workspace.update(cx, |ws, cx| ws.unmount(path.clone(), w, cx)).ok();
            }))
        })
        .separator()
        .item(action("Properties", menu_icon(IconName::Info), |ws, path, w, cx| ws.open_properties(vec![path], None, w, cx)))
}

// Wheel events can leave the handle outside its bounds until prepaint.
// Keep the frosted copy in sync with the position the real list will paint.
fn clamped_scroll_y(offset: f32, max_scroll: f32) -> f32 {
    offset.clamp(-max_scroll, 0.)
}

/// The sidebar's width: the icon rail on phone-sized windows, else the
/// width the user dragged it to, else one that suits the window.
pub fn width(ws: &Workspace, window: &Window) -> f32 {
    let class = width_class(window);
    if class == Width::Phone {
        return 60.;
    }
    if let Some(w) = ws.settings.sidebar_width {
        // Never squeeze the files below a usable width.
        let max = (f32::from(window.viewport_size().width) * 0.45).max(MIN_WIDTH);
        return w.clamp(MIN_WIDTH, MAX_WIDTH.min(max));
    }
    match class {
        Width::Phone => 60.,
        Width::Narrow => 177.,
        Width::Compact => 194.,
        Width::Regular => 220.,
        Width::Wide => 235.,
    }
}

/// Drag payload of the sidebar's resize handle.
#[derive(Clone)]
pub struct SidebarResize;

impl Render for SidebarResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

thread_local! {
    /// Set while building the frosted copies of the list under the glass
    /// footer: their drop targets must not track drags (they sit elsewhere).
    static GHOST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The scrolling list of sidebar sections. `ghost` builds a copy for the
/// glass footer's fake blur: same layout, no scrolling, no drag tracking.
fn places(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>, ghost: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    let theme = cx.theme().clone();
    let ch = chrome(cx);
    let rail = width_class(window) <= Width::Phone;
    let location = ws.pane().read(cx).location.clone();
    let current = match &location {
        Location::Dir(p) => Some(p.clone()),
        _ => None,
    };
    let current_conn = match &location {
        Location::Remote { conn, .. } => Some(conn.clone()),
        _ => None,
    };
    let at_home = matches!(location, Location::Home);
    let at_gallery = matches!(location, Location::Gallery);

    let header = |label: &'static str| {
        if rail {
            return div().mx(px(5.)).mt(px(6.)).mb(px(10.)).h(px(1.)).bg(ch.hairline);
        }
        div()
            .px(px(10.))
            .pt(px(10.))
            .pb(px(4.))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ch.sidebar_heading)
            .child(label)
    };

    let accent = ch.accent;
    let hover_target = ws.sidebar_drop_hover.clone();
    // `fill`: share of the row painted behind it (device usage) and its color.
    let item = |id: ElementId, icon: IconName, label: String, active: bool, fill: Option<(f32, gpui_kit::Hsla)>| {
        h_flex()
            .id(id)
            .relative()
            .when_some(fill, |d, (frac, color)| {
                d.child(
                    // GPUI clips to rectangles, so the fill rounds its left corners
                    // while keeping the usage indicator on the right flat.
                    // A darker track shows the whole capacity behind the fill.
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(6.))
                        .bg(if ch.dark { gpui_kit::hsla(0., 0., 0., 0.22) } else { gpui_kit::hsla(0., 0., 0., 0.06) })
                        .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(frac.clamp(0., 1.)))
                            .rounded_l(px(6.))
                            .bg(linear_gradient(90., linear_color_stop(color.opacity(0.10), 0.), linear_color_stop(color.opacity(0.24), 1.)))
                            .border_r_2()
                            .border_color(color.opacity(0.7)),
                    ),
                )
            })
            .h(px(if rail { 34. } else { 28. }))
            .my(px(1.))
            .when(rail, |d| d.justify_center())
            .when(!rail, |d| d.px(px(10.)).gap(px(9.)))
            .rounded(px(6.))
            .text_size(px(12.))
            .cursor_pointer()
            .text_color(ch.sidebar_text)
            .when(active, |d| d.bg(ch.sidebar_active).text_color(ch.text_strong))
            .when(!active, |d| d.hover(|s| s.bg(ch.hover_strong)))
            .child(crate::icons::icon(icon).size(px(16.)).text_color(if active { ch.sidebar_active_icon } else { ch.sidebar_icon }))
            .when(!rail, |d| d.child(div().flex_1().min_w_0().truncate().child(label.clone())))
            .when(rail, |d| {
                d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(w, cx))
            })
    };
    // Items under a drag get a pulsing accent outline, an open-folder icon
    // and a small nudge; holding there springs the folder open.
    let drop_feedback = |el: gpui_kit::Stateful<gpui_kit::Div>, path: &PathBuf, key: (&'static str, usize)| {
        if hover_target.as_ref() != Some(path) {
            return el;
        }
        el.pl_3()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(6.))
                    .border_2()
                    .with_animation(
                        key,
                        Animation::new(Duration::from_millis(900)).repeat().with_easing(pulsating_between(0.35, 1.0)),
                        move |el, t| el.border_color(accent.opacity(t)).bg(accent.opacity(t * 0.22)),
                    ),
            )
            .child(
                div().absolute().right_2().child(
                    crate::icons::icon(IconName::FolderOpen).size_4().text_color(accent).with_animation(
                        (key.0, key.1 + 10_000),
                        Animation::new(Duration::from_millis(900)).repeat().with_easing(pulsating_between(0.6, 1.0)),
                        |el, t| el.opacity(t),
                    ),
                ),
            )
    };

    // Items of each section; headers are added when assembling below.
    let workspace = cx.entity().downgrade();
    let mut lib: Vec<AnyElement> = vec![
        item("side-home".into(), IconName::House, "Home".into(), at_home, None)
            .debug_selector(|| "side-home".into())
            .on_click(cx.listener(|this, _, w, cx| this.open_location(Location::Home, w, cx)))
            .context_menu({ let ws = workspace.clone(); move |menu, _, _| navigation_menu(menu, ws.clone(), |ws, w, cx| ws.open_location(Location::Home, w, cx)) })
            .into_any_element(),
        item("side-photos".into(), IconName::Images, "Photos".into(), at_gallery, None)
            .on_click(cx.listener(|this, _, w, cx| this.open_location(Location::Gallery, w, cx)))
            .context_menu({ let ws = workspace.clone(); move |menu, _, _| navigation_menu(menu, ws.clone(), |ws, w, cx| ws.open_location(Location::Gallery, w, cx)) })
            .into_any_element(),
        item("side-space".into(), IconName::ChartPie, "Space Analyzer".into(), matches!(location, Location::SpaceAnalyzer(_)), None)
            .on_click(cx.listener(|this, _, w, cx| this.open_space_analyzer(w, cx)))
            .context_menu({ let ws = workspace.clone(); move |menu, _, _| navigation_menu(menu, ws.clone(), |ws, w, cx| ws.open_space_analyzer(w, cx)) })
            .into_any_element(),
        item("side-backups".into(), IconName::DatabaseBackup, "Backups".into(), matches!(location, Location::Backups), None)
            .on_click(cx.listener(|this, _, w, cx| this.open_location(Location::Backups, w, cx)))
            .context_menu({ let ws = workspace.clone(); move |menu, _, _| navigation_menu(menu, ws.clone(), |ws, w, cx| ws.open_location(Location::Backups, w, cx)) })
            .into_any_element(),
    ];
    // Plugin pages sit with the built-in library pages.
    for (ix, (plugin, page)) in ws.plugins.pages.clone().into_iter().enumerate() {
        let active = matches!(&location, Location::PluginPage { plugin: p, page: pg, .. } if *p == plugin && *pg == page.id);
        let icon = match page.icon.as_deref() {
            Some("chart" | "disk") => IconName::ChartPie,
            Some("folder") => IconName::Folder,
            _ => IconName::Puzzle,
        };
        lib.push(
            item(("side-plugin-page", ix).into(), icon, page.title.clone().into(), active, None)
                .on_click(cx.listener({
                    let (plugin, page) = (plugin.clone(), page.id.clone());
                    move |this, _, w, cx| this.open_plugin_page(&plugin, &page, w, cx)
                }))
                .context_menu({
                    let ws = workspace.clone();
                    let (plugin, page) = (plugin.clone(), page.id.clone());
                    move |menu, _, _| {
                        let (plugin, page) = (plugin.clone(), page.clone());
                        navigation_menu(menu, ws.clone(), move |ws, w, cx| ws.open_plugin_page(&plugin, &page, w, cx))
                    }
                })
                .into_any_element(),
        );
    }
    let (mut fav, mut loc, mut dev, mut net): (Vec<AnyElement>, Vec<AnyElement>, Vec<AnyElement>, Vec<AnyElement>) = Default::default();

    for (ix, path) in ws.settings.favorites.clone().into_iter().enumerate() {
        let active = current.as_ref() == Some(&path);
        let icon = ws.places.iter().find(|p| p.path == path).map(|p| icon_for_place(p.icon)).unwrap_or(IconName::Folder);
        let open = path.clone();
        let target = path.clone();
        let this = cx.entity().downgrade();
        fav.push(
            drop_into(drop_feedback(item(("fav", ix).into(), icon, display_name(&path), active, None), &path, ("fav-drop", ix)), path.clone(), cx)
                .debug_selector(move || format!("sidebar-favorite-{ix}"))
                .on_click(cx.listener(move |this, _, w, cx| this.open_path(open.clone(), w, cx)))
                .context_menu(move |menu, _, cx| path_menu(menu, this.clone(), target.clone(), false, cx))
                .into_any_element(),
        );
    }

    for (ix, place) in ws.places.iter().enumerate() {
        let active = current.as_ref() == Some(&place.path);
        let path = place.path.clone();
        let target = path.clone();
        let this = workspace.clone();
        loc.push(
            drop_into(drop_feedback(item(("place", ix).into(), icon_for_place(place.icon), place.name.clone(), active, None), &place.path, ("place-drop", ix)), place.path.clone(), cx)
                .on_click(cx.listener(move |this, _, w, cx| this.open_path(path.clone(), w, cx)))
                .context_menu(move |menu, _, cx| path_menu(menu, this.clone(), target.clone(), false, cx))
                .into_any_element(),
        );
    }

    for (ix, vol) in ws.volumes.iter().enumerate() {
        let active = current.as_ref() == Some(&vol.mount_point);
        let path: PathBuf = vol.mount_point.clone();
        let icon = if vol.removable { IconName::Usb } else { IconName::HardDrive };
        let ejectable = rile_services::volumes::can_unmount(vol);
        let mount = vol.mount_point.clone();
        let target = path.clone();
        let this = workspace.clone();
        let used = vol.used_fraction();
        let usage_tip = format!(
            "{}: {} free of {} ({:.0}% used)",
            vol.name,
            humansize::format_size(vol.available, humansize::DECIMAL),
            humansize::format_size(vol.total, humansize::DECIMAL),
            used * 100.
        );
        dev.push(
            drop_into(drop_feedback(item(("vol", ix).into(), icon, vol.name.clone(), active, Some((used, usage_color(used, ch.accent)))), &path, ("vol-drop", ix)), path.clone(), cx)
                .when(!rail, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .text_color(ch.text_muted)
                            .child(humansize::format_size(vol.available, humansize::DECIMAL)),
                    )
                })
                .when(!rail, |d| d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(usage_tip.clone()).build(w, cx)))
                .on_click(cx.listener(move |this, _, w, cx| this.open_path(path.clone(), w, cx)))
                .when(ejectable && !rail, |d| {
                    d.child(
                        div()
                            .id(("eject", ix))
                            .cursor_pointer()
                            .p_0p5()
                            .rounded_sm()
                            .hover(|s| s.bg(ch.hover_strong))
                            .child(crate::icons::icon(IconName::Eject).size_3p5().text_color(ch.text_muted))
                            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Eject").build(window, cx))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.unmount(mount.clone(), w, cx);
                            })),
                    )
                })
                .context_menu(move |menu, _, cx| path_menu(menu, this.clone(), target.clone(), ejectable, cx))
                .into_any_element(),
        );
    }

    if ws.settings.remote_enabled {
        for (ix, conn) in ws.connections.clone().into_iter().enumerate() {
            let active = current_conn.as_deref() == Some(conn.id.as_str());
            let connected = crate::remote::is_connected(&conn.id);
            let connecting = ws.is_source_loading(&conn.id);
            let label = if conn.name.is_empty() { conn.display() } else { conn.name.clone() };
            let id = conn.id.clone();
            let this = cx.entity().downgrade();
            net.push(
                item(("conn", ix).into(), if connected { IconName::Globe } else { IconName::Server }, label, active, None)
                    .when(connecting, |d| d.child(Spinner::new().xsmall()))
                    .when(connected && !connecting && !rail, |d| d.child(div().size(px(5.)).rounded_full().bg(ch.success)))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, w, cx| this.open_connection(&id, w, cx)
                    }))
                    .context_menu(move |menu, _, _| {
                        let item = |label: &'static str, icon: IconName, f: fn(&mut Workspace, &str, &mut Window, &mut Context<Workspace>)| {
                            let this = this.clone();
                            let id = id.clone();
                            gpui_kit::component::menu::PopupMenuItem::element(move |_, _| {
                                h_flex()
                                    .id(label)
                                    .flex_1()
                                    .mx_neg_2()
                                    .px_2()
                                    .h(px(26.))
                                    .gap_1()
                                    .cursor_pointer()
                                    .aria_label(label)
                                    .child(menu_icon(icon).xsmall())
                                    .child(label)
                            }).on_click(move |_, w, cx| {
                                this.update(cx, |ws, cx| f(ws, &id, w, cx)).ok();
                            })
                        };
                        menu.item(item("Connect", IconName::Link, |ws, id, w, cx| ws.connect_only(id, w, cx)))
                            .item(item("Edit…", IconName::Pencil, |ws, id, w, cx| {
                                let info = ws.connections.iter().find(|c| c.id == id).cloned();
                                ws.open_connect_dialog(info, w, cx)
                            }))
                            .item(item("Disconnect", IconName::Eject, |ws, id, w, cx| ws.disconnect(id, w, cx)))
                            .separator()
                            .item(item("Remove", IconName::Trash, |ws, id, w, cx| ws.remove_connection(id, w, cx)))
                    })
                    .into_any_element(),
            );
        }
    }

    // Nearby Rile devices: the Devices page, then paired and discovered ones.
    let mut near: Vec<AnyElement> = Vec::new();
    if ws.settings.nearby.enabled {
        near.push(
            item("side-devices".into(), IconName::Radar, "All Devices".into(), matches!(location, Location::Devices), None)
                .on_click(cx.listener(|this, _, w, cx| this.open_devices(w, cx)))
                .context_menu({ let ws = workspace.clone(); move |menu, _, _| navigation_menu(menu, ws.clone(), |ws, w, cx| ws.open_devices(w, cx)) })
                .into_any_element(),
        );
        for (ix, row) in ws.nearby.rows().into_iter().enumerate() {
            let conn = rile_remote::peer::conn_id(&row.id);
            let active = current_conn.as_deref() == Some(conn.as_str());
            let online = row.online.is_some();
            let ready = online && row.paired && !row.forgot_us;
            let connecting = ws.is_source_loading(&conn);
            let (id, id_drop, id_ext, id_menu) = (row.id.clone(), row.id.clone(), row.id.clone(), row.id.clone());
            let paired = row.paired;
            let this = cx.entity().downgrade();
            let drop_bg = drop_hl_color(&ch);
            let mut el = item(("near", ix).into(), crate::views::devices::kind_icon(row.kind), row.name.clone(), active, None)
                .when(!online, |d| d.opacity(0.55))
                .when(connecting, |d| d.child(Spinner::new().xsmall()))
                .when(ready && !connecting && !rail, |d| d.child(div().size(px(5.)).rounded_full().bg(ch.success)))
                .when(!row.paired && !rail, |d| {
                    d.child(div().flex_none().text_size(px(10.)).text_color(ch.text_muted).child(if ws.nearby.pairing.contains(&row.id) { "Pairing…" } else { "Pair" }))
                })
                .on_click(cx.listener(move |this, _, w, cx| this.open_device(&id, None, w, cx)));
            if ready {
                el = el
                    .drag_over::<DraggedFiles>(move |s, _, _, _| s.bg(drop_bg))
                    .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_bg))
                    .on_drop(cx.listener(move |this, d: &DraggedFiles, w, cx| {
                        if d.conn.is_none() {
                            let paths = d.resolve(cx);
                            this.send_to_device(&id_drop, paths, w, cx);
                        }
                    }))
                    .on_drop(cx.listener(move |this, ext: &ExternalPaths, w, cx| this.send_to_device(&id_ext, ext.paths().to_vec(), w, cx)));
            }
            near.push(
                el.context_menu(move |menu, _, _| {
                    let item = |label: &'static str, f: fn(&mut Workspace, &str, &mut Window, &mut Context<Workspace>)| {
                        let this = this.clone();
                        let id = id_menu.clone();
                        gpui_kit::component::menu::PopupMenuItem::new(label).on_click(move |_, w, cx| {
                            this.update(cx, |ws, cx| f(ws, &id, w, cx)).ok();
                        })
                    };
                    if !paired {
                        return menu.item(item("Pair…", |ws, id, w, cx| ws.pair_with(id, w, cx)));
                    }
                    menu.item(item("Open", |ws, id, w, cx| ws.open_device(id, None, w, cx)))
                        .item(item("Send Files…", |ws, id, w, cx| ws.pick_and_send(id, w, cx)))
                        .separator()
                        .item(item("Unpair…", |ws, id, w, cx| ws.unpair(id, w, cx)))
                })
                .into_any_element(),
            );
        }
    }

    // Sections in the user's order. Each header can be dragged onto another
    // section to move it there; the whole section is the drop target.
    let drop_hl = theme.drop_target;
    let mut col = v_flex()
        .id("places")
        .when(!ghost, |d| d.flex_1().min_h_0().overflow_y_scroll().track_scroll(&ws.sidebar_scroll))
        .px(px(if rail { 8. } else { 10. }))
        .pb_2();
    for section in ws.settings.sidebar_order() {
        let items = match section {
            SidebarSection::Library => std::mem::take(&mut lib),
            SidebarSection::Favorites => std::mem::take(&mut fav),
            SidebarSection::Locations => std::mem::take(&mut loc),
            SidebarSection::Devices => std::mem::take(&mut dev),
            SidebarSection::Network if ws.settings.remote_enabled => std::mem::take(&mut net),
            SidebarSection::Network => continue,
            SidebarSection::Nearby if ws.settings.nearby.enabled => std::mem::take(&mut near),
            SidebarSection::Nearby => continue,
        };
        // The rail has no header labels to click, so it always shows items.
        let collapsed = !rail && ws.settings.sidebar_section_collapsed(section);
        let mut head = h_flex()
            .id(("section-header", section as usize))
            .debug_selector(move || format!("section-header-{}", section as usize))
            .group("section-header")
            .child(div().flex_1().min_w_0().child(header(section.label())))
            .when(!rail, |d| {
                d.cursor_pointer()
                    .tooltip(move |w, cx| {
                        let tip = if collapsed { "Click to expand, drag to reorder" } else { "Click to collapse, drag to reorder" };
                        gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx)
                    })
                    .on_click(cx.listener(move |this, _, w, cx| this.toggle_sidebar_section(section, w, cx)))
                    .on_drag(DraggedSection(section), |d, _, _, cx| cx.new(|_| d.clone()))
                    // Chevron shows on hover, and always while collapsed.
                    .child(
                        div()
                            .flex_none()
                            .mt(px(6.))
                            .mr(px(if section == SidebarSection::Network { 2. } else { 8. }))
                            .when(!collapsed, |d| d.invisible().group_hover("section-header", |s| s.visible()))
                            .child(
                                crate::icons::icon(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                                    .size(px(12.))
                                    .text_color(ch.text_muted),
                            ),
                    )
            });
        match section {
            // Dropping folders on the Favorites header adds them.
            SidebarSection::Favorites => {
                head = head.drag_over::<DraggedFiles>(move |s, _, _, _| s.bg(drop_hl)).on_drop(cx.listener(
                    |this, d: &DraggedFiles, window, cx| {
                        if d.conn.is_some() {
                            return;
                        }
                        for p in d.resolve(cx).into_iter().filter(|p| p.is_dir()) {
                            if !this.settings.favorites.contains(&p) {
                                this.settings.favorites.push(p);
                            }
                        }
                        this.save_settings(window, cx);
                        cx.notify();
                    },
                ));
            }
            SidebarSection::Network if !rail => {
                head = head.child(
                    div()
                        .id("add-connection")
                        .mt(px(6.))
                        .mr(px(8.))
                        .p_0p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(ch.hover_strong))
                        .child(crate::icons::icon(IconName::Plus).size_3p5().text_color(ch.text_muted))
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Connect to Server…").build(window, cx))
                        .on_click(cx.listener(|this, _, w, cx| this.open_connect_dialog(None, w, cx))),
                );
            }
            _ => {}
        }
        col = col.child(
            v_flex()
                .id(("section", section as usize))
                .border_t_1()
                .border_color(gpui_kit::transparent_black())
                // An accent line marks where a dragged section will land.
                .drag_over::<DraggedSection>(move |s, d, _, _| {
                    if d.0 == section { s } else { s.border_color(accent) }
                })
                .on_drop(cx.listener(move |this, d: &DraggedSection, window, cx| {
                    this.move_sidebar_section(d.0, section, window, cx);
                }))
                .pb(px(2.))
                .child(head.context_menu({
                    let this = cx.entity().downgrade();
                    move |menu, _, _| {
                        let this = this.clone();
                        menu.item(gpui_kit::component::menu::PopupMenuItem::new("Reset Sidebar Order").on_click(move |_, w, cx| {
                            this.update(cx, |ws, cx| {
                                ws.settings.sidebar_sections = SidebarSection::ALL.to_vec();
                                ws.save_settings(w, cx);
                                cx.notify();
                            })
                            .ok();
                        }))
                    }
                }))
                .when(!collapsed, |d| d.children(items)),
        );
    }
    col
}

pub fn render(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> impl IntoElement {
    let sidebar_width = width(ws, window);
    let ch = chrome(cx);
    let rail = width_class(window) <= Width::Phone;
    let col = places(ws, window, cx, false);

    let mac = cfg!(target_os = "macos") && !window.is_fullscreen();
    // The drag area is a sibling of the toggle, not its parent: on Windows a
    // caption hitbox containing the button turns its clicks into HTCAPTION.
    let drag = window_drag(div().id("sidebar-header-drag").flex_1().h_full(), cx);
    let toggle = (!rail).then(|| {
        icon_button("sidebar-toggle", IconName::PanelLeft, format!("Toggle sidebar ({})", crate::views::chrome::shortcut("shift-mod-s")), &ch)
            .debug_selector(|| "sidebar-toggle".into())
            .on_click(cx.listener(|this, _, w, cx| this.toggle_sidebar(&crate::actions::ToggleSidebar, w, cx)))
            // Margins rather than header padding, so the rest stays draggable.
            // The right margin matches the button's vertical inset in the
            // header, so it sits evenly in the corner.
            .when(mac, |b| b.mr(px((crate::views::chrome::TOOLBAR_HEIGHT - 30.) / 2.)))
            .when(!mac, |b| b.ml(px(12.)))
    });
    let header = h_flex()
        .id("sidebar-header")
        .debug_selector(|| "sidebar-header".into())
        .h(px(crate::views::chrome::TOOLBAR_HEIGHT))
        .flex_none();
    // The traffic lights sit in the header's left half on macOS, so the
    // toggle goes right of the drag area there and left of it elsewhere.
    let brand = if mac { header.child(drag).children(toggle) } else { header.children(toggle).child(drag) };

    // Settings, at the bottom right of the sidebar (background jobs live in
    // the status bar).
    let settings_button = |cx: &mut Context<Workspace>| {
        div()
            .id("sidebar-settings")
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(ch.hover_strong))
            .child(crate::icons::icon(IconName::Settings).size(px(14.)).text_color(ch.sidebar_icon))
            .tooltip(|w, cx| {
                gpui_kit::component::tooltip::Tooltip::new(format!("Settings ({},)", crate::views::chrome::mod_key())).build(w, cx)
            })
            .on_click(cx.listener(|this, _, w, cx| this.open_settings(&crate::actions::OpenSettings, w, cx)))
    };
    let rail_buttons = rail.then(|| {
        v_flex()
            .flex_none()
            .h(px(crate::views::chrome::FOOTER_HEIGHT))
            .items_center()
            .justify_center()
            .border_t_1()
            .border_color(ch.sidebar_border)
            .child(settings_button(cx))
    });
    // Items scrolled under the footer: a soft shadow above it says so.
    // A wheel event moves the offset unclamped; GPUI only clamps it when the
    // list is laid out again, after this render. Clamp it here the same way
    // so the frosted copies never run past the list's ends.
    let max_scroll = f32::from(ws.sidebar_scroll.max_offset().y);
    let scroll_y = clamped_scroll_y(f32::from(ws.sidebar_scroll.offset().y), max_scroll);
    let more_below = max_scroll > 0.5 && -scroll_y < max_scroll - 0.5;
    // Mirror the footer shadow when content is hidden above the viewport.
    let header_shadow = (ws.settings.sidebar_scroll_shadows && scroll_y < -0.5).then(|| {
        div()
            .debug_selector(|| "sidebar-header-shadow".into())
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(14.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(gpui_kit::black().opacity(0.28), 0.),
                linear_color_stop(gpui_kit::black().opacity(0.), 1.),
            ))
    });
    let footer_shadow = (ws.settings.sidebar_scroll_shadows && more_below).then(|| {
        div()
            .debug_selector(|| "sidebar-footer-shadow".into())
            .absolute()
            .left_0()
            .right_0()
            .bottom_full()
            .h(px(14.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(gpui_kit::black().opacity(0.), 0.),
                linear_color_stop(gpui_kit::black().opacity(0.28), 1.),
            ))
    });
    // Glass: the footer shows the items continuing beneath it through
    // frosted glass. GPUI can't blur in-window content, so it layers a few
    // offset, faint copies of the list (a cheap box blur) under a milky veil.
    let frost_copies = (!rail && more_below)
        .then_some(ch.sidebar_footer)
        .flatten()
        .map(|_| {
            let viewport = ws.sidebar_scroll.bounds().size.height;
            // The list's top, relative to the footer (which starts where the
            // list's viewport ends).
            let top = scroll_y - f32::from(viewport);
            // A Gaussian-weighted disc of taps: rings every 2px out to 10px, each
            // turned against the last so no copy lines up with another, and
            // fainter further out, so the copies melt into one soft blur
            // instead of showing as separate bands.
            const RINGS: [(f32, usize); 6] = [(0., 1), (2., 6), (4., 8), (6., 10), (8., 12), (10., 14)];
            const SIGMA: f32 = 5.;
            let mut taps: Vec<(f32, f32, f32)> = Vec::new();
            for (ring, (r, n)) in RINGS.iter().enumerate() {
                let weight = (-r * r / (2. * SIGMA * SIGMA)).exp();
                for k in 0..*n {
                    let a = (k as f32 / *n as f32 + ring as f32 * 0.17) * std::f32::consts::TAU;
                    // Half-pixel steps keep the glyph cache small.
                    let snap = |v: f32| (v * 2.).round() / 2.;
                    taps.push((snap(r * a.cos()), snap(r * a.sin()), weight));
                }
            }
            // Total ink roughly that of 1.6 full copies.
            let total: f32 = taps.iter().map(|t| t.2).sum();
            GHOST.with(|g| g.set(true));
            let copies: Vec<_> = taps
                .iter()
                .enumerate()
                .map(|(i, (dx, dy, w))| {
                    div()
                        .id(("sidebar-frost", i))
                        .absolute()
                        .left(px(*dx))
                        .top(px(top + dy))
                        .w(px(sidebar_width - 1.))
                        .opacity(w / total * 1.6)
                        .child(places(ws, window, cx, true))
                })
                .collect();
            GHOST.with(|g| g.set(false));
            copies
        });
    let glass = frost_copies.is_some();
    let footer = (!rail).then(|| {
        let bar = h_flex()
            .id("sidebar-footer")
            .relative()
            .flex_none()
            .h(px(crate::views::chrome::FOOTER_HEIGHT))
            .overflow_hidden()
            .justify_end()
            .items_center()
            .px(px(if sidebar_width < 180. { 6. } else { 10. }))
            .border_t_1()
            .border_color(ch.sidebar_border)
            .when_some(frost_copies, |d, copies| {
                d.children(copies)
                    // Milky veil over the copies; it also keeps them from
                    // receiving clicks or hovers.
                    .child(div().id("sidebar-frost-veil").absolute().inset_0().occlude().when_some(ch.sidebar_footer, |d, c| d.bg(c)))
            })
            // Glass: a faint highlight along the top edge.
            .when(ch.sidebar_footer.is_some(), |d| {
                d.border_color(if ch.dark { gpui_kit::white().opacity(0.08) } else { gpui_kit::white().opacity(0.6) })
            })
            .child(settings_button(cx));
        div().relative().flex_none().children(footer_shadow.filter(|_| !glass)).child(bar)
    });

    let resize_handle = (!rail).then(|| {
        div()
            .id("sidebar-resize")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(5.))
            .cursor_col_resize()
            .hover(|s| s.bg(ch.accent.opacity(0.35)))
            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Drag to resize · double-click to reset").build(w, cx))
            .on_drag(SidebarResize, |d, _, _, cx| cx.new(|_| d.clone()))
            .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|this, ev: &gpui_kit::MouseDownEvent, window, cx| {
                if ev.click_count == 2 {
                    this.settings.sidebar_width = None;
                    this.save_settings(window, cx);
                    cx.notify();
                }
            }))
    });
    v_flex()
        .id("sidebar")
        .relative()
        .w(px(sidebar_width))
        .h_full()
        .flex_none()
        .bg(linear_gradient(150., linear_color_stop(ch.sidebar_top, 0.), linear_color_stop(ch.sidebar_bottom, 1.)))
        .border_r_1()
        .border_color(ch.sidebar_border)
        .child(brand)
        .child(v_flex().relative().flex_1().min_h_0().child(col).children(header_shadow))
        .children(footer)
        .children(rail_buttons)
        .children(resize_handle)
}

/// Sidebar entries accept drops: files are moved/copied into `dest`. Drag
/// movement over them drives the hover animation and spring-loading.
fn drop_into(el: gpui_kit::Stateful<gpui_kit::Div>, dest: PathBuf, cx: &mut Context<Workspace>) -> gpui_kit::Stateful<gpui_kit::Div> {
    if GHOST.with(|g| g.get()) {
        return el;
    }
    let dest2 = dest.clone();
    let (h1, h2) = (dest.clone(), dest.clone());
    let hl = cx.theme().drop_target;
    el.drag_over::<DraggedFiles>(move |s, _, _, _| s.bg(hl))
        .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(hl))
        .on_drag_move::<DraggedFiles>(cx.listener(move |this, ev: &DragMoveEvent<DraggedFiles>, window, cx| {
            this.track_drop_hover(&h1, ev.bounds.contains(&ev.event.position), window, cx);
        }))
        .on_drag_move::<ExternalPaths>(cx.listener(move |this, ev: &DragMoveEvent<ExternalPaths>, window, cx| {
            this.track_drop_hover(&h2, ev.bounds.contains(&ev.event.position), window, cx);
        }))
        .on_drop(cx.listener(move |this, d: &DraggedFiles, window, cx| {
            this.clear_drop_hover(cx);
        let paths: Vec<PathBuf> = d.resolve(cx).into_iter().filter(|p| *p != dest && p.parent() != Some(dest.as_path())).collect();
        let copy = Pane::drop_is_copy(&paths, d.conn.as_deref(), &dest, None, window);
        this.transfer_between(paths, d.conn.clone(), dest.clone(), None, copy, window, cx);
    }))
    .on_drop(cx.listener(move |this, ext: &ExternalPaths, window, cx| {
        this.clear_drop_hover(cx);
        this.transfer(ext.paths().to_vec(), dest2.clone(), true, window, cx);
    }))
}

/// Drag payload (and its drag preview) for reordering sidebar sections.
#[derive(Clone)]
pub struct DraggedSection(pub SidebarSection);

impl Render for DraggedSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ch = chrome(cx);
        div()
            .px(px(10.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(ch.tab_active)
            .border_1()
            .border_color(ch.accent.opacity(0.6))
            .shadow_md()
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ch.text_strong)
            .child(self.0.label())
    }
}

fn drop_hl_color(ch: &crate::theme::Chrome) -> gpui_kit::Hsla {
    ch.accent.opacity(0.16)
}

#[cfg(test)]
mod tests {
    use super::clamped_scroll_y;

    #[gpui_kit::test]
    fn sidebar_properties_target_clicked_folder(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{Modifiers, MouseButton, VisualTestContext};
        use rile_core::history::Location;

        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        let target = root.join("Favorite");
        std::fs::create_dir(&target).unwrap();
        let (window, ws) = crate::tests::setup(&root, cx);
        ws.update(cx, |ws, cx| {
            ws.settings.favorites = vec![target.clone()];
            ws.settings.ai_enabled = false;
            cx.notify();
        });
        let mut vcx = VisualTestContext::from_window(window, cx);
        vcx.run_until_parked();
        let favorite = vcx.debug_bounds("sidebar-favorite-0").expect("favorite row").center();
        vcx.simulate_mouse_down(favorite, MouseButton::Right, Modifiers::none());
        vcx.simulate_mouse_up(favorite, MouseButton::Right, Modifiers::none());
        vcx.run_until_parked();
        // Properties must be the last menu item.
        cx.simulate_keystrokes(window, "up enter");
        vcx.run_until_parked();
        cx.update(|cx| {
            let ws = ws.read(cx);
            assert_eq!(ws.pane().read(cx).location, Location::Dir(root));
            assert_eq!(ws.preview.target, Some(target));
            assert_eq!(ws.preview.target_conn, None);
            assert!(ws.preview.properties_only);
            assert!(ws.preview.properties_view.is_some());
        });
    }

    #[gpui_kit::test]
    fn sidebar_menu_opens_destination_in_new_tab(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{Modifiers, MouseButton, VisualTestContext};
        use rile_core::history::Location;

        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        let (window, ws) = crate::tests::setup(&root, cx);
        let mut vcx = VisualTestContext::from_window(window, cx);
        vcx.run_until_parked();
        let home = vcx.debug_bounds("side-home").expect("Home sidebar row").center();
        vcx.simulate_mouse_down(home, MouseButton::Right, Modifiers::none());
        vcx.simulate_mouse_up(home, MouseButton::Right, Modifiers::none());
        vcx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(ws.read(cx).pane().read(cx).location, Location::Dir(root.clone()));
            assert_eq!(ws.read(cx).slot_tabs().len(), 1);
        });

        cx.simulate_keystrokes(window, "down down enter");
        vcx.run_until_parked();
        cx.update(|cx| {
            let ws = ws.read(cx);
            assert_eq!(ws.slot_tabs().len(), 2);
            assert_eq!(ws.pane().read(cx).location, Location::Home);
            assert_eq!(ws.slot_tabs()[0].read(cx).location, Location::Dir(root));
        });
    }

    #[test]
    fn frosted_scroll_stops_at_both_list_ends() {
        assert_eq!(clamped_scroll_y(-125., 100.), -100.);
        assert_eq!(clamped_scroll_y(25., 100.), 0.);
        assert_eq!(clamped_scroll_y(-40., 100.), -40.);
        assert_eq!(clamped_scroll_y(-100., 100.), -100.);
    }

    #[test]
    fn frosted_scroll_stays_at_origin_when_list_fits() {
        for offset in [-25., 0., 25.] {
            assert_eq!(clamped_scroll_y(offset, 0.), 0.);
        }
    }
}
