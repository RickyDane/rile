//! Icon bundle: only the Lucide icons Rile uses are embedded, on top of the
//! default set gpui-component needs internally.

use std::borrow::Cow;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{AssetSource, Result, SharedString};
use rile_core::settings::IconSet;
use rile_core::places::PlaceIcon;
use rile_core::{Entry, EntryKind};

gpui_kit::assets::icon_assets!(
    AppIcons,
    [
        // Nearby Devices
        Laptop, Radar, MonitorSmartphone, Send, Share2,
        Folder, FolderOpen, FolderPlus, FolderSymlink, FolderUp, File, FileText, FileCode,
        FileArchive, FileImage, FileMusic, FileVideoCamera, FileSpreadsheet, FileSymlink, House,
        Star, HardDrive, Usb, ArrowLeft, ArrowRight, ArrowUp, ChevronRight, LayoutList, Monitor,
        Download, Image, Music, Clapperboard, Settings, Sun, Moon, SunMoon, Eye, EyeOff,
        RotateCw, Copy, Scissors, ClipboardPaste, Pencil, Trash, X, Search, Keyboard,
        SlidersHorizontal, Server, Globe, Plus, LayoutGrid, Columns2, Columns3, PanelRight, Sparkles, Eject, Info,
        ArrowDownWideNarrow, Atom, Calendar, ChevronLeft, Ellipsis, Heart, Maximize2, PanelLeft, Tag,
        CircleX, Puzzle, TriangleAlert,
        Activity, ArrowDownUp, Check,
        Archive, PackageOpen, AppWindow, ExternalLink, Images, Files, Link, FilePlus, WandSparkles,
        LayoutDashboard, TextSearch, Brain, GripVertical, AppWindowMac, Gauge, Play,
        DatabaseBackup, ArchiveRestore, Clock, FolderSync, CircleCheck, Pause, Square,
        SquareTerminal, ShieldCheck, Network, User, KeyRound, FileKey, Lock, IdCard, Cloud, Lightbulb,
    ]
);

/// The icon style in use (index into [`IconSet::ALL`]).
static ICON_SET: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_icon_set(set: IconSet) {
    let ix = IconSet::ALL.iter().position(|s| *s == set).unwrap_or(0);
    ICON_SET.store(ix as u8, std::sync::atomic::Ordering::Relaxed);
}

pub fn icon_set() -> IconSet {
    IconSet::ALL[ICON_SET.load(std::sync::atomic::Ordering::Relaxed) as usize % IconSet::ALL.len()]
}

/// An icon in the user's chosen style. Every icon in the UI goes through
/// here; glyphs a set doesn't cover fall back to the outline version.
pub fn icon(name: IconName) -> Icon {
    let path = gpui_kit::assets::IconNamed::path(name);
    let icon = Icon::new(name);
    match icon_set().dir() {
        Some(dir) => {
            let themed = path.replacen("icons/", &format!("icons/{dir}/"), 1);
            if set_icon(&themed).is_some() { icon.path(themed) } else { icon }
        }
        None => icon,
    }
}

/// The Apple Intelligence symbol, rendered once from the system's SF Symbol
/// with its gradient (None until loaded or where unavailable).
#[derive(Default)]
struct IntelligenceGlyph(Option<Option<std::sync::Arc<gpui_kit::Image>>>);
impl gpui_kit::Global for IntelligenceGlyph {}

/// The Apple Intelligence glyph `size` px tall for the current view; falls
/// back to a sparkle tinted `color` where the symbol isn't available.
pub fn apple_intelligence<V: 'static>(size: f32, color: gpui_kit::Hsla, cx: &mut gpui_kit::Context<V>) -> gpui_kit::AnyElement {
    let view = cx.entity_id();
    intelligence_glyph(size, color, view, cx)
}

