mod actions;
mod ai_glow;
mod app_icon;
#[cfg(target_os = "macos")]
mod backup_menu;
mod art;
mod dialogs;
mod external_open;
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
use gpui_kit::{AnyWindowHandle, App, AppContext, Bounds, Entity, Global, WindowBounds, WindowOptions, px, size};

use rile_core::history::Location;

use crate::workspace::Workspace;

struct MainWindow(AnyWindowHandle, gpui_kit::WeakEntity<Workspace>);
impl Global for MainWindow {}

fn reopen(cx: &mut App) {
    // Auxiliary windows (previews, speed tests) may outlive the workspace.
    let window = cx.try_global::<MainWindow>().map(|window| window.0);
    if window.is_none_or(|window| window.update(cx, |_, window, _| window.activate_window()).is_err()) {
        open_workspace(Location::Home, None, cx).expect("failed to reopen window");
    }
    cx.activate(true);
}

fn open_workspace(
    start: Location,
    reveal: Option<PathBuf>,
    cx: &mut App,
) -> anyhow::Result<(AnyWindowHandle, Entity<Workspace>)> {
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
    let (window, workspace) = gpui_kit::open_window(options, cx, |window, cx| {
        let workspace = cx.new(|cx| {
            let ws = Workspace::new(start, window, cx);
            if let Some(file) = reveal {
                ws.pane().update(cx, |p, _| p.select_after_reload(vec![file]));
            }
            ws
        });
        let weak = workspace.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            // Save before the window releases the workspace, not only on Cmd-Q.
            weak.update(cx, |ws, cx| ws.on_quit(cx)).ok();
            true
        });
        workspace
    })?;
    cx.set_global(MainWindow(window, workspace.downgrade()));
    Ok((window, workspace))
}

/// Shows the Backups page (and optionally its settings) in the main window,
/// opening one if needed. Used by `rile://` links from the menu bar helper.
pub(crate) fn show_backups(settings: bool, cx: &mut App) {
    let existing = cx.try_global::<MainWindow>().and_then(|main| main.1.upgrade().map(|ws| (main.0, ws)));
    let (handle, ws) = match existing {
        Some(found) => found,
        None => match open_workspace(Location::Backups, None, cx) {
            Ok(opened) => opened,
            Err(e) => return eprintln!("Couldn't open the Backups page: {e}"),
        },
    };
    handle
        .update(cx, |_, window, cx| {
            ws.update(cx, |ws, cx| {
                ws.open_location(Location::Backups, window, cx);
                if settings {
                    dialogs::open_backup_settings(cx.entity(), ws.settings.clone(), None, window, cx);
                }
            });
            window.activate_window();
        })
        .ok();
    cx.activate(true);
}

fn main() {
    // Started by the backup launch agent: run due backups without any UI.
    if std::env::args().any(|a| a == rile_backup::agent::RUN_FLAG) {
        workspace::backups::run_agent();
        return;
    }
    // Started at login by its launch agent: the backups menu bar item.
    #[cfg(target_os = "macos")]
    if std::env::args().any(|a| a == rile_backup::agent::MENU_FLAG) {
        backup_menu::run();
        return;
    }
    let application = gpui_kit::application().with_assets(icons::AppAssets);
    application.on_reopen(reopen);
    let requests = external_open::Requests::default();
    application.on_open_urls({
        let requests = requests.clone();
        move |urls: Vec<String>| {
            let (links, files): (Vec<String>, Vec<String>) = urls.into_iter().partition(|u| u.starts_with("rile://"));
            for link in links.iter().filter_map(|u| external_open::AppLink::parse(u)) {
                requests.dispatch_link(link);
            }
            if !files.is_empty() {
                requests.dispatch(external_open::file_paths(files), false);
            }
        }
    });
    // Register before AppKit finishes launching: a cold-start reveal can arrive
    // before GPUI's run callback, just like an open-document event.
    #[cfg(target_os = "macos")]
    let _file_viewer_events = external_open::mac::install({
        let requests = requests.clone();
        move |paths| requests.dispatch(paths, true)
    });
    application
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            actions::bind_keys(cx);
            cx.on_action(|_: &actions::Quit, cx| {
                if let Some(ws) = cx.try_global::<views::WorkspaceHandle>().and_then(|h| h.0.upgrade()) {
                    ws.update(cx, |ws, cx| ws.on_quit(cx));
                }
                cx.quit();
            });

            // `rile <folder>` opens it; `rile <file>` reveals it in its folder.
            let arg = std::env::args_os().nth(1).map(PathBuf::from);
            let reveal = arg.clone().filter(|p| p.is_file());
            let start = match (&arg, &reveal) {
                (Some(flag), _) if flag.as_os_str() == "--backups" => Some(Location::Backups),
                (_, Some(file)) => file.parent().map(|p| Location::Dir(p.to_path_buf())),
                (Some(dir), None) if dir.is_dir() => Some(Location::Dir(dir.clone())),
                _ => None,
            }
            .unwrap_or(Location::Home);
            open_workspace(start, reveal, cx).expect("failed to open window");
            requests.ready(cx);
            cx.activate(true);
        });
}
