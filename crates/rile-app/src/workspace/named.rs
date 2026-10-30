//! Named workspaces: the open tabs of both pane slots saved under a name
//! ("Photos project", "Release"), restored with one click from the
//! workspace menu in the tab strip.

use std::cell::Cell;

use gpui_kit::assets::IconName;
use gpui_kit::component::{WindowExt, input::{InputEvent, InputState}, menu::{PopupMenu, PopupMenuItem}, notification::Notification};
use gpui_kit::{App, AppContext, Context, FocusHandle, WeakEntity, Window};
use rile_core::history::{Location, SavedLocation};
use rile_core::settings::{SavedSlot, SavedWorkspace, ViewKind};

use super::Workspace;
use crate::actions::*;
use crate::dialogs;

impl Workspace {
    /// The current tabs and panes as a workspace named `name`.
    pub fn snapshot_workspace(&self, name: String, cx: &App) -> SavedWorkspace {
        let slots = self
            .tabs
            .iter()
            .enumerate()
            .take(if self.dual { 2 } else { 1 })
            .map(|(slot, tabs)| SavedSlot {
                tabs: tabs.iter().map(|p| SavedLocation::from(&p.read(cx).location)).collect(),
                active: tabs.iter().position(|p| *p == self.panes[slot]).unwrap_or(0),
            })
            .collect();
        SavedWorkspace { name, dual: self.dual, active_slot: if self.dual { self.active_pane } else { 0 }, slots }
    }

    /// Replaces all tabs with saved workspace `name`.
    pub fn restore_workspace(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(saved) = self.settings.workspaces.iter().find(|w| w.name == name).cloned() else { return };
        for pane in self.all_panes() {
            self._pane_subs.remove(&pane.entity_id());
        }
        self.tabs.clear();
        self.panes.clear();
        self.drive_menu = None;
        let dual = saved.dual && saved.slots.len() >= 2;
        // New panes pick their view from this (dual pane uses lists).
        self.dual = dual;
        for slot in saved.slots.iter().take(if dual { 2 } else { 1 }) {
            let mut tabs = Vec::new();
            for loc in &slot.tabs {
                let loc = match Location::from(loc.clone()) {
                    // A folder that's gone (unplugged drive, deleted) opens the Overview.
                    Location::Dir(p) if !p.is_dir() => Location::Home,
                    other => other,
                };
                tabs.push(self.create_pane(loc, window, cx));
            }
            if tabs.is_empty() {
                tabs.push(self.create_pane(Location::Home, window, cx));
            }
            let active = slot.active.min(tabs.len() - 1);
            self.panes.push(tabs[active].clone());
            self.tabs.push(tabs);
        }
        if self.panes.is_empty() {
            let pane = self.create_pane(Location::Home, window, cx);
            self.panes.push(pane.clone());
            self.tabs.push(vec![pane]);
        }
        let view = if dual { ViewKind::List } else { self.settings.default_view };
        for pane in self.all_panes() {
            pane.update(cx, |p, cx| p.set_view(view, cx));
        }
        self.active_pane = saved.active_slot.min(self.panes.len() - 1);
        self.settings.dual_pane = dual;
        self.current_workspace = Some(saved.name.clone());
        self.save_settings(window, cx);
        self.sync_active_flags(cx);
        self.editing_path = false;
        self.sync_search_input(window, cx);
        let focus = self.pane().read(cx).focus_handle.clone();
        focus.focus(window, cx);
        self.update_window_title(window, cx);
        self.sync_preview(cx);
        if matches!(self.pane().read(cx).location, Location::Home) {
            self.refresh_overview(cx);
        }
        cx.notify();
    }

    /// Saves the current layout as `name`, replacing a workspace of that name.
    pub fn save_workspace_as(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.snapshot_workspace(name.clone(), cx);
        match self.settings.workspaces.iter_mut().find(|w| w.name == name) {
            Some(existing) => *existing = ws,
            None => self.settings.workspaces.push(ws),
        }
        self.current_workspace = Some(name.clone());
        self.save_settings(window, cx);
        window.push_notification(Notification::success(format!("Saved workspace “{name}”")), cx);
        cx.notify();
    }

    pub fn update_workspace(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        dialogs::confirm(
            format!("Update workspace “{name}” with the current tabs and pane layout? This replaces its saved layout."),
            "Update Workspace",
            false,
            window,
            cx,
            move |window, cx| {
                ws.update(cx, |ws, cx| ws.save_workspace_as(name.clone(), window, cx)).ok();
            },
        );
    }

