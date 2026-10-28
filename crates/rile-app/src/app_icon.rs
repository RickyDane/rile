//! Keep the running Dock and app bundle icons (window icons on Windows) in sync with the selected style and accent.

use rile_core::settings::{Accent, AppIcon};

include!("app_icon_table.rs");

/// The icon as an image for drawing in the UI (cached, so renders stay cheap).
pub fn png(style: AppIcon, accent: Accent) -> Option<std::sync::Arc<gpui_kit::Image>> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;

    thread_local! {
        static CACHE: RefCell<HashMap<(AppIcon, Accent), Arc<gpui_kit::Image>>> = RefCell::new(HashMap::new());
    }
    let image = CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry((style, accent))
            .or_insert_with(|| Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png_bytes(style, accent).to_vec())))
            .clone()
    });
    Some(image)
}

// Tests use GPUI's simulated app, not the actual NSApplication.
#[cfg(all(target_os = "macos", not(test)))]
pub fn install(style: AppIcon, accent: Accent) {
    use std::cell::Cell;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{NSBundle, NSData};

    thread_local! {
        static CURRENT: Cell<Option<(AppIcon, Accent)>> = const { Cell::new(None) };
    }
    if CURRENT.with(|current| current.get() == Some((style, accent))) {
        return;
    }
    let mtm = MainThreadMarker::new().expect("app icon must be installed on the main thread");
    objc2::rc::autoreleasepool(|_| {
        let data = NSData::with_bytes(png_bytes(style, accent));
        let image = NSImage::initWithData(NSImage::alloc(), &data)
            .expect("embedded app icon must decode");
        // SAFETY: called on the main thread with a valid, non-null image.
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
        // The running icon is temporary. Also set a Finder custom icon on the
        // bundle so the Dock keeps it after quit, without changing signed assets.
        let path = NSBundle::mainBundle().bundlePath().to_string();
        if !persist_bundle_icon(std::path::Path::new(&path), &image) {
            eprintln!("Couldn't persist the app icon for {path}");
        }
    });
    CURRENT.with(|current| current.set(Some((style, accent))));
}

#[cfg(target_os = "macos")]
fn persist_bundle_icon(path: &std::path::Path, image: &objc2_app_kit::NSImage) -> bool {
    use objc2_app_kit::{NSWorkspace, NSWorkspaceIconCreationOptions};
    use objc2_foundation::NSString;

    // Under cargo run the main bundle is a directory, not an app. Never put a
    // custom icon on that directory (or the development executable).
    if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app")) {
        return true;
    }
    NSWorkspace::sharedWorkspace().setIcon_forFile_options(
        Some(image),
        &NSString::from_str(&path.to_string_lossy()),
        NSWorkspaceIconCreationOptions::empty(),
    )
}

/// Windows: sets the chosen icon on Rile's windows (title bar, taskbar and
/// Alt+Tab). The .exe keeps its embedded default for Explorer and shortcuts.
#[cfg(all(windows, not(test)))]
pub fn install(style: AppIcon, accent: Accent) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, EnumWindows, GetWindowThreadProcessId, ICON_BIG, ICON_SMALL, LR_DEFAULTCOLOR, SendMessageW, WM_SETICON,
    };

    // PNG data is a valid icon resource since Windows Vista.
    let icon = |side: u32| {
        let img = image::load_from_memory(png_bytes(style, accent)).ok()?.resize(side, side, image::imageops::FilterType::Lanczos3);
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
        unsafe { CreateIconFromResourceEx(&png, true, 0x0003_0000, side as i32, side as i32, LR_DEFAULTCOLOR) }.ok()
    };
    let (Some(small), Some(big)) = (icon(32), icon(256)) else { return };
    struct Icons(isize, isize);
    unsafe extern "system" fn apply(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == std::process::id() {
            let icons = unsafe { &*(lparam.0 as *const Icons) };
            unsafe {
                SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_SMALL as usize)), Some(LPARAM(icons.0)));
                SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(icons.1)));
            }
        }
        true.into()
    }
    // The icons stay alive for the windows that now use them.
    let icons = Box::leak(Box::new(Icons(small.0 as isize, big.0 as isize)));
    unsafe { EnumWindows(Some(apply), LPARAM(icons as *const Icons as isize)).ok() };
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
pub fn install(_style: AppIcon, _accent: Accent) {}

#[cfg(test)]
mod tests {
    use super::png_bytes;
    use rile_core::settings::{Accent, AppIcon};

    #[test]
    fn every_style_and_accent_has_a_distinct_png() {
        let mut seen: Vec<&[u8]> = Vec::new();
        for style in AppIcon::ALL {
            for accent in Accent::ALL {
                let bytes = png_bytes(style, accent);
                assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "{style:?} {accent:?}");
                // IHDR width and height: 512 × 512.
                assert_eq!(&bytes[16..24], &[0, 0, 2, 0, 0, 0, 2, 0], "{style:?} {accent:?}");
                assert!(!seen.contains(&bytes), "{style:?} {accent:?} duplicates another icon");
                seen.push(bytes);
            }
        }
    }

    #[test]
    fn finder_icon_is_a_complete_icns() {
        let bytes = include_bytes!("../assets/Rile.icns");
        assert_eq!(&bytes[..4], b"icns");
        let length = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
        assert_eq!(length, bytes.len());
        assert!(bytes.windows(4).any(|tag| tag == b"ic10"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn embedded_icon_decodes_with_appkit() {
        use objc2::AnyThread;
        use objc2_app_kit::NSImage;
        use objc2_foundation::NSData;

        objc2::rc::autoreleasepool(|_| {
            for style in AppIcon::ALL {
                for accent in Accent::ALL {
                    let data = NSData::with_bytes(png_bytes(style, accent));
                    let image = NSImage::initWithData(NSImage::alloc(), &data).unwrap();
                    assert!(image.isValid(), "{style:?} {accent:?}");
                    assert!(image.size().width > 0., "{style:?} {accent:?}");
                }
            }
        });
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundle_icon_persists_style_and_accent_changes() {
        use objc2::AnyThread;
        use objc2_app_kit::{NSImage, NSWorkspace};
        use objc2_foundation::{NSData, NSString};

        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Rile.app");
        std::fs::create_dir(&bundle).unwrap();
        let path = NSString::from_str(bundle.to_str().unwrap());
        objc2::rc::autoreleasepool(|_| {
            let mut icons = Vec::new();
            for (style, accent) in [
                (AppIcon::FolioBolt, Accent::Blue),
                (AppIcon::DrawerHotRod, Accent::Blue),
                (AppIcon::DrawerHotRod, Accent::Pink),
            ] {
                let data = NSData::with_bytes(png_bytes(style, accent));
                let image = NSImage::initWithData(NSImage::alloc(), &data).unwrap();
                assert!(super::persist_bundle_icon(&bundle, &image));
                // Read back from the filesystem through Finder's API, not from
                // the running application icon or the in-memory selection.
                let stored = NSWorkspace::sharedWorkspace().iconForFile(&path);
                let bytes = stored.TIFFRepresentation().unwrap().to_vec();
                assert!(!icons.contains(&bytes), "{style:?} {accent:?} did not update the bundle icon");
                icons.push(bytes);
                assert!(super::persist_bundle_icon(dir.path(), &image));
                assert!(!dir.path().join("Icon\r").exists());
            }
            assert!(bundle.join("Icon\r").exists());
        });
    }
}
