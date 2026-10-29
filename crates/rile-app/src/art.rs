//! Full-color artwork: the approved Overview illustrations and the
//! file-type objects of the icon grid (dimensional folders, document pages
//! with a type glyph). Rendered through `img()` so gradients keep their
//! colors (`svg()` paints a tinted alpha mask).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyElement, AppContext, Context, FontWeight, Global, Hsla, Image, ImageFormat, ImageSource, IntoElement, ObjectFit,
    InteractiveElement, ParentElement, RenderImage, Styled, StyledImage, div, img, prelude::FluentBuilder, px, rgba,
};
use rile_core::Entry;
use rile_core::settings::SymbolStyle;

/// Overview illustrations (64 × 64 artboards, exported from the approved mockup).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Art {
    Storage,
    Files,
    Folder,
    Computer,
    ExternalDrive,
    Discovered,
    FreeSpace(FreeSpaceLevel),
    /// Cloud provider logos, shared with the connection navigation.
    GoogleDrive,
    OneDrive,
    /// Network share / NAS (SMB, SFTP, FTP).
    NetworkShare,
    /// Nearby Rile laptop or desktop.
    NearbyComputer,
    /// Nearby Rile server.
    NearbyServer,
    /// A desktop computer (monitor on a stand beside a tower).
    Desktop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeSpaceLevel {
    Unknown,
    Critical,
    Low,
    Moderate,
    Healthy,
}

impl FreeSpaceLevel {
    pub fn from_capacity(free: u64, total: u64) -> Self {
        if total == 0 {
            return Self::Unknown;
        }
        let percent = free as u128 * 100 / total as u128;
        match percent {
            0..=4 => Self::Critical,
            5..=9 => Self::Low,
            10..=24 => Self::Moderate,
            _ => Self::Healthy,
        }
    }
}

const fn style_index(style: SymbolStyle) -> u8 {
    let mut i = 0;
    while i < SymbolStyle::ALL.len() {
        if SymbolStyle::ALL[i] as u8 == style as u8 {
            return i as u8;
        }
        i += 1;
    }
    0
}

/// The symbol style in use (index into [`SymbolStyle::ALL`]); starts as the default.
static SYMBOL_STYLE: AtomicU8 = AtomicU8::new(style_index(SymbolStyle::DEFAULT));

pub fn set_symbol_style(style: SymbolStyle) {
    let ix = SymbolStyle::ALL.iter().position(|s| *s == style).unwrap_or(0);
    SYMBOL_STYLE.store(ix as u8, Ordering::Relaxed);
}

pub fn symbol_style() -> SymbolStyle {
    SymbolStyle::ALL[SYMBOL_STYLE.load(Ordering::Relaxed) as usize % SymbolStyle::ALL.len()]
}

/// The artwork files of one style, embedded at build time.
macro_rules! symbol_files {
    ($dir:literal) => {
        [
            include_str!(concat!("../assets/overview/", $dir, "storage.svg")),
            include_str!(concat!("../assets/overview/", $dir, "files.svg")),
            include_str!(concat!("../assets/overview/", $dir, "folder.svg")),
            include_str!(concat!("../assets/overview/", $dir, "computer.svg")),
            include_str!(concat!("../assets/overview/", $dir, "external-drive.svg")),
            include_str!(concat!("../assets/overview/", $dir, "discovered.svg")),
            include_str!(concat!("../assets/overview/", $dir, "free-space.svg")),
            include_str!(concat!("../assets/overview/", $dir, "network-share.svg")),
            include_str!(concat!("../assets/overview/", $dir, "nearby-computer.svg")),
            include_str!(concat!("../assets/overview/", $dir, "nearby-server.svg")),
            include_str!(concat!("../assets/overview/", $dir, "desktop.svg")),
        ]
    };
}

const FREE_SPACE: usize = 6;
const NETWORK_SHARE: usize = 7;

