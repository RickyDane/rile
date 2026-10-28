//! Set the running Dock icon to the selected style and accent, even under cargo run.

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
    use objc2_foundation::NSData;

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
    });
    CURRENT.with(|current| current.set(Some((style, accent))));
}

#[cfg(any(not(target_os = "macos"), test))]
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
}
