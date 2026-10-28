//! PDF page counting and rasterization (CoreGraphics on macOS,
//! Windows.Data.Pdf on Windows).

#[cfg(not(windows))]
use std::path::Path;

#[cfg(not(windows))]
use anyhow::Result;

/// Number of pages in the PDF at `path`.
#[cfg(target_os = "macos")]
pub fn page_count(path: &Path) -> Result<usize> {
    objc2::rc::autoreleasepool(|_| Ok(mac::open(path)?.number_of_pages()))
}

#[cfg(windows)]
pub use win::{page_count, render_page};

#[cfg(not(any(target_os = "macos", windows)))]
pub fn page_count(_path: &Path) -> Result<usize> {
    anyhow::bail!("PDF rendering is only available on macOS")
}

/// Renders page `page_index` (0-based) of the PDF at `path` onto a white
/// background, scaled so its longer side is `max_px` pixels, as PNG bytes.
#[cfg(target_os = "macos")]
pub fn render_page(path: &Path, page_index: usize, max_px: u32) -> Result<Vec<u8>> {
    use anyhow::Context;
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext, CGImageAlphaInfo,
        CGInterpolationQuality, CGPDFBox, CGPDFPage,
    };
    use objc2_foundation::NSDictionary;

    anyhow::ensure!(max_px > 0, "max_px must be positive");
    objc2::rc::autoreleasepool(|_| {
        let doc = mac::open(path)?;
        let count = doc.number_of_pages();
        anyhow::ensure!(page_index < count, "page {page_index} out of range ({count} pages)");
        // CGPDFDocumentGetPage is 1-based.
        let page = doc.page(page_index + 1).context("missing PDF page")?;

        let crop = CGPDFPage::box_rect(Some(&page), CGPDFBox::CropBox);
        let rotation = CGPDFPage::rotation_angle(Some(&page)).rem_euclid(360);
        let (mut pw, mut ph) = (crop.size.width.abs(), crop.size.height.abs());
        if rotation == 90 || rotation == 270 {
            std::mem::swap(&mut pw, &mut ph);
        }
        anyhow::ensure!(pw > 0. && ph > 0., "empty PDF page");
        let scale = max_px as f64 / pw.max(ph);
        let width = ((pw * scale).round() as usize).max(1);
        let height = ((ph * scale).round() as usize).max(1);

        let space = CGColorSpace::new_device_rgb().context("no RGB color space")?;
        // SAFETY: null data lets CoreGraphics allocate the buffer.
        let ctx = unsafe {
            CGBitmapContextCreate(
                std::ptr::null_mut(),
                width,
                height,
                8,
                0,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .context("failed to create bitmap context")?;
        let target = CGRect::new(CGPoint::new(0., 0.), CGSize::new(width as f64, height as f64));
        CGContext::set_rgb_fill_color(Some(&ctx), 1., 1., 1., 1.);
        CGContext::fill_rect(Some(&ctx), target);
        CGContext::set_interpolation_quality(Some(&ctx), CGInterpolationQuality::High);
        // Map the (rotated) crop box onto the bitmap. getDrawingTransform
        // never scales up, so scale the context first and fit into the
        // unscaled rect.
        CGContext::scale_ctm(Some(&ctx), scale, scale);
        let unscaled = CGRect::new(CGPoint::new(0., 0.), CGSize::new(pw, ph));
        let transform = CGPDFPage::drawing_transform(Some(&page), CGPDFBox::CropBox, unscaled, 0, true);
        CGContext::concat_ctm(Some(&ctx), transform);
        CGContext::clip_to_rect(Some(&ctx), crop);
        CGContext::draw_pdf_page(Some(&ctx), Some(&page));

        let image = CGBitmapContextCreateImage(Some(&ctx)).context("failed to snapshot bitmap")?;
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &image);
        // SAFETY: an empty properties dictionary is valid for PNG.
        let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
            .context("PNG encoding failed")?;
        Ok(data.to_vec())
    })
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn render_page(_path: &Path, _page_index: usize, _max_px: u32) -> Result<Vec<u8>> {
    anyhow::bail!("PDF rendering is only available on macOS")
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::Path;

    use anyhow::{Context, Result};
    use objc2_core_foundation::{CFRetained, CFURL};
    use objc2_core_graphics::CGPDFDocument;

    pub struct Doc(CFRetained<CGPDFDocument>);

    impl Doc {
        pub fn number_of_pages(&self) -> usize {
            CGPDFDocument::number_of_pages(Some(&self.0))
        }

        pub fn page(&self, number: usize) -> Option<CFRetained<objc2_core_graphics::CGPDFPage>> {
            CGPDFDocument::page(Some(&self.0), number)
        }
    }

    pub fn open(path: &Path) -> Result<Doc> {
        let url = CFURL::from_file_path(path).context("invalid path")?;
        let doc = CGPDFDocument::with_url(Some(&url))
            .with_context(|| format!("not a readable PDF: {}", path.display()))?;
        if CGPDFDocument::is_encrypted(Some(&doc)) && !CGPDFDocument::is_unlocked(Some(&doc)) {
            // Many "encrypted" PDFs only restrict printing and open with an
            // empty password.
            // SAFETY: a valid NUL-terminated C string.
            let unlocked = unsafe { CGPDFDocument::unlock_with_password(Some(&doc), std::ptr::NonNull::from(c"").cast()) };
            anyhow::ensure!(unlocked, "PDF is password-protected");
        }
        Ok(Doc(doc))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::path::Path;

    use objc2_core_foundation::{CFURL, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGContext, CGPDFContextBeginPage, CGPDFContextClose, CGPDFContextCreateWithURL, CGPDFContextEndPage};

    /// Writes a PDF with `pages` landscape pages, each with a red square.
    fn make_pdf(path: &Path, pages: usize) {
        let url = CFURL::from_file_path(path).unwrap();
        let media = CGRect::new(CGPoint::new(0., 0.), CGSize::new(400., 200.));
        let ctx = unsafe { CGPDFContextCreateWithURL(Some(&url), &media, None) }.unwrap();
        for _ in 0..pages {
            unsafe { CGPDFContextBeginPage(Some(&ctx), None) };
            CGContext::set_rgb_fill_color(Some(&ctx), 1., 0., 0., 1.);
            CGContext::fill_rect(Some(&ctx), CGRect::new(CGPoint::new(10., 10.), CGSize::new(50., 50.)));
            CGPDFContextEndPage(Some(&ctx));
        }
        CGPDFContextClose(Some(&ctx));
    }

    #[test]
    fn counts_and_renders_pages() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("t.pdf");
        make_pdf(&pdf, 3);
        assert_eq!(super::page_count(&pdf).unwrap(), 3);

        let png = super::render_page(&pdf, 0, 256).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        // IHDR width/height (big-endian) at bytes 16..24.
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((w, h), (256, 128));

        assert!(super::render_page(&pdf, 3, 256).is_err());
    }

    #[test]
    fn rejects_non_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.pdf");
        std::fs::write(&f, "not a pdf").unwrap();
        assert!(super::page_count(&f).is_err());
    }
}