/// Source SVG of a symbol in a style (indices as in [`symbol_files!`]).
fn source(style: SymbolStyle, ix: usize) -> &'static str {
    static FILES: [[&str; 11]; 11] = [
        symbol_files!(""),
        symbol_files!("glass/"),
        symbol_files!("clay/"),
        symbol_files!("line/"),
        symbol_files!("isometric/"),
        symbol_files!("tilt-3d/"),
        symbol_files!("paper-cut/"),
        symbol_files!("pixel-art/"),
        symbol_files!("sticker/"),
        symbol_files!("midnight/"),
        symbol_files!("titanium/"),
    ];
    let style_ix = SymbolStyle::ALL.iter().position(|s| *s == style).unwrap_or(0);
    FILES[style_ix][ix]
}

/// Rasterized symbols, keyed by style, symbol and variant (gauge level or edge).
fn cached(style: SymbolStyle, ix: usize, variant: u32, make: impl FnOnce() -> Arc<Image>) -> Arc<Image> {
    static CACHE: OnceLock<Mutex<HashMap<(SymbolStyle, usize, u32), Arc<Image>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default).lock().unwrap().entry((style, ix, variant)).or_insert_with(make).clone()
}

fn free_space_image(style: SymbolStyle, level: FreeSpaceLevel) -> Arc<Image> {
    let (ix, color, fill) = match level {
        FreeSpaceLevel::Unknown => (0, "#8194a9", 0),
        FreeSpaceLevel::Critical => (1, "#ef8585", 8),
        FreeSpaceLevel::Low => (2, "#eba565", 22),
        FreeSpaceLevel::Moderate => (3, "#e3c66a", 50),
        FreeSpaceLevel::Healthy => (4, "#65cba3", 90),
    };
    // The arc starts at 135° and sweeps 270°; the unrotated needle is at -45°.
    // Every style keeps these hooks (scripts/symbol-suites/generate.py checks them).
    let rotation = if fill == 0 { -45. } else { fill as f32 * 2.7 - 180. };
    cached(style, FREE_SPACE, ix, || {
        let svg = source(style, FREE_SPACE)
            .replace("#5fd39a", color)
            .replace("#85c6fa", color)
            .replace("#2e80cc", color)
            .replace("M23.5 40.5a12 12 0 0 1 17-17", "M23.5 40.5a12 12 0 1 1 17 0")
            .replace("stroke=\"url(#rile-free-arc)\"", &format!("stroke=\"url(#rile-free-arc)\" stroke-opacity=\"{}\" stroke-dasharray=\"{} 100\"", if fill == 0 { 0 } else { 1 }, fill as f32 * 0.5655))
            .replace("d=\"M32 32 38.5 25.5\"", &format!("d=\"M32 32 38.5 25.5\" transform=\"rotate({rotation} 32 32)\""));
        hires_svg(&svg, 256, 256)
    })
}

/// SVGs rasterize at their intrinsic size, so declare a large one to stay
/// crisp on Retina at any display size.
fn hires_svg(svg: &str, w: u32, h: u32) -> Arc<Image> {
    let svg = svg.replacen("<svg ", &format!("<svg width=\"{w}\" height=\"{h}\" "), 1);
    Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
}

/// The symbol for this machine: a desktop when the startup check found one,
/// otherwise the laptop (also while the check is still running).
pub fn this_computer() -> Art {
    this_computer_for(crate::workspace::nearby::cached_facts().map(|f| f.kind))
}

pub(crate) fn this_computer_for(kind: Option<rile_remote::peer::DeviceKind>) -> Art {
    match kind {
        Some(rile_remote::peer::DeviceKind::Desktop) => Art::Desktop,
        _ => Art::Computer,
    }
}

/// A symbol in the user's chosen style.
pub fn image(art: Art) -> Arc<Image> {
    image_in(symbol_style(), art)
}

