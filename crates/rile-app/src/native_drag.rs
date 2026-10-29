//! Keep in-app drags until the pointer leaves the window, then offer real files
//! to AppKit/OLE. GPUI's Windows backend does not implement outgoing drags;
//! its macOS handoff also relies on a retained mouse-down event. `drag` starts
//! a session at the current pointer with its own native drag source instead.

use gpui_kit::{App, Bounds, DragMoveEvent, ExternalDragPayload, MouseButton, Point, Window};

use crate::pane::DraggedFiles;

pub fn handoff(event: &DragMoveEvent<DraggedFiles>, window: &mut Window, cx: &mut App) -> bool {
    if event.event.pressed_button != Some(MouseButton::Left)
        || Bounds::new(Point::default(), window.viewport_size()).contains(&event.event.position)
    {
        return false;
    }
    let Some(files) = event.dragged_item().downcast_ref::<DraggedFiles>() else { return false };
    let Some(ExternalDragPayload::Files(payload)) = files.external_payload(cx) else { return false };
    let paths = payload.entries().iter().map(|(path, _)| path.clone()).collect();

    // Clear the preview/cursor before entering the native loop (blocking on
    // Windows). Neither cancellation nor a failed start may leave a stuck drag
    // or cause an internal drop when the mouse is released later.
    cx.stop_active_drag(window);
    start(paths, window, cx);
    true
}

#[cfg(not(test))]
fn start(paths: Vec<std::path::PathBuf>, window: &mut Window, cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        let _ = cx;
        // AppKit's dragging session is asynchronous, so it can start in place.
        start_session(paths, window);
    }
    // OLE's DoDragDrop runs a modal loop until the drop. Started here, inside
    // GPUI's event dispatch, that loop would pump GPUI's own messages (frame
    // requests, foreground tasks) while the app is borrowed: the reentrant
    // borrow panics in the window procedure and aborts the process. Run it from
    // a foreground task instead, where nothing is borrowed and GPUI can keep
    // drawing and running tasks during the drag.
    #[cfg(windows)]
    {
        use raw_window_handle::HasWindowHandle;
        let Ok(handle) = window.window_handle().map(|handle| handle.as_raw()) else { return };
        cx.spawn(async move |_| start_session(paths, &win::Handle(handle))).detach();
    }
}

#[cfg(not(test))]
fn start_session(paths: Vec<std::path::PathBuf>, window: &impl raw_window_handle::HasWindowHandle) {
    // GPUI captures the mouse on button-down. OLE must own capture during its
    // drag loop, and GPUI will not necessarily see the eventual button-up.
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
    }
    // A small, valid PNG is required by AppKit; reuse the bundled app artwork.
    static PREVIEW: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let preview = PREVIEW.get_or_init(|| {
        let icon = image::load_from_memory(include_bytes!("../assets/app-icons/drawer-hot-rod/blue.png"))
            .expect("bundled drag icon")
            .resize(32, 32, image::imageops::FilterType::Lanczos3);
        let mut bytes = std::io::Cursor::new(Vec::new());
        icon.write_to(&mut bytes, image::ImageFormat::Png).expect("drag icon PNG");
        bytes.into_inner()
    });
    if let Err(error) = drag::start_drag(
        window,
        drag::DragItem::Files(paths),
        drag::Image::Raw(preview.clone()),
        |_, _| {},
        // Copy is deliberately the default: the source must never be removed
        // just because another app accepted the drag.
        drag::Options::default(),
    ) {
        eprintln!("Could not start native file drag: {error}");
    }
}

#[cfg(all(windows, not(test)))]
mod win {
    use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};

    /// The window's raw handle, detached from GPUI's `Window` borrow.
    pub struct Handle(pub RawWindowHandle);

    impl HasWindowHandle for Handle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            // SAFETY: the HWND belongs to a live GPUI window for the drag's duration.
            Ok(unsafe { WindowHandle::borrow_raw(self.0) })
        }
    }
}

// Headless windows have no native handle. Record the OS boundary instead of
// opening a real AppKit/OLE session in UI integration tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Requests(pub Vec<Vec<std::path::PathBuf>>);
#[cfg(test)]
impl gpui_kit::Global for Requests {}

#[cfg(test)]
fn start(paths: Vec<std::path::PathBuf>, _: &mut Window, cx: &mut App) {
    cx.default_global::<Requests>().0.push(paths);
}
