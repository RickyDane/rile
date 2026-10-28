use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme, h_flex, menu::ContextMenuExt, v_flex};
use gpui_kit::{
    AnyElement, AppContext, Context, ElementId, Render, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, linear_color_stop, linear_gradient, prelude::FluentBuilder, px,
};
use rile_core::history::Location;
use rile_core::settings::SidebarSection;

use std::time::Duration;

use gpui_kit::{Animation, AnimationExt, DragMoveEvent, ExternalPaths, pulsating_between};

use crate::icons::icon_for_place;
use crate::pane::{DraggedFiles, Pane};
use crate::theme::chrome;
use crate::views::chrome::{Width, icon_button, mod_key, width_class, window_drag};
use crate::workspace::{Workspace, display_name};

/// Limits for a sidebar width set by dragging its edge.
pub const MIN_WIDTH: f32 = 160.;
pub const MAX_WIDTH: f32 = 420.;

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

pub fn render(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> impl IntoElement {
    let sidebar_width = width(ws, window);
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
    let mut lib: Vec<AnyElement> = vec![
        item("side-home".into(), IconName::House, "Home".into(), at_home, None)
            .on_click(cx.listener(|this, _, w, cx| this.open_location(Location::Home, w, cx)))
            .into_any_element(),
        item("side-photos".into(), IconName::Images, "Photos".into(), at_gallery, None)
            .on_click(cx.listener(|this, _, w, cx| this.open_location(Location::Gallery, w, cx)))
            .into_any_element(),
        item("side-space".into(), IconName::ChartPie, "Space Analyzer".into(), matches!(location, Location::SpaceAnalyzer(_)), None)
            .on_click(cx.listener(|this, _, w, cx| this.open_space_analyzer(w, cx)))
            .into_any_element(),
        item("side-backups".into(), IconName::DatabaseBackup, "Backups".into(), matches!(location, Location::Backups), None)
            .on_click(cx.listener(|this, _, w, cx| this.open_location(Location::Backups, w, cx)))
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
                .on_click(cx.listener(move |this, _, w, cx| this.open_plugin_page(&plugin, &page.id, w, cx)))
                .into_any_element(),
        );
    }
    let (mut fav, mut loc, mut dev, mut net): (Vec<AnyElement>, Vec<AnyElement>, Vec<AnyElement>, Vec<AnyElement>) = Default::default();

    for (ix, path) in ws.settings.favorites.clone().into_iter().enumerate() {
        let active = current.as_ref() == Some(&path);
        let icon = ws.places.iter().find(|p| p.path == path).map(|p| icon_for_place(p.icon)).unwrap_or(IconName::Folder);
        let open = path.clone();
        let remove = path.clone();
        let this = cx.entity().downgrade();
        fav.push(
            drop_into(drop_feedback(item(("fav", ix).into(), icon, display_name(&path), active, None), &path, ("fav-drop", ix)), path.clone(), cx)
                .on_click(cx.listener(move |this, _, w, cx| this.open_path(open.clone(), w, cx)))
                .context_menu(move |menu, _, _| {
                    let this = this.clone();
                    let remove = remove.clone();
                    menu.item(
                        gpui_kit::component::menu::PopupMenuItem::new("Remove from Favorites").on_click(
                            move |_, w, cx| {
                                this.update(cx, |ws, cx| ws.toggle_favorite_path(remove.clone(), w, cx)).ok();
                            },
                        ),
                    )
                })
                .into_any_element(),
        );
    }

    for (ix, place) in ws.places.iter().enumerate() {
        let active = current.as_ref() == Some(&place.path);
        let path = place.path.clone();
        loc.push(
            drop_into(drop_feedback(item(("place", ix).into(), icon_for_place(place.icon), place.name.clone(), active, None), &place.path, ("place-drop", ix)), place.path.clone(), cx)
                .on_click(cx.listener(move |this, _, w, cx| this.open_path(path.clone(), w, cx)))
                .into_any_element(),
        );
    }

    for (ix, vol) in ws.volumes.iter().enumerate() {
        let active = current.as_ref() == Some(&vol.mount_point);
        let path: PathBuf = vol.mount_point.clone();
        let icon = if vol.removable { IconName::Usb } else { IconName::HardDrive };
        let ejectable = rile_services::volumes::can_unmount(vol);
        let mount = vol.mount_point.clone();
        let used = vol.used_fraction();
        let usage_tip = format!(
            "{}: {} free of {} ({:.0}% used)",
            vol.name,
            humansize::format_size(vol.available, humansize::DECIMAL),
            humansize::format_size(vol.total, humansize::DECIMAL),
            used * 100.
        );
        dev.push(
            drop_into(drop_feedback(item(("vol", ix).into(), icon, vol.name.clone(), active, Some((used, usage_color(used, &ch)))), &path, ("vol-drop", ix)), path.clone(), cx)
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
                .into_any_element(),
        );
    }

    if ws.settings.remote_enabled {
        for (ix, conn) in ws.connections.clone().into_iter().enumerate() {
            let active = current_conn.as_deref() == Some(conn.id.as_str());
            let connected = crate::remote::is_connected(&conn.id);
            let label = if conn.name.is_empty() { conn.display() } else { conn.name.clone() };
            let id = conn.id.clone();
            let this = cx.entity().downgrade();
            net.push(
                item(("conn", ix).into(), if connected { IconName::Globe } else { IconName::Server }, label, active, None)
                    .when(connected && !rail, |d| d.child(div().size(px(5.)).rounded_full().bg(ch.success)))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, w, cx| this.open_connection(&id, w, cx)
                    }))
                    .context_menu(move |menu, _, _| {
                        let item = |label: &'static str, f: fn(&mut Workspace, &str, &mut Window, &mut Context<Workspace>)| {
                            let this = this.clone();
                            let id = id.clone();
                            gpui_kit::component::menu::PopupMenuItem::new(label).on_click(move |_, w, cx| {
                                this.update(cx, |ws, cx| f(ws, &id, w, cx)).ok();
                            })
                        };
                        menu.item(item("Connect", |ws, id, w, cx| ws.open_connection(id, w, cx)))
                            .item(item("Edit…", |ws, id, w, cx| {
                                let info = ws.connections.iter().find(|c| c.id == id).cloned();
                                ws.open_connect_dialog(info, w, cx)
                            }))
                            .item(item("Disconnect", |ws, id, w, cx| ws.disconnect(id, w, cx)))
                            .separator()
                            .item(item("Remove", |ws, id, w, cx| ws.remove_connection(id, w, cx)))
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
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
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
        };
        let mut head = h_flex()
            .id(("section-header", section as usize))
            .child(div().flex_1().min_w_0().child(header(section.label())))
            .when(!rail, |d| {
                d.cursor_grab()
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Drag to reorder").build(w, cx))
                    .on_drag(DraggedSection(section), |d, _, _, cx| cx.new(|_| d.clone()))
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
                .children(items),
        );
    }

    let mac = cfg!(target_os = "macos") && !window.is_fullscreen();
    let brand = window_drag(
        h_flex()
            .id("sidebar-header")
            .debug_selector(|| "sidebar-header".into())
            .h(px(crate::views::chrome::TOOLBAR_HEIGHT))
            .flex_none()
            .when(rail, |d| d.justify_center())
            .when(!rail, |d| {
                d.justify_end()
                    .pl(px(12.))
                    .pr(px(if sidebar_width < 180. { 4. } else { 12. }))
                    .gap(px(if sidebar_width < 180. { 2. } else { 8. }))
            })
            // The traffic lights sit in the header's left half on macOS.
            .when(!rail && !mac, |d| d.justify_start())
            .when(!rail, |d| {
                d.child(
                    icon_button("sidebar-toggle", IconName::PanelLeft, format!("Toggle sidebar (⇧{}S)", mod_key()), &ch)
                        .debug_selector(|| "sidebar-toggle".into())
                        .on_click(cx.listener(|this, _, w, cx| this.toggle_sidebar(&crate::actions::ToggleSidebar, w, cx))),
                )
            }),
        cx,
    );

    // Settings and background jobs, at the bottom of the sidebar.
    let jobs = ws.ops.len();
    let jobs_open = ws.jobs_open;
    let footer_button = |id: &'static str, icon: IconName, tip: String, active: bool| {
        div()
            .id(id)
            .relative()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .cursor_pointer()
            .when(active, |d| d.bg(ch.sidebar_active))
            .hover(|s| s.bg(ch.hover_strong))
            .child(crate::icons::icon(icon).size(px(16.)).text_color(if active { ch.sidebar_active_icon } else { ch.sidebar_icon }))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx))
    };
    let buttons = |cx: &mut Context<Workspace>| {
        let settings = footer_button("sidebar-settings", IconName::Settings, format!("Settings ({},)", crate::views::chrome::mod_key()), false)
            .on_click(cx.listener(|this, _, w, cx| this.open_settings(&crate::actions::OpenSettings, w, cx)));
        let jobs_button = footer_button(
            "sidebar-jobs",
            IconName::Activity,
            if jobs == 0 { "Background jobs".to_string() } else { format!("Background jobs: {jobs} running") },
            jobs_open || jobs > 0,
        )
        .when(jobs > 0, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(-3.))
                    .right(px(-3.))
                    .min_w(px(15.))
                    .h(px(15.))
                    .px(px(4.))
                    .rounded_full()
                    .bg(ch.accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(gpui_kit::white())
                    .child(jobs.to_string()),
            )
        })
        .on_click(cx.listener(|this, _, _, cx| {
            let just_closed = this.jobs_closed_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400));
            this.jobs_open = !this.jobs_open && !just_closed;
            cx.notify();
        }));
        (settings, jobs_button)
    };
    let (settings_button, jobs_button) = buttons(cx);
    let rail_buttons = rail.then(|| {
        v_flex().flex_none().items_center().gap(px(4.)).pb(px(14.)).child(settings_button).child(jobs_button)
    });
    let (settings_button, jobs_button) = buttons(cx);
    let footer = (!rail).then(|| {
        h_flex()
            .flex_none()
            .px(px(if sidebar_width < 180. { 10. } else { 16. }))
            .pt(px(6.))
            .pb(px(10.))
            .gap(px(4.))
            .child(settings_button)
            .child(jobs_button)
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
        .child(col)
        .children(footer)
        .children(rail_buttons)
        .children(resize_handle)
}

/// Sidebar entries accept drops: files are moved/copied into `dest`. Drag
/// movement over them drives the hover animation and spring-loading.
fn drop_into(el: gpui_kit::Stateful<gpui_kit::Div>, dest: PathBuf, cx: &mut Context<Workspace>) -> gpui_kit::Stateful<gpui_kit::Div> {
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

/// Usage color: accent, amber above 80% and red above 95%.
fn usage_color(used: f32, ch: &crate::theme::Chrome) -> gpui_kit::Hsla {
    if used >= 0.95 {
        gpui_kit::rgba(0xf07178ff).into()
    } else if used >= 0.8 {
        gpui_kit::rgba(0xe5a54bff).into()
    } else {
        ch.accent
    }
}
