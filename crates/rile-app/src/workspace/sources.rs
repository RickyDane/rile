//! Per-slot storage and remote source menus, shared by the mouse and keyboard.
use gpui_kit::{Context, DismissEvent, Focusable, InteractiveElement, IntoElement, ParentElement, Styled, Window, px};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::assets::IconName;
use gpui_kit::component::{menu::{PopupMenu, PopupMenuItem}, popover::Popover};
use rile_core::history::Location;

use super::Workspace;
use crate::actions::{SelectLeftDrive, SelectRightDrive};

impl Workspace {
    pub(super) fn select_left_drive(&mut self, _: &SelectLeftDrive, window: &mut Window, cx: &mut Context<Self>) {
        self.show_drive_menu(0, window, cx);
    }

    pub(super) fn select_right_drive(&mut self, _: &SelectRightDrive, window: &mut Window, cx: &mut Context<Self>) {
        self.show_drive_menu(1, window, cx);
    }

    fn show_drive_menu(&mut self, slot: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dual { return; }
        self.set_active_pane(slot, window, cx);
        let pane = self.panes[slot].clone();
        let location = pane.read(cx).location.clone();
        let volumes = self.volumes.clone();
        let connections = if self.settings.remote_enabled { self.connections.clone() } else { Vec::new() };
        let ws = cx.entity().downgrade();
        let focus = pane.read(cx).focus_handle.clone();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu.action_context(focus).scrollable(true).max_h(px(400.));
            let current = match &location {
                Location::Dir(path) => volumes.iter().filter(|v| path.starts_with(&v.mount_point))
                    .max_by_key(|v| v.mount_point.as_os_str().len()).map(|v| &v.mount_point),
                _ => None,
            };
            for volume in &volumes {
                let target = volume.mount_point.clone();
                let ws = ws.clone();
                menu = menu.item(PopupMenuItem::new(volume.name.clone())
                    .checked(current == Some(&volume.mount_point))
                    .on_click(move |_, window, cx| {
                        _ = ws.update(cx, |ws, cx| {
                            ws.set_active_pane(slot, window, cx);
                            ws.open_location(Location::Dir(target.clone()), window, cx);
                        });
                    }));
            }
            if !connections.is_empty() && !volumes.is_empty() { menu = menu.separator().label("Servers"); }
            for conn in connections {
                let ws = ws.clone();
                let checked = matches!(&location, Location::Remote { conn: id, .. } if *id == conn.id);
                let label = if conn.name.is_empty() { conn.display() } else { conn.name.clone() };
                menu = menu.item(PopupMenuItem::new(label).checked(checked).on_click(move |_, window, cx| {
                    _ = ws.update(cx, |ws, cx| {
                        ws.set_active_pane(slot, window, cx);
                        ws.open_connection(&conn.id, window, cx);
                    });
                }));
            }
            menu
        });
        cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, _, cx| {
            this.drive_menu = None;
            cx.notify();
        }).detach();
        menu.focus_handle(cx).focus(window, cx);
        self.drive_menu = Some((slot, menu));
        cx.notify();
    }

    pub(crate) fn render_drive_selector(&self, slot: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let location = &self.panes[slot].read(cx).location;
        let label = match location {
            Location::Dir(path) => crate::views::chrome::volume_for(self, path).map(|v| v.name.clone()),
            Location::Remote { conn, .. } => Some(crate::remote::label(conn)),
            _ => None,
        }.unwrap_or_else(|| "Select drive".into());
        let menu = self.drive_menu.as_ref().filter(|(ix, _)| *ix == slot).map(|(_, menu)| menu.clone());
        let ws = cx.entity().downgrade();
        let menu_focus = menu.as_ref().map(|menu| menu.focus_handle(cx));
        let icon = match &location { Location::Remote { conn, .. } => crate::remote::icon(conn), _ => IconName::HardDrive };
        let tip = format!("{label} · Select drive ({}F{})", if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" }, slot + 1);
        gpui_kit::div().flex().flex_none().child(
            Popover::new(("drive-selector", slot)).open(menu.is_some())
                .anchor(if slot == 0 { gpui_kit::Anchor::TopLeft } else { gpui_kit::Anchor::TopRight })
                .when_some(menu_focus, |popover, focus| popover.track_focus(&focus))
                .appearance(false)
                .trigger(crate::views::chrome::icon_button(("drive-selector-button", slot), icon, tip, &crate::theme::chrome(cx))
                    .w_auto().max_w(px(180.)).overflow_hidden().label(label)
                    .debug_selector(move || format!("drive-selector-{slot}")))
                .on_open_change(move |open, window, cx| {
                    _ = ws.update(cx, |ws, cx| {
                        if *open { ws.show_drive_menu(slot, window, cx); }
                        else { ws.drive_menu = None; cx.notify(); }
                    });
                })
                .content(move |_, _, _| gpui_kit::div().children(menu.clone())),
        )
    }
}
