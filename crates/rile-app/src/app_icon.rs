//! Set the running Dock icon to match the selected accent, even under cargo run.

use rile_core::settings::Accent;

#[cfg(any(target_os = "macos", test))]
fn icon_bytes(accent: Accent) -> &'static [u8] {
    match accent {
        Accent::Blue => include_bytes!("../assets/app-icons/blue.icns"),
        Accent::Purple => include_bytes!("../assets/app-icons/purple.icns"),
        Accent::Pink => include_bytes!("../assets/app-icons/pink.icns"),
        Accent::Red => include_bytes!("../assets/app-icons/red.icns"),
        Accent::Orange => include_bytes!("../assets/app-icons/orange.icns"),
        Accent::Green => include_bytes!("../assets/app-icons/green.icns"),
        Accent::Teal => include_bytes!("../assets/app-icons/teal.icns"),
        Accent::Graphite => include_bytes!("../assets/app-icons/graphite.icns"),
    }
}

// Tests use GPUI's simulated app, not the actual NSApplication.
#[cfg(all(target_os = "macos", not(test)))]
pub fn install(accent: Accent) {
    use std::cell::Cell;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    thread_local! {
        static CURRENT: Cell<Option<Accent>> = const { Cell::new(None) };
    }
    if CURRENT.with(|current| current.get() == Some(accent)) {
        return;
    }
    let mtm = MainThreadMarker::new().expect("app icon must be installed on the main thread");
    objc2::rc::autoreleasepool(|_| {
        let data = NSData::with_bytes(icon_bytes(accent));
        let image = NSImage::initWithData(NSImage::alloc(), &data)
            .expect("embedded Pathline app icon must decode");
        // SAFETY: called on the main thread with a valid, non-null image.
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    });
    CURRENT.with(|current| current.set(Some(accent)));
}

#[cfg(any(not(target_os = "macos"), test))]
pub fn install(_accent: Accent) {}

#[cfg(test)]
mod tests {
    use super::icon_bytes;
    use rile_core::settings::Accent;

    #[test]
    fn all_accents_have_distinct_complete_icons() {
        for (i, accent) in Accent::ALL.into_iter().enumerate() {
            let bytes = icon_bytes(accent);
            assert_eq!(&bytes[..4], b"icns", "{accent:?}");
            let length = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
            assert_eq!(length, bytes.len(), "{accent:?}");
            assert!(bytes.windows(4).any(|tag| tag == b"ic10"), "{accent:?}");
            for other in &Accent::ALL[..i] {
                assert_ne!(bytes, icon_bytes(*other), "{accent:?} and {other:?}");
            }
        }
        assert_eq!(icon_bytes(Accent::Blue), include_bytes!("../assets/Rile.icns"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn embedded_icon_decodes_with_appkit() {
        use objc2::AnyThread;
        use objc2_app_kit::NSImage;
        use objc2_foundation::NSData;

        objc2::rc::autoreleasepool(|_| {
            for accent in Accent::ALL {
                let data = NSData::with_bytes(icon_bytes(accent));
                let image = NSImage::initWithData(NSImage::alloc(), &data).unwrap();
                assert!(image.isValid(), "{accent:?}");
                assert!(image.size().width > 0., "{accent:?}");
                assert!(image.size().height > 0., "{accent:?}");
            }
        });
    }
}
