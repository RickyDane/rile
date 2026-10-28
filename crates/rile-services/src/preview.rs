//! Quick-look style previews: images by extension, UTF-8 text, a hexdump for
//! binary files and a name listing for folders. Reads at most a bounded
//! prefix of any file.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rile_core::fs_entry::natural_cmp;

/// Extensions rendered as images by the UI.
pub const IMAGE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "tiff", "tif", "avif"];
/// Bytes inspected for NUL when deciding text vs binary.
const SNIFF_BYTES: usize = 8192;
const HEX_BYTES: usize = 512;
const DIR_PREVIEW_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Image(PathBuf),
    Text {
        text: String,
        truncated: bool,
        /// Lowercase file extension, as a syntax highlighting hint.
        language: Option<String>,
    },
    Binary {
        size: u64,
        /// Hexdump of the first 512 bytes.
        hex: String,
    },
    Directory {
        /// First 200 non-hidden names, sorted naturally.
        entries: Vec<String>,
        /// Total number of non-hidden entries.
        total: usize,
    },
    Unsupported(String),
}

/// Loads a preview of `path`, reading at most `max_text_bytes` of text.
pub fn load(path: &Path, max_text_bytes: usize) -> Result<Preview> {
    let meta = fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    if meta.is_dir() {
        return load_dir(path);
    }
    if !meta.is_file() {
        // Never open FIFOs, sockets or devices: reading could block forever.
        return Ok(Preview::Unsupported("Special file".into()));
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    if ext.as_deref().is_some_and(|e| IMAGE_EXTENSIONS.contains(&e)) {
        return Ok(Preview::Image(path.to_path_buf()));
    }

    let size = meta.len();
    let want = max_text_bytes.max(SNIFF_BYTES) as u64;
    let mut buf = Vec::with_capacity(want.min(size) as usize);
    fs::File::open(path)
        .with_context(|| format!("Cannot open {}", path.display()))?
        .take(want)
        .read_to_end(&mut buf)?;
    let read_all = (buf.len() as u64) < want;

    match decode_text(&buf, max_text_bytes, read_all) {
        Some((text, cut)) => {
            let truncated = cut || !read_all;
            Ok(Preview::Text { text, truncated, language: ext.filter(|e| !e.is_empty()) })
        }
        None => Ok(Preview::Binary { size, hex: hexdump(&buf[..buf.len().min(HEX_BYTES)]) }),
    }
}

/// Returns the text if `buf` looks like UTF-8 text (or UTF-16 with a byte
/// order mark, as Windows' "Unicode" files are), truncated to at most
/// `max_bytes` on a char boundary, and whether it was cut there. `complete`
/// says whether `buf` is the whole file (if not, an incomplete trailing char
/// is tolerated).
fn decode_text(buf: &[u8], max_bytes: usize, complete: bool) -> Option<(String, bool)> {
    let utf16 = match buf {
        [0xff, 0xfe, rest @ ..] => Some((rest, u16::from_le_bytes as fn([u8; 2]) -> u16)),
        [0xfe, 0xff, rest @ ..] => Some((rest, u16::from_be_bytes as fn([u8; 2]) -> u16)),
        _ => None,
    };
    // A whole file of UTF-16 has an even length.
    if let Some((rest, unit)) = utf16.filter(|(rest, _)| !complete || rest.len() % 2 == 0) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| unit([c[0], c[1]])).collect();
        let text = String::from_utf16_lossy(&units);
        if text.contains('\0') {
            return None;
        }
        return Some(cut_at(&text, max_bytes));
    }
    let buf = buf.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(buf);
    if buf[..buf.len().min(SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let valid = match std::str::from_utf8(buf) {
        Ok(s) => s,
        // Incomplete multi-byte sequence at the very end of a partial read.
        Err(e) if e.error_len().is_none() && !complete => {
            // SAFETY-free: valid_up_to is a char boundary by definition.
            std::str::from_utf8(&buf[..e.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    Some(cut_at(valid, max_bytes))
}

fn cut_at(text: &str, max_bytes: usize) -> (String, bool) {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), end < text.len())
}

fn load_dir(path: &Path) -> Result<Preview> {
    let mut names: Vec<(String, String)> = fs::read_dir(path)
        .with_context(|| format!("Cannot list {}", path.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| !rile_core::fs_entry::is_hidden_entry(e))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .map(|n| (n.to_lowercase(), n))
        .collect();
    let total = names.len();
    let cmp = |a: &(String, String), b: &(String, String)| natural_cmp(&a.0, &b.0).then_with(|| a.1.cmp(&b.1));
    if names.len() > DIR_PREVIEW_ENTRIES {
        names.select_nth_unstable_by(DIR_PREVIEW_ENTRIES, cmp);
        names.truncate(DIR_PREVIEW_ENTRIES);
    }
    names.sort_unstable_by(cmp);
    Ok(Preview::Directory { entries: names.into_iter().map(|(_, n)| n).collect(), total })
}

/// Classic `hexdump -C` style output, 16 bytes per line.
pub fn hexdump(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 4 + 16);
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let _ = write!(out, "{:08x}  ", i * 16);
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => {
                    let _ = write!(out, "{b:02x} ");
                }
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        out.extend(chunk.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' }));
        out.push_str("|\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_preview() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a.RS");
        fs::write(&f, "fn main() {}\n").unwrap();
        assert_eq!(
            load(&f, 1024).unwrap(),
            Preview::Text { text: "fn main() {}\n".into(), truncated: false, language: Some("rs".into()) }
        );
        let e = t.path().join("empty");
        fs::write(&e, "").unwrap();
        assert_eq!(load(&e, 1024).unwrap(), Preview::Text { text: String::new(), truncated: false, language: None });
    }

    #[test]
    fn truncated_utf8_text() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("big.txt");
        // 3-byte chars; 10000 of them, so the read boundary splits one.
        fs::write(&f, "€".repeat(10_000)).unwrap();
        let Preview::Text { text, truncated, .. } = load(&f, 100).unwrap() else { panic!("not text") };
        assert!(truncated);
        assert_eq!(text, "€".repeat(33));
        let Preview::Text { text, truncated, .. } = load(&f, 10_000).unwrap() else { panic!("not text") };
        assert!(truncated);
        assert_eq!(text.len(), 9999);
    }

    #[test]
    fn binary_preview() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("x.bin");
        let data: Vec<u8> = (0..=255u8).cycle().take(2000).collect();
        fs::write(&f, &data).unwrap();
        let Preview::Binary { size, hex } = load(&f, 1024).unwrap() else { panic!("not binary") };
        assert_eq!(size, 2000);
        assert_eq!(hex.lines().count(), 32);
        assert!(hex.starts_with("00000000  00 01 02 03 04 05 06 07  08 09"));
        // Invalid UTF-8 without NUL is binary too.
        fs::write(&f, [0xffu8, 0xfe, 0x41]).unwrap();
        assert!(matches!(load(&f, 1024).unwrap(), Preview::Binary { size: 3, .. }));
    }

    #[test]
    fn image_and_dir_preview() {
        let t = tempfile::tempdir().unwrap();
        let img = t.path().join("photo.JPG");
        fs::write(&img, b"not really").unwrap();
        assert_eq!(load(&img, 10).unwrap(), Preview::Image(img.clone()));
        for n in ["file10", "file2", "File1", ".hidden"] {
            fs::write(t.path().join(n), "").unwrap();
        }
        for i in 0..300 {
            fs::write(t.path().join(format!("z{i}")), "").unwrap();
        }
        let Preview::Directory { entries, total } = load(t.path(), 10).unwrap() else { panic!("not dir") };
        assert_eq!(total, 304);
        assert_eq!(entries.len(), 200);
        assert_eq!(&entries[..4], ["File1", "file2", "file10", "photo.JPG"]);
        assert_eq!(entries[4], "z0");
        assert_eq!(entries[5], "z1");
        assert_eq!(entries[14], "z10");
    }

    #[test]
    fn hexdump_format() {
        let h = hexdump(b"AB\n");
        assert_eq!(h, format!("00000000  41 42 0a {}  |AB.|\n", "   ".repeat(13)));
    }
}
