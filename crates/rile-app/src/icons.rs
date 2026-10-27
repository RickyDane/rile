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
        LayoutDashboard, TextSearch, Brain, GripVertical, AppWindowMac,
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
    use gpui_kit::{AppContext, IntoElement, Styled, StyledImage};
    let view = cx.entity_id();
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
            cx.spawn(async move |_, cx| {
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

/// Glyph for AI commands in menus (SVG icons only; the toolbar button
/// uses the system's Apple Intelligence symbol, see [`apple_intelligence`]).
pub fn ai_icon() -> Icon {
    icon(IconName::Sparkles)
}

fn set_icon(path: &str) -> Option<&'static [u8]> {
    crate::icon_sets::ICONS.iter().find(|(p, _)| *p == path).map(|(_, bytes)| *bytes)
}

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
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

fn wants_native_icon(entry: &Entry) -> bool {
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
    match cfg!(target_os = "macos").then(|| native_icon_for(app, view, cx)).flatten() {
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
