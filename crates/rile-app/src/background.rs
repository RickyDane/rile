//! One tray item for Rile, Nearby and backup status/actions.
//! Closing hides the workspace rather than dropping its Nearby server/tasks.

use gpui_kit::{App, Window};

#[cfg(any(target_os = "macos", windows))]
pub(crate) struct RunningInTray;
#[cfg(any(target_os = "macos", windows))]
impl gpui_kit::Global for RunningInTray {}

pub fn install(cx: &mut App) {
    #[cfg(any(target_os = "macos", windows))]
    {
        use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{Icon, TrayIconBuilder};
        use rile_services::autostart;

        let allowed = std::env::var_os("RILE_NO_LAUNCH_AGENTS").is_none();
        // Refresh moved/updated installations, without opting anyone in.
        if allowed && autostart::is_enabled() {
            if let Err(error) = autostart::set_enabled(true) {
                eprintln!("Couldn't refresh login startup: {error:#}");
            }
        }
        let startup = CheckMenuItem::with_id("rile.login", "Start Rile at login", allowed, autostart::is_enabled(), None);
        let mut backups = crate::backup_menu::BackupMenu::new();
        let quit = MenuItem::with_id("rile.quit", "Quit Rile", true, None);

        let build = || -> anyhow::Result<_> {
            let menu = Menu::new();
            menu.append_items(&[
                &MenuItem::with_id("rile.open", "Open Rile", true, None),
                &MenuItem::new("Nearby stays available when enabled", false, None),
                &startup,
                &PredefinedMenuItem::separator(),
                &backups.menu,
                &PredefinedMenuItem::separator(),
                &quit,
            ])?;
            let image = image::load_from_memory(crate::app_icon::WINDOWS_PNG)?
                .resize(32, 32, image::imageops::FilterType::Lanczos3)
                .into_rgba8();
            let (width, height) = image.dimensions();
            let icon = Icon::from_rgba(image.into_raw(), width, height)?;
            Ok(TrayIconBuilder::new()
                .with_tooltip("Rile — running in the background")
                .with_icon(icon)
                .with_menu(Box::new(menu))
                .build()?)
        };
        let icon = match build() {
            Ok(icon) => icon,
            Err(error) => {
                eprintln!("Couldn't install Rile's tray icon: {error:#}");
                return;
            }
        };
        cx.set_global(RunningInTray);
        let login = startup.clone();
        let status_icon = icon.clone();
        cx.spawn(async move |cx| {
            let mut monitor = crate::backup_menu::Monitor::default();
            let mut visible = true;
            loop {
                let (next, model) = cx.background_executor().spawn(async move {
                    let model = monitor.refresh();
                    (monitor, model)
                }).await;
                monitor = next;
                cx.update(|cx| {
                    // Let an old helper finish its own backup before taking
                    // over its icon; never display both during the handoff.
                    if visible == monitor.handoff_pending {
                        if status_icon.set_visible(!monitor.handoff_pending).is_ok() {
                            visible = !monitor.handoff_pending;
                        }
                    }
                    if let Err(error) = backups.update(&model) {
                        eprintln!("Updating the backup menu: {error:#}");
                    }
                    login.set_checked(autostart::is_enabled());
                    let running = cx.try_global::<crate::MainWindow>().and_then(|main| main.1.upgrade())
                        .map_or(0, |ws| ws.read(cx).backups.running.len());
                    quit.set_text(crate::backup_menu::quit_title(running));
                    let title = if model.title.is_empty() { "Rile — running in the background" } else { &model.title };
                    status_icon.set_tooltip(Some(title)).ok();
                });
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            }
        })
        .detach();
        // Menu actions wake immediately; backup status refreshes independently.
        let (tx, rx) = smol::channel::unbounded();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = tx.try_send(event);
        }));
        // GPUI owns the native event loop; handle actions on its UI thread.
        cx.spawn(async move |cx| {
            // Keep the native item alive until the app's executor shuts down.
            let _icon = icon;
            while let Ok(event) = rx.recv().await {
                cx.update(|cx| match event.id.as_ref() {
                    "rile.open" => crate::reopen(cx),
                    "rile.quit" => crate::quit(cx),
                    "rile.login" => {
                        let wanted = !autostart::is_enabled();
                        if let Err(error) = autostart::set_enabled(wanted) {
                            startup.set_checked(autostart::is_enabled());
                            report_error(format!("Couldn't change login startup: {error:#}"), cx);
                        } else {
                            startup.set_checked(wanted);
                        }
                    }
                    id if id.starts_with("rile.backup.") => crate::backup_menu::dispatch(id, cx),
                    _ => {},
                });
            }
        })
        .detach();
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = cx;
}

pub fn available(cx: &App) -> bool {
    #[cfg(any(target_os = "macos", windows))]
    return cx.try_global::<RunningInTray>().is_some();
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = cx;
        false
    }
}

pub fn start_hidden(args: &[String], cx: &App) -> bool {
    args.first().is_some_and(|arg| arg == rile_services::autostart::BACKGROUND_FLAG) && available(cx)
}

#[cfg(any(target_os = "macos", windows))]
fn report_error(message: String, cx: &mut App) {
    use gpui_kit::component::{WindowExt, notification::Notification};
    crate::reopen(cx);
    if let Some(handle) = cx.try_global::<crate::MainWindow>().map(|main| main.0) {
        handle.update(cx, |_, window, cx| window.push_notification(Notification::error(message), cx)).ok();
    }
}

/// Returns false if there's no tray to recover the hidden window from.
pub fn hide(window: &mut Window, cx: &mut App) -> bool {
    if available(cx) {
        // GPUI's simulated platform has no implementation of native hiding.
        #[cfg(test)]
        return true;
        #[cfg(all(target_os = "macos", not(test)))]
        {
            cx.hide();
            return true;
        }
        #[cfg(all(windows, not(test)))]
        if let Some(hwnd) = hwnd(window) {
            use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindowAsync};
            // SAFETY: the HWND belongs to this live GPUI window on the UI thread.
            // Posting avoids reentrant GPUI callbacks during native close handling.
            unsafe {
                let _ = ShowWindowAsync(hwnd, SW_HIDE);
            }
            return true;
        }
    }
    let _ = (window, cx);
    false
}

/// Restore hidden windows for tray, Dock, external-open and Nearby requests.
pub fn show(window: &mut Window, cx: &mut App) {
    #[cfg(all(windows, not(test)))]
    if let Some(hwnd) = hwnd(window) {
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SW_SHOW, ShowWindowAsync};
        // SAFETY: the HWND belongs to this live GPUI window on the UI thread.
        unsafe {
            // Do not unmaximize an already visible or maximized window.
            let command = if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW };
            let _ = ShowWindowAsync(hwnd, command);
        }
    }
    window.activate_window();
    cx.activate(true);
}

#[cfg(all(windows, not(test)))]
fn hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}
