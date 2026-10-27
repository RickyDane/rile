mod actions;
mod ai_glow;
mod app_icon;
mod art;
mod dialogs;
mod icon_sets;
mod icons;
mod jobs;
mod pane;
mod progress;
mod remote;
mod sidebar;
mod statusbar;
mod theme;
mod views;
mod workspace;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use gpui_kit::component::TitleBar;
use gpui_kit::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

use rile_core::history::Location;

use crate::workspace::Workspace;

fn main() {
    gpui_kit::application()
        .with_assets(icons::AppAssets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            actions::bind_keys(cx);
            cx.on_action(|_: &actions::Quit, cx| {
                if let Some(ws) = cx.try_global::<views::WorkspaceHandle>().and_then(|h| h.0.upgrade()) {
                    ws.update(cx, |ws, cx| ws.on_quit(cx));
                }
                cx.quit();
            });

            let bounds = Bounds::centered(None, size(px(1180.), px(740.)), cx);
            let mut options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(560.), px(430.))),
                ..TitleBar::window_options()
            };
            // Center the traffic lights in the sidebar header (52px tall).
            if let Some(titlebar) = options.titlebar.as_mut() {
                titlebar.traffic_light_position = Some(gpui_kit::point(px(20.), px(19.)));
            }
            // `rile <folder>` opens it; `rile <file>` reveals it in its folder.
            let arg = std::env::args().nth(1).map(PathBuf::from);
            let reveal = arg.clone().filter(|p| p.is_file());
            let start = match (&arg, &reveal) {
                (_, Some(file)) => file.parent().map(|p| Location::Dir(p.to_path_buf())),
                (Some(dir), None) if dir.is_dir() => Some(Location::Dir(dir.clone())),
                _ => None,
            }
            .unwrap_or(Location::Home);
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let ws = Workspace::new(start, window, cx);
                    if let Some(file) = reveal {
                        ws.pane().update(cx, |p, _| p.select_after_reload(vec![file]));
                    }
                    ws
                })
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