/// A symbol in a given style (used by the style picker's previews).
pub fn image_in(style: SymbolStyle, art: Art) -> Arc<Image> {
    let protocol = match art {
        Art::GoogleDrive => Some(rile_remote::Protocol::GoogleDrive),
        Art::OneDrive => Some(rile_remote::Protocol::OneDrive),
        _ => None,
    };
    if let Some(protocol) = protocol {
        return crate::icons::protocol_logo(protocol).expect("cloud protocols have logos");
    }
    let ix = match art {
        Art::Storage => 0,
        Art::Files => 1,
        Art::Folder => 2,
        Art::Computer => 3,
        Art::ExternalDrive => 4,
        Art::Discovered => 5,
        Art::FreeSpace(level) => return free_space_image(style, level),
        Art::GoogleDrive | Art::OneDrive => unreachable!("provider logos handled above"),
        Art::NetworkShare => NETWORK_SHARE,
        Art::NearbyComputer => 8,
        Art::NearbyServer => 9,
        Art::Desktop => 10,
    };
    cached(style, ix, 0, || hires_svg(source(style, ix), 256, 256))
}

pub(crate) fn image_for_size(art: Art, size: f32) -> Arc<Image> {
    if art != Art::NetworkShare {
        return image(art);
    }
    // GPUI already rasterizes SVGs at 2x. Shrinking a 512px texture to a
    // 46/56px icon uses GPU bilinear sampling, not an area filter: thin vents
    // can disappear and curved corners become square depending on alignment.
    // Rasterize the centered rack at its display size instead of changing it.
    let style = symbol_style();
    let edge = size.ceil().max(1.) as u32;
    cached(style, NETWORK_SHARE, edge, || hires_svg(source(style, NETWORK_SHARE), edge, edge))
}