/// [`apple_intelligence`] outside a view's context; `view` is re-rendered
/// once the symbol has loaded.
pub fn intelligence_glyph(size: f32, color: gpui_kit::Hsla, view: gpui_kit::EntityId, cx: &mut gpui_kit::App) -> gpui_kit::AnyElement {
    use gpui_kit::{AppContext, IntoElement, Styled, StyledImage};
    let glyph = cx.default_global::<IntelligenceGlyph>();
    match &glyph.0 {
        Some(Some(image)) => {
            return gpui_kit::img(image.clone()).h(gpui_kit::px(size)).w(gpui_kit::px(size)).object_fit(gpui_kit::ObjectFit::Contain).into_any_element();
        }
        Some(None) => {}
        None => {
            glyph.0 = Some(None);
            let task = cx.background_spawn(async {
                rile_services::app_icons::symbol_png("apple.intelligence", 96, rile_services::app_icons::INTELLIGENCE_GRADIENT)
                    .ok()
                    .map(|png| std::sync::Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png)))
            });
            cx.spawn(async move |cx| {
                let image = task.await;
                cx.update(|cx| {
                    if image.is_some() {
                        cx.default_global::<IntelligenceGlyph>().0 = Some(image);
                        cx.notify(view);
                    }
                });
            })
            .detach();
        }
    }
    icon(IconName::Sparkles).size(gpui_kit::px(size)).text_color(color).into_any_element()
}

/// A connection protocol's glyph: the provider's own full-color logo for
/// cloud drives, a Lucide icon tinted `color` for server protocols.
pub fn protocol_glyph(protocol: rile_remote::Protocol, size: f32, color: gpui_kit::Hsla) -> gpui_kit::AnyElement {
    use gpui_kit::{IntoElement, Styled, StyledImage};
    use rile_remote::Protocol;
    static LOGOS: std::sync::OnceLock<[std::sync::Arc<gpui_kit::Image>; 2]> = std::sync::OnceLock::new();
    let logos = LOGOS.get_or_init(|| {
        let svg = |bytes: &[u8]| std::sync::Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Svg, bytes.to_vec()));
        [svg(include_bytes!("../assets/logos/google-drive.svg")), svg(include_bytes!("../assets/logos/onedrive.svg"))]
    });
    let logo = |i: usize| gpui_kit::img(logos[i].clone()).size(gpui_kit::px(size)).object_fit(gpui_kit::ObjectFit::Contain).into_any_element();
    let name = match protocol {
        Protocol::GoogleDrive => return logo(0),
        Protocol::OneDrive => return logo(1),
        Protocol::Sftp => IconName::SquareTerminal,
        Protocol::Ftps => IconName::ShieldCheck,
        Protocol::Smb => IconName::Network,
        Protocol::Ftp => IconName::Server,
        Protocol::Peer => IconName::Laptop,
    };
    icon(name).size(gpui_kit::px(size)).text_color(color).into_any_element()
}

/// Asset path of the Apple Intelligence symbol as an SVG, see [`intelligence_svg`].
const INTELLIGENCE_SVG: &str = "icons/apple-intelligence.svg";

/// Glyph for AI commands in menus, which only take SVG icons: the Apple
/// Intelligence symbol as a single-color mask (the full-color version is
/// [`apple_intelligence`]).
pub fn ai_icon() -> Icon {
    menu_style(Icon::new(IconName::Sparkles).path(INTELLIGENCE_SVG))
}

/// An icon for a context-menu item. The menu draws icons at a fixed 12 px
/// with a 4 px gap, so scale the glyph up to 14 px and widen the gap to
/// leave about 8 px before the label.
pub fn menu_icon(name: IconName) -> Icon {
    menu_style(icon(name))
}

fn menu_style(icon: Icon) -> Icon {
    use gpui_kit::Styled;
    icon.transform(gpui_kit::Transformation::scale(gpui_kit::size(14. / 12., 14. / 12.))).mr(gpui_kit::px(5.))
}

/// The Apple Intelligence symbol wrapped in an SVG (the rendered symbol as
/// an embedded image), so SVG-only places can show it; the sparkle where
/// the symbol isn't available.
fn intelligence_svg() -> Result<Option<Cow<'static, [u8]>>> {
    static SVG: std::sync::OnceLock<Option<Vec<u8>>> = std::sync::OnceLock::new();
    let svg = SVG.get_or_init(|| {
        let png = rile_services::app_icons::symbol_png("apple.intelligence", 96, rile_services::app_icons::INTELLIGENCE_GRADIENT).ok()?;
        Some(
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><image x="1" y="1" width="22" height="22" href="data:image/png;base64,{}"/></svg>"#,
                base64(&png)
            )
            .into_bytes(),
        )
    });
    match svg {
        Some(bytes) => Ok(Some(Cow::Borrowed(bytes.as_slice()))),
        None => AppIcons.load(gpui_kit::assets::IconNamed::path(IconName::Sparkles).as_ref()),
    }
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

