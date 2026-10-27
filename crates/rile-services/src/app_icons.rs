//! Native file icons. On macOS this asks NSWorkspace for the icon Finder
//! would show (application bundles, documents with custom icons, ...),
//! rendered to PNG at the requested pixel size.

use std::path::Path;

use anyhow::Result;

/// PNG bytes of the system icon for `path`, `size`×`size` pixels.
#[cfg(target_os = "macos")]
pub fn icon_png(path: &Path, size: u32) -> Result<Vec<u8>> {
    use anyhow::Context;
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    let path = path.to_str().context("non-UTF-8 path")?;
    objc2::rc::autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        let image = workspace.iconForFile(&NSString::from_str(path));
        let side = size as f64;
        let mut rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(side, side));
        // SAFETY: valid rect pointer; nil context and hints are allowed.
        let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }
            .context("icon has no bitmap representation")?;
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
        // SAFETY: an empty properties dictionary is valid for PNG.
        let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
            .context("PNG encoding failed")?;
        Ok(data.to_vec())
    })
}

#[cfg(not(target_os = "macos"))]
pub fn icon_png(_path: &Path, _size: u32) -> Result<Vec<u8>> {
    anyhow::bail!("native icons are only available on macOS")
}

/// PNG thumbnail of an image file (any format macOS reads, e.g. HEIC),
/// fitting in `edge`×`edge` pixels.
#[cfg(target_os = "macos")]
pub fn image_thumbnail_png(path: &Path, edge: u32) -> Result<Vec<u8>> {
    use anyhow::Context;
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    let path = path.to_str().context("non-UTF-8 path")?;
    objc2::rc::autoreleasepool(|_| {
        let image = NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path)).context("unreadable image")?;
        let size = image.size();
        anyhow::ensure!(size.width > 0. && size.height > 0., "empty image");
        let scale = edge as f64 / size.width.max(size.height);
        let mut rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new((size.width * scale).round().max(1.), (size.height * scale).round().max(1.)));
        // SAFETY: valid rect pointer; nil context and hints are allowed.
        let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }.context("image has no bitmap")?;
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
        // SAFETY: an empty properties dictionary is valid for PNG.
        let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
            .context("PNG encoding failed")?;
        Ok(data.to_vec())
    })
}

#[cfg(not(target_os = "macos"))]
pub fn image_thumbnail_png(_path: &Path, _edge: u32) -> Result<Vec<u8>> {
    anyhow::bail!("native image decoding is only available on macOS")
}

/// The colors of Apple Intelligence's glow, along a diagonal.
pub const INTELLIGENCE_GRADIENT: &[(f64, [f64; 3])] = &[
    (0.0, [1.00, 0.62, 0.17]),
    (0.33, [1.00, 0.23, 0.53]),
    (0.66, [0.66, 0.33, 0.97]),
    (1.0, [0.23, 0.56, 1.00]),
];

/// PNG of an SF Symbol (e.g. `"apple.intelligence"`), `height` pixels tall,
/// filled with a diagonal gradient of `(location, rgb)` stops.
#[cfg(target_os = "macos")]
pub fn symbol_png(name: &str, height: u32, gradient: &[(f64, [f64; 3])]) -> Result<Vec<u8>> {
    use anyhow::Context;
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGBlendMode, CGColorSpace, CGContext, CGGradient,
        CGGradientDrawingOptions, CGImage, CGImageAlphaInfo,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    anyhow::ensure!(height > 0 && !gradient.is_empty(), "invalid symbol size or gradient");
    objc2::rc::autoreleasepool(|_| {
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
            .with_context(|| format!("no system symbol \"{name}\""))?;
        let size = image.size();
        let aspect = if size.height > 0. { size.width / size.height } else { 1. };
        let mut rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new((height as f64 * aspect).round(), height as f64));
        // SAFETY: valid rect pointer; nil context and hints are allowed.
        let glyph = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }.context("symbol has no bitmap")?;
        let (w, h) = (CGImage::width(Some(&glyph)), CGImage::height(Some(&glyph)));
        let space = CGColorSpace::new_device_rgb().context("no RGB color space")?;
        // SAFETY: null data lets CoreGraphics allocate the buffer.
        let ctx = unsafe { CGBitmapContextCreate(std::ptr::null_mut(), w, h, 8, 0, Some(&space), CGImageAlphaInfo::PremultipliedLast.0) }
            .context("failed to create bitmap context")?;
        let components: Vec<f64> = gradient.iter().flat_map(|(_, c)| [c[0], c[1], c[2], 1.0]).collect();
        let locations: Vec<f64> = gradient.iter().map(|(l, _)| *l).collect();
        // SAFETY: both buffers hold `gradient.len()` entries (4 components each).
        let grad = unsafe { CGGradient::with_color_components(Some(&space), components.as_ptr(), locations.as_ptr(), gradient.len()) }
            .context("failed to create gradient")?;
        let (wf, hf) = (w as f64, h as f64);
        // Top-left to bottom-right (CoreGraphics' origin is bottom-left).
        CGContext::draw_linear_gradient(
            Some(&ctx),
            Some(&grad),
            CGPoint::new(0., hf),
            CGPoint::new(wf, 0.),
            CGGradientDrawingOptions::DrawsBeforeStartLocation | CGGradientDrawingOptions::DrawsAfterEndLocation,
        );
        // Keep the gradient only where the glyph is.
        CGContext::set_blend_mode(Some(&ctx), CGBlendMode::DestinationIn);
        CGContext::draw_image(Some(&ctx), CGRect::new(CGPoint::new(0., 0.), CGSize::new(wf, hf)), Some(&glyph));
        let out = CGBitmapContextCreateImage(Some(&ctx)).context("failed to snapshot bitmap")?;
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &out);
        // SAFETY: an empty properties dictionary is valid for PNG.
        let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
            .context("PNG encoding failed")?;
        Ok(data.to_vec())
    })
}

#[cfg(not(target_os = "macos"))]
pub fn symbol_png(_name: &str, _height: u32, _gradient: &[(f64, [f64; 3])]) -> Result<Vec<u8>> {
    anyhow::bail!("system symbols are only available on macOS")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn renders_an_app_icon() {
        let png = super::icon_png(std::path::Path::new("/System/Applications/Calculator.app"), 64).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn renders_the_apple_intelligence_symbol() {
        let png = super::symbol_png("apple.intelligence", 64, super::INTELLIGENCE_GRADIENT).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        std::fs::write(std::env::temp_dir().join("rile-ai-symbol.png"), &png).unwrap();
    }
}