#[cfg(windows)]
mod win {
    use std::path::Path;

    use anyhow::{Context, Result};
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Storage::StorageFile;
    use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
    use windows::UI::Color;
    use windows::core::HSTRING;

    fn open(path: &Path) -> Result<PdfDocument> {
        let path = HSTRING::from(dunce::simplified(path).as_os_str());
        let file = StorageFile::GetFileFromPathAsync(&path)?.join().context("opening the PDF")?;
        PdfDocument::LoadFromFileAsync(&file)?.join().context("reading the PDF")
    }

    pub fn page_count(path: &Path) -> Result<usize> {
        let _com = crate::win::ComScope::new();
        Ok(open(path)?.PageCount()? as usize)
    }

    pub fn render_page(path: &Path, page_index: usize, max_px: u32) -> Result<Vec<u8>> {
        let _com = crate::win::ComScope::new();
        let doc = open(path)?;
        let page = doc.GetPage(u32::try_from(page_index)?).context("no such page")?;
        let size = page.Size()?;
        anyhow::ensure!(size.Width > 0. && size.Height > 0., "empty page");
        let scale = max_px as f32 / size.Width.max(size.Height);
        let options = PdfPageRenderOptions::new()?;
        options.SetDestinationWidth((size.Width * scale).round().max(1.) as u32)?;
        options.SetDestinationHeight((size.Height * scale).round().max(1.) as u32)?;
        options.SetBackgroundColor(Color { A: 255, R: 255, G: 255, B: 255 })?;
        // PNG is the default encoding.
        let stream = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)?.join().context("rendering the page")?;
        let len = u32::try_from(stream.Size()?)?;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(len)?.join()?;
        let mut png = vec![0u8; len as usize];
        reader.ReadBytes(&mut png)?;
        Ok(png)
    }
}
