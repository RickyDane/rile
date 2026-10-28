//! Whole-library image discovery for the gallery view, plus cheap per-image
//! metadata (dimensions, capture date).
//!
//! On macOS images come from Spotlight (`mdfind`), streamed in batches, plus a
//! direct walk of Photos library `originals` folders, which Spotlight usually
//! doesn't index. Elsewhere `~/Pictures` is walked.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// Paths per `on_batch` call.
pub const BATCH_SIZE: usize = 500;

/// Image file extensions recognised by the directory walks (lowercase).
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "heic", "heif", "webp", "tif", "tiff", "bmp", "raw", "dng", "cr2", "cr3",
    "nef", "arw", "orf", "rw2", "raf", "avif", "jxl",
];

/// Substrings that mark paths not worth showing (caches, app internals, ...).
const EXCLUDED: &[&str] = &[
    "/Library/Caches",
    "/.Trash",
    ".app/",
    "/node_modules/",
    "/Library/Containers",
    "/Library/Application Support",
    "/Library/Group Containers",
    "/.git/",
];

/// Dimensions and capture time of an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageMeta {
    /// Display width in pixels (EXIF rotation applied).
    pub width: u32,
    /// Display height in pixels (EXIF rotation applied).
    pub height: u32,
    /// EXIF capture date when present, otherwise the file's creation or
    /// modification time.
    pub taken: Option<SystemTime>,
}

/// Streams every image in the user's home folder to `on_batch`, in batches of
/// up to [`BATCH_SIZE`] deduplicated paths. Returns early (killing `mdfind`)
/// once `cancel` is set.
pub fn find_images(cancel: &AtomicBool, mut on_batch: impl FnMut(Vec<PathBuf>)) {
    let Some(home) = home_dir() else { return };
    let mut sink = Sink { seen: HashSet::new(), batch: Vec::new(), on_batch: &mut on_batch };

    #[cfg(target_os = "macos")]
    {
        spotlight(&home, cancel, &mut sink);
        // Photos libraries: mdfind rarely indexes inside the bundle.
        if let Ok(entries) = std::fs::read_dir(home.join("Pictures")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("photoslibrary")) {
                    walk(&path.join("originals"), cancel, &mut sink);
                }
            }
        }
    }
    // Windows Search finds pictures anywhere in the profile; the Pictures
    // folder (which may be redirected to OneDrive) is walked as well, for
    // when indexing is off.
    #[cfg(windows)]
    {
        let _ = crate::win::windows_search("kind:picture", Some(&home), cancel, |p| {
            if has_image_extension(&p) && !is_excluded(&p.to_string_lossy()) {
                sink.push(p);
            }
            true
        });
        walk(&dirs::picture_dir().unwrap_or_else(|| home.join("Pictures")), cancel, &mut sink);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    walk(&home.join("Pictures"), cancel, &mut sink);

    if !cancel.load(Ordering::Relaxed) {
        sink.flush();
    }
}