    pub fn delete_workspace(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.workspaces.retain(|w| w.name != name);
        if self.current_workspace.as_deref() == Some(name) {
            self.current_workspace = None;
        }
        self.save_settings(window, cx);
        cx.notify();
    }

    /// Asks for a name and saves the current layout under it.
    pub(crate) fn save_workspace(&mut self, _: &SaveWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        let default = self.current_workspace.clone().unwrap_or_else(|| {
            (1..).map(|n| format!("Workspace {n}")).find(|n| !self.settings.workspaces.iter().any(|w| &w.name == n)).unwrap()
        });
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name").default_value(default));
        let ws = cx.entity().downgrade();
        // Runs from the OK button and the input's Enter subscription (inside
        // a workspace update), so the workspace is only touched deferred.
        let submit = {
            let input = input.clone();
            move |window: &mut Window, cx: &mut App| -> bool {
                let name = input.read(cx).value().trim().to_string();
                if name.is_empty() {
                    return false;
                }
                let ws = ws.clone();
                window.defer(cx, move |window, cx| {
                    ws.update(cx, |ws, cx| ws.save_workspace_as(name, window, cx)).ok();
                });
                true
            }
        };
        self._dialog_sub = Some(cx.subscribe_in(&input, window, {
            let submit = submit.clone();
            move |_, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event
                    && submit(window, cx)
                {
                    window.close_dialog(cx);
                }
            }
        }));
        let first = Cell::new(true);
        let focus_pane = self.pane().read(cx).focus_handle.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.update(cx, |i, cx| {
                    i.focus(window, cx);
                    i.select_all(window, cx);
                });
            }
            let submit = submit.clone();
            let focus_pane = focus_pane.clone();
            dialogs::input_dialog(dialog, "Save Workspace", &input, move |window, cx| submit(window, cx), cx)
                .on_ok(|_, _, _| false)
                .on_close(move |_, window, cx| focus_pane.focus(window, cx))
        });
    }
}

/// The workspace menu: saved workspaces, save, update and delete.
pub fn menu(
    mut menu: PopupMenu,
    ws: WeakEntity<Workspace>,
    focus: FocusHandle,
    saved: Vec<SavedWorkspace>,
    current: Option<String>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    use crate::icons::menu_icon;
    menu = menu.action_context(focus).min_w(gpui_kit::px(260.)).label("Workspaces");
    if saved.is_empty() {
        menu = menu.item(PopupMenuItem::new("No saved workspaces yet").disabled(true));
    }
    for w in &saved {
        let (ws, name) = (ws.clone(), w.name.clone());
        let tabs: usize = w.slots.iter().map(|s| s.tabs.len()).sum();
        let label = format!("{}  ·  {} tab{}{}", w.name, tabs, if tabs == 1 { "" } else { "s" }, if w.dual { ", dual" } else { "" });
        menu = menu.item(PopupMenuItem::new(label).checked(current.as_deref() == Some(&w.name)).on_click(move |_, window, cx| {
            ws.update(cx, |ws, cx| ws.restore_workspace(&name, window, cx)).ok();
        }));
    }
    {
        // Opened once the menu has closed, so its dismissal can't take the
        // dialog's focus.
        let ws = ws.clone();
        menu = menu.separator().item(PopupMenuItem::new("Save Current Tabs as Workspace…").icon(menu_icon(IconName::Save)).on_click(move |_, window, cx| {
            let ws = ws.clone();
            window.defer(cx, move |window, cx| {
                ws.update(cx, |ws, cx| ws.save_workspace(&SaveWorkspace, window, cx)).ok();
            });
        }));
    }
    if let Some(name) = current.clone().filter(|n| saved.iter().any(|w| &w.name == n)) {
        let ws = ws.clone();
        let label = format!("Update “{name}”");
        menu = menu.item(PopupMenuItem::new(label).icon(menu_icon(IconName::RotateCw)).on_click(move |_, window, cx| {
            let name = name.clone();
            let ws = ws.clone();
            window.defer(cx, move |window, cx| {
                ws.update(cx, |ws, cx| ws.update_workspace(name, window, cx)).ok();
            });
        }));
    }
    if !saved.is_empty() {
        let ws = ws.clone();
        let names: Vec<String> = saved.iter().map(|w| w.name.clone()).collect();
        menu = menu.submenu_with_icon(Some(menu_icon(IconName::Trash)), "Delete", window, cx, move |mut menu, _, _| {
            for name in &names {
                let (ws, name) = (ws.clone(), name.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, window, cx| {
                    ws.update(cx, |ws, cx| ws.delete_workspace(&name, window, cx)).ok();
                }));
            }
            menu
        });
    }
    menu
}