pub fn art(art: Art, size: f32) -> AnyElement {
    if matches!(art, Art::GoogleDrive | Art::OneDrive) {
        // Provider logos fill their viewBox; the device illustrations have
        // built-in margins. Keep the same layout slot but match their weight.
        return div()
            .size(px(size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(img(image(art)).size(px(size * 0.72)).object_fit(ObjectFit::Contain))
            .into_any_element();
    }
    img(image_for_size(art, size)).size(px(size)).flex_none().object_fit(ObjectFit::Contain).into_any_element()
}

/// A document page with a folded corner (54 × 70 artboard).
fn page(dark: bool) -> Arc<Image> {
    static PAGES: [OnceLock<Arc<Image>>; 2] = [const { OnceLock::new() }; 2];
    PAGES[dark as usize]
        .get_or_init(|| {
            let (top, bottom, fold_a, fold_b) = if dark {
                ("#3a3f4b", "#2b303b", "#262b36", "#454d5f")
            } else {
                ("#ffffff", "#eef1f6", "#dfe4ec", "#ffffff")
            };
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 54 70"><defs><linearGradient id="p" x1="0" y1="0" x2="1" y2="1"><stop stop-color="{top}"/><stop offset="1" stop-color="{bottom}"/></linearGradient><linearGradient id="f" x1="0" y1="1" x2="1" y2="0"><stop stop-color="{fold_a}"/><stop offset="1" stop-color="{fold_b}"/></linearGradient></defs><path d="M6 .5h30.5L53.5 16v48a5.5 5.5 0 0 1-5.5 5.5H6A5.5 5.5 0 0 1 .5 64V6A5.5 5.5 0 0 1 6 .5Z" fill="url(#p)"/><path d="M36.5 .5V12a4 4 0 0 0 4 4h13Z" fill="url(#f)"/></svg>"##
            );
            hires_svg(&svg, 216, 280)
        })
        .clone()
}

enum Glyph {
    Text(&'static str),
    Owned(String),
    Icon(IconName),
    Badge(&'static str),
}

/// Type color and glyph for a file, matching the mockup's document objects.
fn style(entry: &Entry) -> (u32, Glyph) {
    let ext = entry.extension();
    match ext.as_str() {
        "rs" => (0xdf985cff, Glyph::Text("R")),
        "toml" => (0xd7a563ff, Glyph::Text("[T]")),
        "md" | "markdown" | "mdx" => (0x96a5bdff, Glyph::Text("M↓")),
        "tsx" | "jsx" => (0x49b9e4ff, Glyph::Icon(IconName::Atom)),
        "css" | "scss" | "sass" | "less" => (0x4aabedff, Glyph::Text("3")),
        "pdf" => (0xda877cff, Glyph::Badge("PDF")),
        "json" | "jsonc" => (0xd0b770ff, Glyph::Text("{ }")),
        "yaml" | "yml" | "ini" | "cfg" | "conf" => (0xb7a6d9ff, Glyph::Text("⋮")),
        "js" | "mjs" | "cjs" => (0xe3c66aff, Glyph::Text("JS")),
        "ts" | "mts" => (0x5aa2e6ff, Glyph::Text("TS")),
        "py" => (0x6aa5d8ff, Glyph::Text("Py")),
        "go" => (0x5ec4d8ff, Glyph::Text("Go")),
        "swift" => (0xec8a5cff, Glyph::Text("Sw")),
        "c" | "h" | "cpp" | "hpp" | "cc" => (0x7f9fd8ff, Glyph::Text("C")),
        "html" | "htm" | "xml" => (0xe0855cff, Glyph::Text("</>")),
        "sh" | "zsh" | "bash" | "fish" => (0x8fc27aff, Glyph::Text("$_")),
        "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "bmp" | "tif" | "tiff" | "ico" | "avif" => {
            (0xbd96e0ff, Glyph::Icon(IconName::Image))
        }
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" => (0xe07a8aff, Glyph::Icon(IconName::Clapperboard)),
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "aiff" => (0xd68bc0ff, Glyph::Icon(IconName::Music)),
        "csv" | "xls" | "xlsx" | "numbers" | "ods" => (0x6cc18eff, Glyph::Icon(IconName::FileSpreadsheet)),
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" => (0xc5a16dff, Glyph::Text("ZIP")),
        "dmg" | "iso" | "pkg" => (0xa9b4c6ff, Glyph::Icon(IconName::HardDrive)),
        "txt" | "log" | "rtf" | "doc" | "docx" | "pages" => (0x8495aeff, Glyph::Text("≡")),
        "" => (0x8495aeff, Glyph::Text("≡")),
        other if other.len() <= 4 => (0x8495aeff, Glyph::Owned(other.to_uppercase())),
        _ => (0x8495aeff, Glyph::Text("≡")),
    }
}

/// Image extensions the grid shows as real thumbnails.
pub const THUMB_EXTS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "tif", "tiff", "heic", "heif", "avif", "dng", "cr2", "nef", "arw"];
/// Skip thumbnails for huge files to keep scrolling smooth.
const THUMB_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Never decode images larger than this many pixels, even transiently.
const THUMB_MAX_PIXELS: usize = 100_000_000;
/// Cached thumbnails before the cache starts over.
const THUMB_CACHE_LIMIT: usize = 1500;

pub fn wants_thumbnail(entry: &Entry, thumbnails: bool) -> bool {
    thumbnails && !entry.is_dir && THUMB_EXTS.contains(&entry.extension().as_str()) && entry.size < THUMB_MAX_BYTES
}

type ThumbKey = (PathBuf, Option<SystemTime>, u64, u32);

enum Thumb {
    Pending,
    Ready(Arc<RenderImage>),
    Failed,
}

/// Downscaled thumbnails, decoded on a background thread and cached by
/// path, modification time, size and edge length. Handing full image paths
/// to `img()` would keep every full-resolution bitmap (and every GIF frame)
/// in memory.
#[derive(Default)]
struct Thumbnails {
    cache: HashMap<ThumbKey, Thumb>,
}
impl Global for Thumbnails {}

/// Thumbnail quality in percent (10–100), from `Settings::thumbnail_quality`.
static THUMB_QUALITY: AtomicU8 = AtomicU8::new(100);

pub fn set_thumbnail_quality(percent: u8) {
    THUMB_QUALITY.store(percent.clamp(10, 100), Ordering::Relaxed);
}

/// Edge length (device pixels) for a thumbnail shown at `size` points: full
/// Retina detail at 100%, scaled down by the quality setting. The edge is
/// part of the cache key, so changing the quality decodes anew.
fn thumb_edge(size: f32) -> u32 {
    scaled_edge(full_edge(size), THUMB_QUALITY.load(Ordering::Relaxed))
}

fn full_edge(size: f32) -> u32 {
    ((size * 2.).ceil() as u32).next_power_of_two().clamp(128, 1024)
}

pub(crate) fn scaled_edge(full: u32, percent: u8) -> u32 {
    (full * u32::from(percent.clamp(10, 100))).div_ceil(100).max(16)
}

pub(crate) fn decode_thumbnail(path: &Path, edge: u32) -> Option<Arc<RenderImage>> {
    if let Ok(dims) = imagesize::size(path)
        && dims.width.saturating_mul(dims.height) > THUMB_MAX_PIXELS
    {
        return None;
    }
    // `decode` reads only the first frame of animated images. Formats the
    // image crate can't read (HEIC, RAW…) go through the system decoder.
    let image = match image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode() {
        Ok(image) => image,
        Err(_) => {
            let png = rile_services::app_icons::image_thumbnail_png(path, edge).ok()?;
            image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?
        }
    };
    let mut rgba = image.thumbnail(edge, edge).into_rgba8();
    // gpui stores BGRA.
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(rgba)])))
}

