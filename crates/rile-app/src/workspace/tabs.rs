//! Folder tabs. Each pane slot keeps its own list of tabs; every tab is a
//! full `Pane` with its own history, selection and view.

use gpui_kit::{Context, Window};
use rile_core::history::Location;

use super::Workspace;
use crate::actions::*;

impl Workspace {
    /// Tabs of the active slot.
    pub fn slot_tabs(&self) -> &[gpui_kit::Entity<crate::pane::Pane>] {
        &self.tabs[self.active_pane]
    }

    pub fn active_tab(&self) -> usize {
        self.tabs[self.active_pane].iter().position(|p| p == self.pane()).unwrap_or(0)
    }

    /// Opens `location` in a new tab right after the current one.
    pub fn open_tab(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.create_pane(location, window, cx);
        let slot = self.active_pane;
        let at = self.active_tab() + 1;
        self.tabs[slot].insert(at, pane);
        self.activate_tab(at, window, cx);
    }

    pub fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let slot = self.active_pane;
        let Some(pane) = self.tabs[slot].get(ix).cloned() else { return };
        self.panes[slot] = pane.clone();
        self.sync_active_flags(cx);
        self.editing_path = false;
        self.sync_search_input(window, cx);
        let focus = pane.read(cx).focus_handle.clone();
        focus.focus(window, cx);
        if matches!(pane.read(cx).location, Location::Home) {
            self.refresh_overview(cx);
        }
        self.update_window_title(window, cx);
        self.sync_preview(cx);
        cx.notify();
    }

    /// Closes a tab of the active slot. The last tab returns to the Overview
    /// instead, so a slot is never empty.
    pub fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let slot = self.active_pane;
        if self.tabs[slot].len() <= 1 {
            self.open_location(Location::Home, window, cx);
            return;
        }
        if ix >= self.tabs[slot].len() {
            return;
        }
        let was_active = ix == self.active_tab();
        let closed = self.tabs[slot].remove(ix);
        self._pane_subs.remove(&closed.entity_id());
        if was_active {
            let next = ix.min(self.tabs[slot].len() - 1);
            self.activate_tab(next, window, cx);
        } else {
            cx.notify();
        }
    }

    pub fn update_window_title(&self, window: &mut Window, cx: &gpui_kit::App) {
        window.set_window_title(&tab_title(&self.pane().read(cx).location));
    }

    pub(crate) fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(Location::Home, window, cx);
    }

    pub(crate) fn close_tab_action(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.active_tab();
        self.close_tab(ix, window, cx);
    }

    pub(crate) fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.slot_tabs().len();
        let ix = (self.active_tab() + 1) % n;
        self.activate_tab(ix, window, cx);
    }

    pub(crate) fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.slot_tabs().len();
        let ix = (self.active_tab() + n - 1) % n;
        self.activate_tab(ix, window, cx);
    }

    pub(crate) fn open_in_new_tab(&mut self, _: &OpenInNewTab, window: &mut Window, cx: &mut Context<Self>) {
        let target = {
            let pane = self.pane().read(cx);
            pane.cursor_entry()
                .filter(|e| e.is_dir && pane.selected.contains(&e.path))
                .map(|e| pane.location_for(e.path.clone()))
                .unwrap_or_else(|| pane.location.clone())
        };
        self.open_tab(target, window, cx);
    }
}

pub fn tab_title(location: &Location) -> String {
    match location {
        Location::Home => "Home".to_string(),
        Location::Gallery => "Photos".to_string(),
        Location::Backups => "Backups".to_string(),
        Location::Devices => "Nearby Devices".to_string(),
        Location::SpaceAnalyzer(_) => "Space Analyzer".to_string(),
        Location::PluginPage { title, .. } => title.clone(),
        // The startup disk's root reads as its volume name (as in the sidebar).
        Location::Dir(p) if p.parent().is_none() && cfg!(target_os = "macos") => "Macintosh HD".to_string(),
        Location::Dir(p) => super::display_name(p),
        Location::Remote { conn, path } => {
            match rile_remote::paths::file_name(path) {
                "" => crate::remote::label(conn),
                name => format!("{name} — {}", crate::remote::label(conn)),
            }
        }
    }
}
