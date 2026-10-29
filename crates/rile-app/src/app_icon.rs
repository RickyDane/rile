//! Platform app artwork, plus macOS Dock and bundle icon customization.

use rile_core::settings::{Accent, AppIcon};

include!("app_icon_table.rs");

pub(crate) const WINDOWS_PNG: &[u8] = include_bytes!("../assets/app-icons/windows/folded-folio.png");

fn ui_png_bytes(style: AppIcon, accent: Accent) -> &'static [u8] {
    if cfg!(windows) { WINDOWS_PNG } else { png_bytes(style, accent) }
}

/// The icon as an image for drawing in the UI (cached, so renders stay cheap).
pub fn png(style: AppIcon, accent: Accent) -> Option<std::sync::Arc<gpui_kit::Image>> {
    // Windows always uses Folded Folio, including in About. Retain the
    // saved preference so settings remain compatible across platforms.
    let (style, accent) = if cfg!(windows) { (AppIcon::default(), Accent::Blue) } else { (style, accent) };
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
            .or_insert_with(|| Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, ui_png_bytes(style, accent).to_vec())))
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

#[cfg(any(not(target_os = "macos"), test))]
pub fn install(_style: AppIcon, _accent: Accent) {}

#[cfg(test)]
mod tests {
    use super::png_bytes;
    use rile_core::settings::{Accent, AppIcon};

    #[cfg(windows)]
    #[test]
    fn windows_ignores_saved_icon_preferences() {
        let default = super::png(AppIcon::default(), Accent::Blue).unwrap();
        for style in AppIcon::ALL {
            for accent in Accent::ALL {
                let image = super::png(style, accent).unwrap();
                assert!(std::sync::Arc::ptr_eq(&default, &image));
                assert_eq!(super::ui_png_bytes(style, accent), super::WINDOWS_PNG);
            }
        }
    }

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
    fn ui_artwork_matches_the_platform() {
        for style in AppIcon::ALL {
            for accent in Accent::ALL {
                let expected = if cfg!(windows) { super::WINDOWS_PNG } else { png_bytes(style, accent) };
                assert_eq!(super::ui_png_bytes(style, accent), expected);
            }
        }
    }

    fn assert_transparent_folio(bytes: &[u8], size: u32) {
        let image = image::load_from_memory(bytes).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (size, size));
        // Empty space above the folder must stay transparent, not a macOS tile.
        assert_eq!(image.get_pixel(size / 2, size / 8).0[3], 0);
        for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
            assert_eq!(image.get_pixel(x, y).0[3], 0);
        }
        assert_eq!(image.get_pixel(size / 2, size * 5 / 8).0[3], 255);
    }

    #[test]
    fn windows_folio_png_is_transparent() {
        assert_transparent_folio(super::WINDOWS_PNG, 512);
    }

    #[test]
    fn explorer_icon_has_all_sizes_and_transparency() {
        let bytes = include_bytes!("../assets/Rile.ico");
        let sizes = [16u32, 24, 32, 48, 64, 128, 256];
        assert_eq!(&bytes[..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes(bytes[4..6].try_into().unwrap()) as usize, sizes.len());
        for (index, size) in sizes.into_iter().enumerate() {
            let entry = &bytes[6 + index * 16..6 + (index + 1) * 16];
            assert_eq!(if entry[0] == 0 { 256 } else { entry[0] as u32 }, size);
            assert_eq!(entry[0], entry[1]);
            let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
            assert_transparent_folio(&bytes[offset..offset + length], size);
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