/// The bounded thumbnail for a local image, at the user's thumbnail quality,
/// or None while it loads (or if it can't be decoded); the requesting view
/// re-renders once it's ready.
pub fn thumbnail<V: 'static>(path: &Path, modified: Option<SystemTime>, bytes: u64, size: f32, cx: &mut Context<V>) -> Option<Arc<RenderImage>> {
    downscaled(path, modified, bytes, thumb_edge(size), cx)
}

/// Like [`thumbnail`] but always at full detail, for large previews (the
/// inspector stage, the gallery viewer) that the quality setting must not blur.
pub fn preview_image<V: 'static>(path: &Path, modified: Option<SystemTime>, bytes: u64, size: f32, cx: &mut Context<V>) -> Option<Arc<RenderImage>> {
    downscaled(path, modified, bytes, full_edge(size), cx)
}

fn downscaled<V: 'static>(path: &Path, modified: Option<SystemTime>, bytes: u64, edge: u32, cx: &mut Context<V>) -> Option<Arc<RenderImage>> {
    let key: ThumbKey = (path.to_path_buf(), modified, bytes, edge);
    let view = cx.entity_id();
    let cache = cx.default_global::<Thumbnails>();
    match cache.cache.get(&key) {
        Some(Thumb::Ready(image)) => return Some(image.clone()),
        Some(Thumb::Pending | Thumb::Failed) => return None,
        None => {}
    }
    if cache.cache.len() >= THUMB_CACHE_LIMIT {
        cache.cache.retain(|_, t| matches!(t, Thumb::Pending));
    }
    cache.cache.insert(key.clone(), Thumb::Pending);
    let task = cx.background_spawn({
        let (path, edge) = (key.0.clone(), key.3);
        async move { decode_thumbnail(&path, edge) }
    });
    cx.spawn(async move |_, cx| {
        let image = task.await;
        cx.update(|cx| {
            let slot = match image {
                Some(image) => Thumb::Ready(image),
                None => Thumb::Failed,
            };
            cx.default_global::<Thumbnails>().cache.insert(key, slot);
            cx.notify(view);
        });
    })
    .detach();
    None
}