/// Dimensions and capture date of the image at `path`, read from its header
/// (no full decode). `None` if it isn't a readable image.
#[cfg(target_os = "macos")]
pub fn image_meta(path: &Path) -> Option<ImageMeta> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_core_foundation::{CFDictionary, CFURL};
    use objc2_foundation::{NSDictionary, NSNumber, NSString};
    use objc2_image_io::CGImageSource;

    fn get(dict: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<Retained<AnyObject>> {
        dict.objectForKey(&NSString::from_str(key))
    }
    fn number(dict: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<i64> {
        get(dict, key)?.downcast::<NSNumber>().ok().map(|n| n.as_i64())
    }

    objc2::rc::autoreleasepool(|_| {
        let url = CFURL::from_file_path(path)?;
        // SAFETY: nil options are allowed.
        let source = unsafe { CGImageSource::with_url(&url, None) }?;
        // SAFETY: valid index 0 (checked by the call itself); nil options.
        let props = unsafe { source.properties_at_index(0, None) }?;
        // SAFETY: CFDictionary is toll-free bridged to NSDictionary, and
        // ImageIO property dictionaries are keyed by strings.
        let props: &NSDictionary<NSString, AnyObject> =
            unsafe { &*(&*props as *const CFDictionary as *const NSDictionary<NSString, AnyObject>) };

        let mut width = u32::try_from(number(props, "PixelWidth")?).ok()?;
        let mut height = u32::try_from(number(props, "PixelHeight")?).ok()?;
        // EXIF orientations 5..=8 are rotated by 90 degrees.
        if matches!(number(props, "Orientation"), Some(5..=8)) {
            std::mem::swap(&mut width, &mut height);
        }
        let exif_date = get(props, "{Exif}")
            .and_then(|e| e.downcast::<NSDictionary>().ok())
            .and_then(|exif| {
                // SAFETY: EXIF dictionaries are keyed by strings.
                let exif: &NSDictionary<NSString, AnyObject> =
                    unsafe { &*(&*exif as *const NSDictionary as *const NSDictionary<NSString, AnyObject>) };
                get(exif, "DateTimeOriginal")
            })
            .and_then(|d| d.downcast::<NSString>().ok())
            .and_then(|s| parse_exif_date(&s.to_string()));
        Some(ImageMeta { width, height, taken: exif_date.or_else(|| file_time(path)) })
    })
}

/// Windows: the Shell's photo properties (the same ones Explorer's Details
/// pane shows), read through the installed property handlers.
#[cfg(windows)]
pub fn image_meta(path: &Path) -> Option<ImageMeta> {
    use windows::Win32::Storage::EnhancedStorage::{PKEY_Image_HorizontalSize, PKEY_Image_VerticalSize, PKEY_Photo_DateTaken, PKEY_Photo_Orientation};
    use windows::Win32::UI::Shell::{IShellItem2, SHCreateItemFromParsingName};

    if !has_image_extension(path) || !path.is_file() {
        return None;
    }
    let _com = crate::win::ComScope::new();
    let wide = crate::win::Wide::path(dunce::simplified(path));
    let item: IShellItem2 = unsafe { SHCreateItemFromParsingName(wide.pcwstr(), None) }.ok()?;
    let (mut width, mut height) = unsafe { (item.GetUInt32(&PKEY_Image_HorizontalSize).unwrap_or(0), item.GetUInt32(&PKEY_Image_VerticalSize).unwrap_or(0)) };
    // EXIF orientations 5–8 are rotated by 90°.
    if unsafe { item.GetUInt32(&PKEY_Photo_Orientation) }.is_ok_and(|o| (5..=8).contains(&o)) {
        std::mem::swap(&mut width, &mut height);
    }
    let taken = unsafe { item.GetFileTime(&PKEY_Photo_DateTaken) }.ok().and_then(|ft| {
        // 100 ns ticks since 1601-01-01.
        let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        let unix = (ticks / 10_000_000).checked_sub(11_644_473_600)?;
        Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(unix))
    });
    Some(ImageMeta { width, height, taken: taken.or_else(|| file_time(path)) })
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn image_meta(path: &Path) -> Option<ImageMeta> {
    // Without a header parser only the date is cheap; dimensions stay 0.
    if !has_image_extension(path) || !path.is_file() {
        return None;
    }
    Some(ImageMeta { width: 0, height: 0, taken: file_time(path) })
}

fn file_time(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    meta.created().or_else(|_| meta.modified()).ok()
}

/// Parses EXIF `"YYYY:MM:DD HH:MM:SS"` (local time, no zone).
#[cfg(target_os = "macos")]
fn parse_exif_date(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i32>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, min, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if year < 1900 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // SAFETY: zeroed tm is a valid starting value; mktime only reads/normalises it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = min;
    tm.tm_sec = sec;
    tm.tm_isdst = -1;
    // SAFETY: valid pointer to an initialised tm.
    let secs = unsafe { libc::mktime(&mut tm) };
    if secs < 0 {
        return None;
    }
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
}

struct Sink<'a, F: FnMut(Vec<PathBuf>)> {
    seen: HashSet<PathBuf>,
    batch: Vec<PathBuf>,
    on_batch: &'a mut F,
}

