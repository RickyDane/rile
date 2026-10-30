//! The staging shelf: a floating package button in the bottom-right corner
//! that collects files and folders from anywhere ("Add to Shelf", or drop
//! them on it). Added items fly into the button. Clicking it opens a small
//! popup listing what's staged, with actions that send the whole shelf
//! somewhere at once: copy/move into the current folder or the other pane,
//! compress, send to a nearby device, or drag it all out.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, notification::Notification, v_flex, WindowExt};
use gpui_kit::{
    AnyElement, AppContext, Context, ExternalPaths, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, Point,
    StatefulInteractiveElement, Styled, Window, div, point, prelude::FluentBuilder, px,
};

use super::{Workspace, display_name, item_count};
use crate::actions::*;
use crate::pane::DraggedFiles;
use crate::views::kit::{self, Tone};

const FILE_NAME: &str = "shelf.json";
/// Button diameter and its distance from the window's right / bottom edge.
const SIZE: f32 = 50.;
const RIGHT: f32 = 20.;
const FLIGHT: Duration = Duration::from_millis(720);
const STAGGER: Duration = Duration::from_millis(55);
const BUMP: Duration = Duration::from_millis(380);
/// At most this many items fly; the rest land with the last one.
const MAX_FLIGHTS: usize = 6;

fn bottom() -> f32 {
    crate::views::chrome::FOOTER_HEIGHT + 16.
}

/// An item on its way into the shelf button.
pub struct Flight {
    id: usize,
    from: Point<Pixels>,
    start: Instant,
    icon: IconName,
    /// Items this flight represents (the last one carries the overflow).
    count: usize,
}

#[derive(Default)]
pub struct ShelfState {
    /// Staged paths, in the order they were added.
    pub items: Vec<PathBuf>,
    pub open: bool,
    flights: Vec<Flight>,
    next_flight: usize,
    /// Items still in the air (not counted on the badge yet).
    in_air: usize,
    bump: Option<Instant>,
    closed_at: Option<Instant>,
}

impl ShelfState {
    pub fn load() -> Self {
        let items: Vec<PathBuf> = path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self { items: items.into_iter().filter(|p| p.exists()).collect(), ..Default::default() }
    }

    fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if let Ok(bytes) = serde_json::to_vec(&self.items) {
            std::fs::write(path, bytes).ok();
        }
    }

    /// Adds paths not on the shelf yet; returns the ones added.
    fn add(&mut self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let added: Vec<PathBuf> = paths.into_iter().filter(|p| !self.items.contains(p)).collect();
        let mut seen = std::collections::HashSet::new();
        let added: Vec<PathBuf> = added.into_iter().filter(|p| seen.insert(p.clone())).collect();
        self.items.extend(added.iter().cloned());
        self.save();
        added
    }

    /// Drops items that no longer exist (moved or deleted elsewhere).
    fn prune(&mut self) {
        let before = self.items.len();
        self.items.retain(|p| p.exists());
        if self.items.len() != before {
            self.save();
        }
    }
}

fn path() -> Option<PathBuf> {
    rile_core::settings::Settings::config_dir().map(|d| d.join(FILE_NAME))
}

/// Center of the shelf button, in window coordinates.
fn button_center(window: &Window) -> Point<Pixels> {
    let size = window.viewport_size();
    point(size.width - px(RIGHT + SIZE / 2.), size.height - px(bottom() + SIZE / 2.))
}

fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 { 4. * t * t * t } else { 1. - (-2. * t + 2.).powi(3) / 2. }
}

impl Workspace {
    pub(crate) fn add_to_shelf(&mut self, _: &AddToShelf, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane().read(cx).conn().is_some() {
            window.push_notification(Notification::info("The shelf holds files on this computer only"), cx);
            return;
        }
        let paths = self.selection(cx);
        let from = window.mouse_position();
        self.stage(paths, Some(from), window, cx);
    }