/// The file object shown in the icon grid and inspector, fitted into a
/// `size` × `size` box: folder artwork, an app's own icon, a framed image
/// thumbnail, or a document page with the file type's glyph.
pub fn file_object<V: 'static>(entry: &Entry, size: f32, thumbnails: bool, local: bool, cx: &mut Context<V>) -> AnyElement {
    let dark = crate::theme::chrome(cx).dark;
    if crate::icons::is_app_bundle(entry) {
        return crate::icons::entry_icon(entry, size * 0.9, rgba(0x58adffff).into(), cx);
    }
    if entry.is_dir {
        return art(Art::Folder, size);
    }
    if local && wants_thumbnail(entry, thumbnails) {
        // Vector images rasterize at their own size; bitmaps are downscaled.
        let source: Option<ImageSource> = if entry.extension() == "svg" {
            Some(entry.path.clone().into())
        } else {
            thumbnail(&entry.path, entry.modified, entry.size, size, cx).map(Into::into)
        };
        if let Some(source) = source {
            let frame: Hsla = if dark { rgba(0x3a3f4bff).into() } else { rgba(0xffffffff).into() };
            return framed_thumbnail(source, size, frame).into_any_element();
        }
    }
    let (color, glyph) = style(entry);
    let color: Hsla = rgba(color).into();
    let h = size * 0.86;
    let w = h * 54. / 70.;
    let font = (h * 0.22).max(8.);
    let glyph: AnyElement = match glyph {
        Glyph::Text(t) => mono(t.to_string(), font, color),
        Glyph::Owned(t) => { let scale = if t.len() > 3 { 0.7 } else { 0.85 }; mono(t, font * scale, color) }
        Glyph::Icon(icon) => crate::icons::icon(icon).size(px(h * 0.36)).text_color(color).into_any_element(),
        Glyph::Badge(t) => div()
            .px(px(font * 0.25))
            .py(px(font * 0.08))
            .rounded(px(3.))
            .bg(rgba(0xba605cff))
            .text_color(rgba(0xffffffff))
            .text_size(px(font * 0.55))
            .font_weight(FontWeight::BOLD)
            .child(t)
            .into_any_element(),
    };
    div()
        .relative()
        .flex_none()
        .w(px(w))
        .h(px(h))
        .child(img(page(dark)).absolute().inset_0().size_full())
        .child(
            div()
                .absolute()
                .inset_0()
                .pt(px(h * 0.12))
                .flex()
                .items_center()
                .justify_center()
                .child(glyph),
        )
        .into_any_element()
}

pub(crate) fn framed_thumbnail(source: ImageSource, size: f32, frame: Hsla) -> gpui_kit::Div {
    let padding = (size * 0.03).max(2.);
    let width = size - padding * 2.;
    let height = size * 0.8 - padding * 2.;
    div()
        .w(px(size))
        .h(px(size * 0.8))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .p(px(padding))
        .rounded(px(5.))
        .bg(frame)
        .shadow_md()
        .child(
            img(source)
                .w(px(width))
                .h(px(height))
                .min_w(px(0.))
                .min_h(px(0.))
                .flex_none()
                // GPUI otherwise supplies the source's intrinsic aspect ratio
                // to layout, which can enlarge the box beyond its set height.
                // Only the viewport uses this ratio; Contain fits the actual image.
                .aspect_ratio(width / height)
                .object_fit(ObjectFit::Contain)
                .debug_selector(|| "thumbnail-image".into())
                .rounded(px(3.)),
        )
}

fn mono(text: String, size: f32, color: Hsla) -> AnyElement {
    div()
        .font_family("Menlo")
        .text_size(px(size))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .whitespace_nowrap()
        .when(text.chars().count() > 2, |d| d.text_size(px(size * 0.78)))
        .child(text)
        .into_any_element()
}