fn set_icon(path: &str) -> Option<&'static [u8]> {
    crate::icon_sets::ICONS.iter().find(|(p, _)| *p == path).map(|(_, bytes)| *bytes)
}

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == INTELLIGENCE_SVG {
            return intelligence_svg();
        }
        if let Some(bytes) = AppIcons.load(path)? {
            return Ok(Some(bytes));
        }
        if let Some(bytes) = set_icon(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(AppIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

pub fn icon_for_entry(entry: &Entry) -> IconName {
    if entry.is_dir {
        return if entry.kind == EntryKind::Symlink { IconName::FolderSymlink } else { IconName::Folder };
    }
    if entry.kind == EntryKind::Symlink {
        return IconName::FileSymlink;
    }
    match entry.extension().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "svg" | "bmp" | "tif" | "tiff" | "ico" | "avif" => {
            IconName::FileImage
        }
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" => IconName::FileVideoCamera,
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "aiff" => IconName::FileMusic,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "dmg" | "iso" | "zst" => IconName::FileArchive,
        "csv" | "xls" | "xlsx" | "numbers" | "ods" => IconName::FileSpreadsheet,
        "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "c" | "h" | "cpp" | "hpp" | "java" | "kt" | "swift"
        | "rb" | "php" | "sh" | "zsh" | "json" | "toml" | "yaml" | "yml" | "html" | "css" | "scss" | "lua"
        | "sql" | "xml" | "zig" => IconName::FileCode,
        "txt" | "md" | "pdf" | "doc" | "docx" | "rtf" | "pages" | "log" => IconName::FileText,
        _ => IconName::File,
    }
}

pub fn icon_for_place(icon: PlaceIcon) -> IconName {
    match icon {
        PlaceIcon::Home => IconName::House,
        PlaceIcon::Desktop => IconName::Monitor,
        PlaceIcon::Documents => IconName::FileText,
        PlaceIcon::Downloads => IconName::Download,
        PlaceIcon::Pictures => IconName::Image,
        PlaceIcon::Music => IconName::Music,
        PlaceIcon::Videos => IconName::Clapperboard,
        PlaceIcon::Folder => IconName::Folder,
    }
}

/// Icon for a path we only know by name (stats once to detect folders).
pub fn icon_for_path(path: &std::path::Path) -> IconName {
    match Entry::from_path(path.to_path_buf()) {
        Some(e) => icon_for_entry(&e),
        None => IconName::File,
    }
}

/// Native (.app) icons, loaded once in the background and cached.
#[derive(Default)]
struct NativeIcons {
    loaded: std::collections::HashMap<std::path::PathBuf, Option<std::sync::Arc<gpui_kit::Image>>>,
}
impl gpui_kit::Global for NativeIcons {}

/// Pixel size icons are rasterized at (covers grid view on retina).
const NATIVE_ICON_PX: u32 = 128;

/// App bundles are folders, so check the path's extension directly
/// (`Entry::extension` is empty for folders).
pub fn is_app_bundle(entry: &Entry) -> bool {
    entry.is_dir && entry.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))
}

/// Items whose look is their own icon: app bundles on macOS; programs,
/// shortcuts and icon files on Windows (Explorer shows them that way).
fn wants_native_icon(entry: &Entry) -> bool {
    if cfg!(windows) {
        return !entry.is_dir && matches!(&*entry.extension(), "exe" | "lnk" | "url" | "ico" | "msi" | "appref-ms");
    }
    cfg!(target_os = "macos") && is_app_bundle(entry)
}

/// The system icon for app bundles, or `None` (yet) — callers fall back to
/// the generic icon while it loads.
fn native_icon(entry: &Entry, view: gpui_kit::EntityId, cx: &mut gpui_kit::App) -> Option<std::sync::Arc<gpui_kit::Image>> {
    if !wants_native_icon(entry) {
        return None;
    }
    native_icon_for(&entry.path, view, cx)
}