    /// Puts `paths` on the shelf. With `from`, they fly there from that point.
    pub fn stage(&mut self, paths: Vec<PathBuf>, from: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        let added = self.shelf.add(paths);
        if added.is_empty() {
            return;
        }
        let Some(from) = from else {
            self.shelf.bump = Some(Instant::now());
            cx.notify();
            return;
        };
        let now = Instant::now();
        let flying = added.len().min(MAX_FLIGHTS);
        for (i, path) in added.iter().take(flying).enumerate() {
            let id = self.shelf.next_flight;
            self.shelf.next_flight += 1;
            // Fan the stack out a little so several items read as several.
            let spread = point(px(i as f32 * 9.), px(i as f32 * -6.));
            let count = if i + 1 == flying { added.len() - i } else { 1 };
            self.shelf.flights.push(Flight { id, from: from + spread, start: now + STAGGER * i as u32, icon: crate::icons::icon_for_path(path), count });
        }
        self.shelf.in_air += added.len();
        let _ = window;
        cx.notify();
    }

    pub(crate) fn toggle_shelf(&mut self, _: &ToggleShelf, _: &mut Window, cx: &mut Context<Self>) {
        let just_closed = self.shelf.closed_at.is_some_and(|t| t.elapsed() < Duration::from_millis(300));
        self.shelf.open = !self.shelf.open && !just_closed;
        if self.shelf.open {
            self.shelf.prune();
        }
        cx.notify();
    }

    fn unstage(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.shelf.items.retain(|p| p != path);
        self.shelf.save();
        if self.shelf.items.is_empty() {
            self.shelf.open = false;
        }
        cx.notify();
    }

    pub fn clear_shelf(&mut self, cx: &mut Context<Self>) {
        self.shelf.items.clear();
        self.shelf.open = false;
        self.shelf.save();
        cx.notify();
    }

    /// Copies or moves the whole shelf into `dest`, then empties it.
    fn send_shelf(&mut self, dest: PathBuf, copy: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.shelf.prune();
        let paths: Vec<PathBuf> = self.shelf.items.iter().filter(|p| p.parent() != Some(dest.as_path()) || copy).cloned().collect();
        if paths.is_empty() {
            window.push_notification(Notification::info("Everything on the shelf is already in this folder"), cx);
            return;
        }
        self.transfer(paths, dest, copy, window, cx);
        self.clear_shelf(cx);
    }

    /// Advances flight animations; returns the elements to paint.
    fn shelf_flights(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let now = Instant::now();
        let target = button_center(window);
        let landed: Vec<usize> = self
            .shelf
            .flights
            .iter()
            .filter(|f| now >= f.start && now.duration_since(f.start) >= FLIGHT)
            .map(|f| f.count)
            .collect();
        if !landed.is_empty() {
            self.shelf.flights.retain(|f| !(now >= f.start && now.duration_since(f.start) >= FLIGHT));
            self.shelf.in_air = self.shelf.in_air.saturating_sub(landed.iter().sum());
            self.shelf.bump = Some(now);
        }
        if self.shelf.flights.is_empty() {
            self.shelf.in_air = 0;
        } else {
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        let ch = crate::theme::chrome(cx);
        self.shelf
            .flights
            .iter()
            .filter(|f| now >= f.start)
            .map(|f| {
                let t = (now.duration_since(f.start).as_secs_f32() / FLIGHT.as_secs_f32()).clamp(0., 1.);
                let e = ease_in_out(t);
                // Quadratic Bézier arcing above both ends.
                let (ax, ay) = (f32::from(f.from.x), f32::from(f.from.y));
                let (bx, by) = (f32::from(target.x), f32::from(target.y));
                let (cx_, cy_) = ((ax + bx) / 2. + (bx - ax) * 0.15, ay.min(by) - 150.);
                let u = 1. - e;
                let x = u * u * ax + 2. * u * e * cx_ + e * e * bx;
                let y = u * u * ay + 2. * u * e * cy_ + e * e * by;
                let size = 42. - 22. * e;
                let opacity = if t > 0.82 { 1. - (t - 0.82) / 0.18 * 0.8 } else { 1. };
                kit::surface(div(), cx)
                    .id(("shelf-flight", f.id))
                    .absolute()
                    .left(px(x - size / 2.))
                    .top(px(y - size / 2.))
                    .size(px(size))
                    .rounded(px(size * 0.24))
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(opacity)
                    .child(crate::icons::icon(f.icon).size(px(size * 0.5)).text_color(ch.accent))
                    .when(f.count > 1, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(-5.))
                                .right(px(-6.))
                                .px(px(4.))
                                .rounded_full()
                                .bg(ch.accent)
                                .text_size(px(9.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(gpui_kit::white())
                                .child(format!("{}", f.count)),
                        )
                    })
                    .into_any_element()
            })
            .collect()
    }

