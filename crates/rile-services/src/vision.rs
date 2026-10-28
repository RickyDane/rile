//! On-device image understanding: scene classification labels and OCR text
//! via Apple's Vision framework on macOS; OCR text (no labels — Windows has
//! no built-in classifier) via Windows.Media.Ocr on Windows.

use std::path::Path;

use anyhow::Result;

/// What Vision found in an image.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageTags {
    /// Human-readable classification labels with confidence (0..=1),
    /// highest first.
    pub labels: Vec<(String, f32)>,
    /// Recognized text, one line per observation.
    pub text: String,
}

/// Minimum classifier confidence for a label to be kept.
pub const MIN_LABEL_CONFIDENCE: f32 = 0.3;
/// Maximum number of labels returned.
pub const MAX_LABELS: usize = 15;
/// Maximum number of characters of recognized text returned.
pub const MAX_TEXT_CHARS: usize = 2000;

/// Classifies and OCRs the image at `path`. Synchronous and fairly slow
/// (hundreds of ms); call from a background thread.
#[cfg(target_os = "macos")]
pub fn analyze_image(path: &Path) -> Result<ImageTags> {
    use anyhow::Context;
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSDictionary, NSString, NSURL};
    use objc2_vision::{
        VNClassifyImageRequest, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
        VNRequestTextRecognitionLevel,
    };

    anyhow::ensure!(path.is_file(), "not a file: {}", path.display());
    let path_str = path.to_str().context("non-UTF-8 path")?;
    objc2::rc::autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
        // SAFETY: an empty options dictionary is valid.
        let handler =
            unsafe { VNImageRequestHandler::initWithURL_options(VNImageRequestHandler::alloc(), &url, &NSDictionary::new()) };

        // SAFETY: plain initializers.
        let classify = unsafe { VNClassifyImageRequest::new() };
        let ocr = VNRecognizeTextRequest::new();
        ocr.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        ocr.setUsesLanguageCorrection(true);
        ocr.setAutomaticallyDetectsLanguage(true);

        let requests: [&VNRequest; 2] = [&classify, &ocr];
        handler
            .performRequests_error(&NSArray::from_slice(&requests))
            .map_err(|e| anyhow::anyhow!("Vision failed: {}", e.localizedDescription()))?;

        let mut labels: Vec<(String, f32)> = Vec::new();
        // SAFETY: results are only read after performRequests returned.
        if let Some(results) = unsafe { classify.results() } {
            for obs in results.iter() {
                // SAFETY: plain property getters.
                let (confidence, id) = unsafe { (obs.confidence(), obs.identifier()) };
                if confidence >= MIN_LABEL_CONFIDENCE {
                    labels.push((humanize(&id.to_string()), confidence));
                }
            }
        }
        labels.sort_by(|a, b| b.1.total_cmp(&a.1));
        labels.truncate(MAX_LABELS);

        let mut lines = Vec::new();
        if let Some(results) = ocr.results() {
            for obs in results.iter() {
                if let Some(best) = obs.topCandidates(1).firstObject() {
                    let line = best.string().to_string();
                    let line = line.trim();
                    if !line.is_empty() {
                        lines.push(line.to_string());
                    }
                }
            }
        }
        Ok(ImageTags { labels, text: cap_chars(lines.join("\n"), MAX_TEXT_CHARS) })
    })
}

/// Windows: text only, in the user's profile languages.
#[cfg(windows)]
pub fn analyze_image(path: &Path) -> Result<ImageTags> {
    use anyhow::Context;
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::{FileAccessMode, StorageFile};
    use windows::core::HSTRING;

    let _com = crate::win::ComScope::new();
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().context("no OCR language installed")?;
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(dunce::simplified(path).as_os_str()))?.join().context("opening the image")?;
    let stream = file.OpenAsync(FileAccessMode::Read)?.join()?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.join().context("unsupported image")?;
    let max = OcrEngine::MaxImageDimension()?;
    if decoder.PixelWidth()? > max || decoder.PixelHeight()? > max {
        return Ok(ImageTags::default());
    }
    let bitmap = decoder.GetSoftwareBitmapAsync()?.join()?;
    let result = engine.RecognizeAsync(&bitmap)?.join().context("recognizing text")?;
    let lines = result.Lines()?;
    let text: Vec<String> = (0..lines.Size()?).filter_map(|i| lines.GetAt(i).ok()?.Text().ok().map(|t| t.to_string())).collect();
    Ok(ImageTags { labels: Vec::new(), text: cap_chars(text.join("\n"), MAX_TEXT_CHARS) })
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn analyze_image(_path: &Path) -> Result<ImageTags> {
    anyhow::bail!("image analysis is only available on macOS and Windows")
}

/// `"sky_blue"` -> `"sky blue"`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn humanize(identifier: &str) -> String {
    identifier.replace('_', " ")
}

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn cap_chars(mut s: String, max: usize) -> String {
    if let Some((idx, _)) = s.char_indices().nth(max) {
        s.truncate(idx);
    }
    s
}

#[cfg(test)]
mod unit_tests {
    #[test]
    fn helpers() {
        assert_eq!(super::humanize("sky_blue"), "sky blue");
        assert_eq!(super::cap_chars("héllo".into(), 2), "hé");
        assert_eq!(super::cap_chars("hi".into(), 5), "hi");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// A hand-written one-page PDF with large Helvetica text. CoreGraphics
    /// tolerates the missing xref table and rebuilds it.
    const TEXT_PDF: &str = "%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 600 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 52 >> stream
BT /F1 64 Tf 30 70 Td (HELLO RILE) Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF
";

    #[test]
    fn reads_text_from_rendered_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("t.pdf");
        std::fs::write(&pdf, TEXT_PDF).unwrap();
        let png = crate::pdf::render_page(&pdf, 0, 1200).unwrap();
        let img = dir.path().join("t.png");
        std::fs::write(&img, png).unwrap();

        let tags = super::analyze_image(&img).unwrap();
        assert!(tags.text.to_uppercase().contains("HELLO"), "got {tags:?}");
        assert!(tags.labels.iter().all(|(l, c)| !l.contains('_') && *c >= super::MIN_LABEL_CONFIDENCE));
    }

    #[test]
    fn missing_file_errors() {
        assert!(super::analyze_image(std::path::Path::new("/nonexistent/x.png")).is_err());
    }
}
