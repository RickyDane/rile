//! Small Windows helpers shared by the native `mod win` backends.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::PCWSTR;

/// A NUL-terminated UTF-16 copy of `s` for `PCWSTR` parameters.
pub struct Wide(Vec<u16>);

impl Wide {
    pub fn new(s: impl AsRef<OsStr>) -> Self {
        Self(s.as_ref().encode_wide().chain(Some(0)).collect())
    }

    pub fn path(p: &Path) -> Self {
        Self::new(p.as_os_str())
    }

    pub fn pcwstr(&self) -> PCWSTR {
        PCWSTR(self.0.as_ptr())
    }
}

/// Initialises COM (apartment-threaded, as the Shell needs) for the current
/// thread while it lives. Safe to nest: an already-initialised thread keeps
/// its mode and the guard just balances its own call.
pub struct ComScope(bool);

impl ComScope {
    pub fn new() -> Self {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        // S_OK / S_FALSE need a matching CoUninitialize; RPC_E_CHANGED_MODE doesn't.
        Self(hr.is_ok())
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// UTF-16 buffer (possibly NUL-terminated) to a `String`.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// PNG of what Explorer shows for `path` (`SIIGBF_ICONONLY`: its icon,
/// `SIIGBF_THUMBNAILONLY`: its thumbnail, from the shell's thumbnail cache
/// and installed codecs such as HEIF), fitting in `size`×`size`.
pub fn shell_image_png(path: &Path, size: u32, flags: windows::Win32::UI::Shell::SIIGBF) -> anyhow::Result<Vec<u8>> {
    use anyhow::{Context, ensure};
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HGDIOBJ, ReleaseDC,
    };
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName};

    let _com = ComScope::new();
    let path = Wide::path(dunce::simplified(path));
    let factory: IShellItemImageFactory = unsafe { SHCreateItemFromParsingName(path.pcwstr(), None) }.context("shell item")?;
    let side = size as i32;
    let bitmap = unsafe { factory.GetImage(SIZE { cx: side, cy: side }, flags) }.context("no image for this item")?;
    let object = HGDIOBJ(bitmap.0);
    let result = (|| {
        let mut bm = BITMAP::default();
        ensure!(unsafe { GetObjectW(object, size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) } != 0, "bitmap info");
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        ensure!(w > 0 && h > 0, "empty bitmap");
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Negative: rows top to bottom.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let dc = unsafe { GetDC(None) };
        let lines = unsafe { GetDIBits(dc, bitmap, 0, h as u32, Some(pixels.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS) };
        unsafe { ReleaseDC(None, dc) };
        ensure!(lines == h, "reading bitmap");
        // Photos come without alpha (all zero): make them opaque. Icons are
        // premultiplied BGRA.
        let opaque = pixels.chunks_exact(4).all(|p| p[3] == 0);
        for p in pixels.chunks_exact_mut(4) {
            p.swap(0, 2);
            if opaque {
                p[3] = 255;
            } else if p[3] > 0 && p[3] < 255 {
                let a = p[3] as u32;
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        let img = image::RgbaImage::from_raw(w as u32, h as u32, pixels).context("bitmap size")?;
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        Ok(png)
    })();
    unsafe { DeleteObject(object).ok().ok() };
    result
}

/// Runs a Windows Search query (Advanced Query Syntax, e.g. `kind:picture`
/// or plain words) through a `search-ms:` shell folder, scoped to `root`,
/// calling `found` for each file-system result until it returns `false` or
/// `cancel` is set. Uses the system index; unindexed locations are crawled.
pub fn windows_search(
    query: &str,
    root: Option<&Path>,
    cancel: &std::sync::atomic::AtomicBool,
    mut found: impl FnMut(std::path::PathBuf) -> bool,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::sync::atomic::Ordering;
    use windows::Win32::UI::Shell::{BHID_EnumItems, IEnumShellItems, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH};

    // search-ms values are percent-encoded.
    fn encode(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    }
    let _com = ComScope::new();
    let mut uri = format!("search-ms:query={}", encode(query));
    if let Some(root) = root {
        uri.push_str(&format!("&crumb=location:{}", encode(&dunce::simplified(root).to_string_lossy())));
    }
    let folder: IShellItem = unsafe { SHCreateItemFromParsingName(&windows::core::HSTRING::from(uri), None) }.context("Windows Search")?;
    let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.context("Windows Search")?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut batch = [None];
        let mut fetched = 0u32;
        if unsafe { items.Next(&mut batch, Some(&mut fetched)) }.is_err() || fetched == 0 {
            return Ok(());
        }
        let Some(item) = batch[0].take() else { continue };
        // Non-file results (mail, settings) have no file-system path.
        let Ok(name) = (unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }) else { continue };
        let path = std::path::PathBuf::from(unsafe { name.to_string() }.unwrap_or_default());
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(name.0 as *const _)) };
        if !path.as_os_str().is_empty() && !found(path) {
            return Ok(());
        }
    }
}