    /// The floating shelf button, its popup and items in flight.
    pub(crate) fn render_shelf(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let flights = self.shelf_flights(window, cx);
        let dragging = cx.has_active_drag();
        let count = self.shelf.items.len().saturating_sub(self.shelf.in_air);
        if self.shelf.items.is_empty() && !dragging && flights.is_empty() {
            self.shelf.open = false;
            return Vec::new();
        }
        let ch = crate::theme::chrome(cx);
        // A short spring when items land: grow, overshoot, settle.
        let bump = self.shelf.bump.map(|t| t.elapsed()).filter(|d| *d < BUMP).map(|d| {
            let t = d.as_secs_f32() / BUMP.as_secs_f32();
            (t * std::f32::consts::PI).sin() * (1. - t) * 0.5
        });
        if bump.is_some() {
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        } else {
            self.shelf.bump = None;
        }
        let grow = 1. + bump.unwrap_or(0.) + if dragging { 0.08 } else { 0. };
        let size = SIZE * grow;
        let inset = (size - SIZE) / 2.;
        let open = self.shelf.open;
        let button = div()
            .id("shelf-button")
            .debug_selector(|| "shelf-button".into())
            .group("shelf-button")
            .absolute()
            .right(px(RIGHT - inset))
            .bottom(px(bottom() - inset))
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .occlude()
            .on_drop(cx.listener(|this, d: &DraggedFiles, window, cx| {
                if d.conn.is_some() {
                    window.push_notification(Notification::info("The shelf holds files on this computer only"), cx);
                    return;
                }
                if d.set.is_none() {
                    let paths = d.resolve(cx);
                    this.stage(paths, None, window, cx);
                }
            }))
            .on_drop(cx.listener(|this, ext: &ExternalPaths, window, cx| this.stage(ext.paths().to_vec(), None, window, cx)))
            .tooltip(move |w, cx| {
                let tip = if count == 0 { "Drop here to stage".to_string() } else { format!("Shelf · {}", item_count(count)) };
                gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx)
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_shelf(&ToggleShelf, window, cx)))
            // Paint the circular surface before the badge, so its border cannot
            // overpaint the count where the badge extends beyond the circle.
            .child(
                div()
                    .absolute()
                    .size_full()
                    .rounded_full()
                    .bg(if open { ch.accent } else { ch.window })
                    .border_1()
                    .border_color(if open { ch.accent } else { ch.hairline })
                    .shadow_lg()
                    .group_hover("shelf-button", |s| s.border_color(ch.accent.opacity(0.6)))
                    .group_drag_over::<DraggedFiles>("shelf-button", move |s| s.bg(ch.accent_soft).border_color(ch.accent).border_2())
                    .group_drag_over::<ExternalPaths>("shelf-button", move |s| s.bg(ch.accent_soft).border_color(ch.accent).border_2()),
            )
            .child(
                crate::icons::icon(if dragging { IconName::PackagePlus } else { IconName::Package })
                    .size(px(22. * grow))
                    .text_color(if open { gpui_kit::white() } else { ch.accent }),
            )
            .when(count > 0, |d| {
                d.child(
                    div()
                        .debug_selector(|| "shelf-count".into())
                        .absolute()
                        .top(px(-2.))
                        .right(px(-2.))
                        .min_w(px(19.))
                        .h(px(19.))
                        .px(px(5.))
                        .rounded_full()
                        .bg(if open { ch.window } else { ch.accent })
                        .border_2()
                        .border_color(ch.window)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.))
                        .line_height(px(12.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if open { ch.accent } else { gpui_kit::white() })
                        .child(div().debug_selector(|| "shelf-count-label".into()).child(count.to_string())),
                )
            });
        let mut out = Vec::new();
        if open && !self.shelf.items.is_empty() {
            out.push(self.render_shelf_popup(window, cx));
        }
        out.push(button.into_any_element());
        out.extend(flights);
        out
    }

    fn render_shelf_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let items = self.shelf.items.clone();
        let here = self.pane().read(cx).current_dir().map(Path::to_path_buf);
        let other = self.other_dir(cx).filter(|(_, conn)| conn.is_none()).map(|(d, _)| d);
        let devices: Vec<(String, String)> = if self.settings.nearby.enabled {
            self.nearby.rows().into_iter().filter(|r| r.paired && r.online.is_some() && !r.forgot_us).map(|r| (r.id, r.name)).collect()
        } else {
            Vec::new()
        };
        let set = Arc::new(items.clone());
        let source = self.pane().downgrade();
        let rows = items.iter().enumerate().map(|(i, path)| {
            let p = path.clone();
            let parent = path.parent().map(super::display_name).unwrap_or_default();
            h_flex()
                .id(("shelf-item", i))
                .debug_selector(move || format!("shelf-item-{i}"))
                .px(px(9.))
                .h(px(56.))
                .flex_none()
                .rounded(px(6.))
                .gap(px(10.))
                .hover(|s| s.bg(ch.hover))
                .on_click(cx.listener({
                    let p = p.clone();
                    move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                        if ev.click_count() >= 2 {
                            this.shelf.open = false;
                            this.reveal_in_pane(p.clone(), window, cx);
                        }
                    }
                }))
                .child(crate::icons::icon(crate::icons::icon_for_path(path)).size(px(21.)).text_color(ch.accent).flex_none())
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(3.))
                        .child(div().truncate().text_size(px(12.)).line_height(px(16.)).font_weight(FontWeight::MEDIUM).text_color(ch.text).child(display_name(path)))
                        .child(div().truncate().text_size(px(11.)).line_height(px(15.)).text_color(ch.text_faint).child(parent)),
                )
                .child(
                    div()
                        .id(("shelf-remove", i))
                        .debug_selector(move || format!("shelf-remove-{i}"))
                        .size(px(23.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .cursor_pointer()
                        .hover(|s| s.bg(ch.hover_strong))
                        .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Remove from shelf").build(w, cx))
                        .child(crate::icons::icon(IconName::X).size(px(12.)).text_color(ch.text_faint))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.unstage(&p, cx);
                        })),
                )
        }).collect::<Vec<_>>();

        let action = |id: gpui_kit::ElementId, icon: IconName, label: String, enabled: bool, cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .gap(px(11.))
                .px(px(11.))
                .h(px(34.))
                .flex_none()
                .rounded(px(7.))
                .text_size(px(12.))
                .when(enabled, |d| d.cursor_pointer().text_color(ch.text).hover(|s| s.bg(ch.hover_strong)))
                .when(!enabled, |d| d.text_color(ch.text_faint))
                .child(crate::icons::icon(icon).size(px(17.)).text_color(if enabled { ch.accent } else { ch.text_faint }))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
        };
        let here_name = here.as_ref().map(|d| display_name(d)).unwrap_or_else(|| "a folder".into());
        // Reserve room for the header and actions; only the collection scrolls
        // in ordinary windows, keeping the destinations within reach.
        let action_count = 3 + usize::from(other.is_some()) + devices.len();
        let actions_height = action_count as f32 * 34. + 16. + if devices.is_empty() { 0. } else { 32. };
        let popup_height = (f32::from(window.viewport_size().height) - bottom() - SIZE - 26.).max(100.);
        let list_height = (popup_height - actions_height - 128.).clamp(56., 240.);
        let mut actions = v_flex().flex_none().p(px(8.)).border_t_1().border_color(ch.hairline);
        {
            let dest = here.clone();
            actions = actions.child(action("shelf-copy-here".into(), IconName::Copy, format!("Copy to {here_name}"), dest.is_some(), cx).debug_selector(|| "shelf-copy-here".into()).when_some(dest, |d, dest| {
                d.on_click(cx.listener(move |this, _, window, cx| this.send_shelf(dest.clone(), true, window, cx)))
            }));
        }
        {
            let dest = here.clone();
            actions = actions.child(action("shelf-move-here".into(), IconName::FolderInput, format!("Move to {here_name}"), dest.is_some(), cx).when_some(dest, |d, dest| {
                d.on_click(cx.listener(move |this, _, window, cx| this.send_shelf(dest.clone(), false, window, cx)))
            }));
        }
        if let Some(dest) = other {
            let name = display_name(&dest);
            actions = actions.child(action("shelf-copy-other".into(), IconName::Columns2, format!("Copy to other pane ({name})"), true, cx).on_click(cx.listener(
                move |this, _, window, cx| this.send_shelf(dest.clone(), true, window, cx),
            )));
        }
        {
            let dir = here.clone().or_else(|| items.first().and_then(|p| p.parent()).map(Path::to_path_buf));
            actions = actions.child(action("shelf-compress".into(), IconName::Archive, "Compress…".into(), dir.is_some(), cx).when_some(dir, |d, dir| {
                d.on_click(cx.listener(move |this, _, window, cx| {
                    this.shelf.open = false;
                    let paths = this.shelf.items.clone();
                    this.compress_paths(paths, dir.clone(), window, cx);
                }))
            }));
        }
        if !devices.is_empty() {
            actions = actions
                .child(div().mx(px(10.)).my(px(7.)).h(px(1.)).bg(ch.hairline))
                .child(div().px(px(11.)).pb(px(6.)).text_size(px(9.)).line_height(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_faint).child("SEND TO DEVICE"));
        }
        for (i, (id, name)) in devices.into_iter().enumerate() {
            actions = actions.child(
                action(("shelf-device", i).into(), IconName::Send, name, true, cx)
                    .child(div().text_size(px(11.)).text_color(ch.text_faint).child("Nearby"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let paths = this.shelf.items.clone();
                        this.send_to_device(&id, paths, window, cx);
                        this.clear_shelf(cx);
                    })),
            );
        }

        let total = items.len();
        kit::surface(v_flex(), cx)
            .id("shelf-popup")
            .debug_selector(|| "shelf-popup".into())
            .occlude()
            .absolute()
            .right(px(RIGHT))
            .bottom(px(bottom() + SIZE + 10.))
            .w(px(348.))
            .rounded(px(12.))
            .max_h(px(popup_height))
            .overflow_hidden()
            .overflow_y_scroll()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.shelf.open = false;
                this.shelf.closed_at = Some(Instant::now());
                cx.notify();
            }))
            .child(
                h_flex()
                    .id("shelf-drag")
                    .debug_selector(|| "shelf-drag".into())
                    .px(px(18.))
                    .pt(px(18.))
                    .pb(px(14.))
                    .flex_none()
                    .gap(px(11.))
                    .cursor_grab()
                    // Dragging the header drags the whole shelf: into a folder,
                    // the sidebar, or out to other apps.
                    .on_drag(DraggedFiles { source, conn: None, path: items.first().cloned().unwrap_or_default(), set: Some(set) }, |_, offset, _, cx| {
                        cx.new(|_| crate::views::DragPreview { icon: IconName::Package, offset })
                    })
                    .child(
                        div().size(px(34.)).flex_none().flex().items_center().justify_center()
                            .rounded(px(9.)).border_1().border_color(ch.accent.opacity(0.13)).bg(ch.accent.opacity(0.04))
                            .child(crate::icons::icon(IconName::Package).size(px(21.)).text_color(ch.accent)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(div().text_size(px(15.)).line_height(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child("Shelf"))
                            .child(div().text_size(px(11.)).line_height(px(15.)).text_color(ch.text_muted).child(format!("{} collected", item_count(total)))),
                    )
                    .child(kit::button("shelf-clear", "Clear all", Tone::Ghost, true, cx).debug_selector(|| "shelf-clear".into()).on_click(cx.listener(|this, _, _, cx| this.clear_shelf(cx)))),
            )
            .child(
                div().mx(px(12.)).mb(px(12.)).p(px(5.)).flex_none()
                    .rounded(px(9.)).border_1().border_color(ch.hairline).bg(ch.window)
                    .debug_selector(|| "shelf-tray".into())
                    .child(div().id("shelf-list").debug_selector(|| "shelf-list".into()).max_h(px(list_height)).overflow_y_scroll().children(rows)),
            )
            .child(
                div().px(px(12.)).pb(px(13.)).flex_none().text_center().text_size(px(10.)).line_height(px(14.)).text_color(ch.text_faint)
                    .child("Drag the shelf header to take everything along"),
            )
            .child(actions)
            .into_any_element()
    }
}