impl<F: FnMut(Vec<PathBuf>)> Sink<'_, F> {
    fn push(&mut self, path: PathBuf) {
        if self.seen.insert(path.clone()) {
            self.batch.push(path);
            if self.batch.len() >= BATCH_SIZE {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        if !self.batch.is_empty() {
            (self.on_batch)(std::mem::take(&mut self.batch));
        }
    }
}

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn is_excluded(path: &str) -> bool {
    // The patterns use `/`; Windows paths use `\`.
    let normalized;
    let path = if cfg!(windows) {
        normalized = path.replace('\\', "/");
        if normalized.contains("/AppData/") || normalized.contains("/$Recycle.Bin/") {
            return true;
        }
        normalized.as_str()
    } else {
        path
    };
    if path.contains(".photoslibrary/") {
        // Only the originals of a Photos library; skip its derivatives,
        // thumbnails and database.
        return !path.contains(".photoslibrary/originals/");
    }
    EXCLUDED.iter().any(|x| path.contains(x))
}

fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Recursive directory walk (no symlink following) collecting image files.
fn walk<F: FnMut(Vec<PathBuf>)>(root: &Path, cancel: &AtomicBool, sink: &mut Sink<'_, F>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                if !rile_core::fs_entry::is_hidden_entry(&entry) {
                    stack.push(path);
                }
            } else if ft.is_file() && has_image_extension(&path) {
                sink.push(path);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn spotlight<F: FnMut(Vec<PathBuf>)>(home: &Path, cancel: &AtomicBool, sink: &mut Sink<'_, F>) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    let Ok(mut child) = Command::new("/usr/bin/mdfind")
        .arg("-0")
        .arg("-onlyin")
        .arg(home)
        .arg("kMDItemContentTypeTree == 'public.image'")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let Some(stdout) = child.stdout.take() else { return };

    // Read on a helper thread so a quiet mdfind can't block cancellation.
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(4096);
    let reader = std::thread::spawn(move || {
        for record in BufReader::new(stdout).split(0) {
            let Ok(record) = record else { break };
            if tx.send(record).is_err() {
                break;
            }
        }
    });

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(record) => {
                use std::os::unix::ffi::OsStringExt;
                let path = PathBuf::from(std::ffi::OsString::from_vec(record));
                if path.as_os_str().is_empty() {
                    continue;
                }
                if !is_excluded(&path.to_string_lossy()) {
                    sink.push(path);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(rx);
    let _ = child.wait();
    let _ = reader.join();
}

fn home_dir() -> Option<PathBuf> {
    dirs::home_dir().filter(|h| !h.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusions() {
        assert!(is_excluded("/Users/a/Library/Caches/x.png"));
        assert!(is_excluded("/Users/a/Applications/Foo.app/Contents/Resources/i.png"));
        assert!(is_excluded("/Users/a/p/node_modules/x/logo.png"));
        assert!(is_excluded("/Users/a/Pictures/Photos Library.photoslibrary/resources/derivatives/x.jpg"));
        assert!(!is_excluded("/Users/a/Pictures/Photos Library.photoslibrary/originals/A/x.heic"));
        assert!(!is_excluded("/Users/a/Pictures/holiday.jpg"));
    }

    #[test]
    fn walk_dedupes_and_batches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        for i in 0..3 {
            std::fs::write(dir.path().join(format!("{i}.JPG")), b"x").unwrap();
        }
        std::fs::write(dir.path().join("sub/a.png"), b"x").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        let cancel = AtomicBool::new(false);
        let mut got = Vec::new();
        let mut on_batch = |b: Vec<PathBuf>| got.extend(b);
        let mut sink = Sink { seen: HashSet::new(), batch: Vec::new(), on_batch: &mut on_batch };
        walk(dir.path(), &cancel, &mut sink);
        walk(dir.path(), &cancel, &mut sink);
        sink.flush();
        assert_eq!(got.len(), 4);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn exif_dates() {
        assert!(parse_exif_date("2021:07:04 12:30:00").is_some());
        assert!(parse_exif_date("0000:00:00 00:00:00").is_none());
        assert!(parse_exif_date("garbage").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn meta_of_generated_png() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("t.pdf");
        std::fs::write(&pdf, "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 100] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n").unwrap();
        let png = dir.path().join("t.png");
        std::fs::write(&png, crate::pdf::render_page(&pdf, 0, 300).unwrap()).unwrap();
        let meta = image_meta(&png).unwrap();
        assert_eq!((meta.width, meta.height), (300, 100));
        assert!(meta.taken.is_some());
        assert!(image_meta(&pdf.with_extension("missing")).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cancel_stops_spotlight_quickly() {
        let cancel = AtomicBool::new(true);
        let start = std::time::Instant::now();
        find_images(&cancel, |_| panic!("no batches after cancel"));
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
}