/// The system icon of an application bundle at `path`, loaded in the
/// background and cached; `view` re-renders once it's ready.
fn native_icon_for(path: &std::path::Path, view: gpui_kit::EntityId, cx: &mut gpui_kit::App) -> Option<std::sync::Arc<gpui_kit::Image>> {
    use gpui_kit::AppContext;
    let cache = cx.default_global::<NativeIcons>();
    if let Some(hit) = cache.loaded.get(path) {
        return hit.clone();
    }
    cache.loaded.insert(path.to_path_buf(), None);
    let path = path.to_path_buf();
    let task = cx.background_spawn({
        let path = path.clone();
        async move {
            rile_services::app_icons::icon_png(&path, NATIVE_ICON_PX)
                .ok()
                .map(|png| std::sync::Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png)))
        }
    });
    cx.spawn(async move |cx| {
        let image = task.await;
        cx.update(|cx| {
            if image.is_some() {
                cx.default_global::<NativeIcons>().loaded.insert(path, image);
                // Views cache their last render; re-render the requester.
                cx.notify(view);
            }
        });
    })
    .detach();
    None
}

/// An application's own icon (e.g. in "Open With"), with a generic
/// placeholder while it loads.
pub fn app_icon<V: 'static>(app: &std::path::Path, size: f32, color: gpui_kit::Hsla, cx: &mut gpui_kit::Context<V>) -> gpui_kit::AnyElement {
    use gpui_kit::{IntoElement, Styled, StyledImage};
    let view = cx.entity_id();
    match cfg!(any(target_os = "macos", windows)).then(|| native_icon_for(app, view, cx)).flatten() {
        Some(image) => gpui_kit::img(image).size(gpui_kit::px(size)).object_fit(gpui_kit::ObjectFit::Contain).into_any_element(),
        None => icon(IconName::AppWindow).size(gpui_kit::px(size * 0.8)).text_color(color).into_any_element(),
    }
}

/// Icon element for an entry: the real app icon for bundles, else the
/// Lucide icon tinted like the rest of the UI.
pub fn entry_icon<V: 'static>(entry: &Entry, size: f32, color: gpui_kit::Hsla, cx: &mut gpui_kit::Context<V>) -> gpui_kit::AnyElement {
    use gpui_kit::{IntoElement, Styled, StyledImage};
    let view = cx.entity_id();
    match native_icon(entry, view, cx) {
        Some(image) => gpui_kit::img(image).size(gpui_kit::px(size)).object_fit(gpui_kit::ObjectFit::Contain).into_any_element(),
        None => icon(icon_for_entry(entry)).size(gpui_kit::px(size)).text_color(color).into_any_element(),
    }
}

/// Folders shared read-only with paired Nearby devices, published by
/// `theme::apply` whenever settings change so views can badge them.
#[derive(Default)]
pub struct SharedFolders(pub std::collections::HashSet<std::path::PathBuf>);
impl gpui_kit::Global for SharedFolders {}

pub fn is_shared(entry: &Entry, cx: &gpui_kit::App) -> bool {
    entry.is_dir && cx.try_global::<SharedFolders>().is_some_and(|s| s.0.contains(&entry.path))
}

/// Overlays the "shared" badge on a local folder's `icon` (`size` px box)
/// when the folder is shared with Nearby devices; returns it unchanged else.
pub fn with_shared_badge(icon: gpui_kit::AnyElement, entry: &Entry, size: f32, local: bool, cx: &gpui_kit::App) -> gpui_kit::AnyElement {
    use gpui_kit::{IntoElement, ParentElement, Styled, div, px};
    if !local || !is_shared(entry, cx) {
        return icon;
    }
    let ch = crate::theme::chrome(cx);
    let badge = (size * 0.5).max(10.);
    let offset = px(-(badge * 0.2));
    div()
        .relative()
        .flex_none()
        .child(icon)
        .child(
            div()
                .absolute()
                .right(offset)
                .bottom(offset)
                .size(px(badge))
                .rounded_full()
                .bg(ch.accent)
                .border_1()
                .border_color(ch.browser)
                .flex()
                .items_center()
                .justify_center()
                .child(self::icon(IconName::Share2).size(px(badge * 0.62)).text_color(gpui_kit::white())),
        )
        .into_any_element()
}
