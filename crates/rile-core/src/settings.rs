use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{SortKey, SortOrder};

/// Reasoning allowance for local text models. Apple Intelligence is unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiThinking {
    None,
    #[default]
    Low,
    Medium,
    High,
    Unlimited,
}

impl AiThinking {
    pub fn token_budget(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Low => 64,
            Self::Medium => 128,
            Self::High => 256,
            Self::Unlimited => -1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// Accent color presets of the design system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    #[default]
    Blue,
    Purple,
    Pink,
    Red,
    Orange,
    Green,
    Teal,
    Graphite,
}

impl Accent {
    pub const ALL: [Accent; 8] = [
        Accent::Blue,
        Accent::Purple,
        Accent::Pink,
        Accent::Red,
        Accent::Orange,
        Accent::Green,
        Accent::Teal,
        Accent::Graphite,
    ];

    /// Hue (degrees) and saturation (0..1) of the accent.
    pub fn hue_sat(self) -> (f32, f32) {
        match self {
            Accent::Blue => (212., 0.92),
            Accent::Purple => (268., 0.75),
            Accent::Pink => (330., 0.80),
            Accent::Red => (0., 0.80),
            Accent::Orange => (28., 0.95),
            Accent::Green => (142., 0.65),
            Accent::Teal => (178., 0.70),
            Accent::Graphite => (220., 0.08),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Accent::Blue => "Blue",
            Accent::Purple => "Purple",
            Accent::Pink => "Pink",
            Accent::Red => "Red",
            Accent::Orange => "Orange",
            Accent::Green => "Green",
            Accent::Teal => "Teal",
            Accent::Graphite => "Graphite",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Compact,
    #[default]
    Comfortable,
    Spacious,
}

impl Density {
    pub fn row_height(self) -> f32 {
        match self {
            Density::Compact => 22.,
            Density::Comfortable => 26.,
            Density::Spacious => 32.,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewKind {
    #[default]
    List,
    Grid,
    Columns,
}

/// Sections of the sidebar, which the user can reorder by dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarSection {
    Library,
    Favorites,
    Locations,
    Devices,
    Network,
    /// Other Rile devices on the local network.
    Nearby,
}

impl SidebarSection {
    pub const ALL: [Self; 6] = [Self::Library, Self::Favorites, Self::Locations, Self::Devices, Self::Network, Self::Nearby];

    pub fn label(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Favorites => "Favorites",
            Self::Locations => "Locations",
            Self::Devices => "Devices",
            Self::Network => "Network",
            Self::Nearby => "Nearby",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartLocation {
    #[default]
    Home,
    LastVisited,
    /// [`Settings::start_dir`]
    Custom,
}

/// Icon styles the UI can be drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconSet {
    /// Lucide outlines (the original look).
    #[default]
    Outline,
    /// Solid Phosphor glyphs.
    Filled,
    /// Phosphor two-tone: outline plus a translucent fill.
    Duotone,
    /// Thin Phosphor strokes.
    Light,
}

impl IconSet {
    pub const ALL: [IconSet; 4] = [IconSet::Outline, IconSet::Filled, IconSet::Duotone, IconSet::Light];

    pub fn label(self) -> &'static str {
        match self {
            IconSet::Outline => "Outline",
            IconSet::Filled => "Filled",
            IconSet::Duotone => "Duotone",
            IconSet::Light => "Light",
        }
    }

    /// Asset folder of the set (None: the built-in Lucide icons).
    pub fn dir(self) -> Option<&'static str> {
        match self {
            IconSet::Outline => None,
            IconSet::Filled => Some("fill"),
            IconSet::Duotone => Some("duotone"),
            IconSet::Light => Some("light"),
        }
    }
}

/// Artwork of the Dock icon (tinted by the accent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppIcon {
    #[default]
    FolioBolt,
    DrawerBolt,
    FolioFlame,
    FolioEmber,
    FolioHotRod,
    DrawerFlame,
    DrawerEmber,
    DrawerHotRod,
}

impl AppIcon {
    pub const ALL: [AppIcon; 8] = [
        AppIcon::FolioBolt,
        AppIcon::DrawerBolt,
        AppIcon::FolioFlame,
        AppIcon::FolioEmber,
        AppIcon::FolioHotRod,
        AppIcon::DrawerFlame,
        AppIcon::DrawerEmber,
        AppIcon::DrawerHotRod,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AppIcon::FolioBolt => "Folio Bolt",
            AppIcon::DrawerBolt => "Drawer Bolt",
            AppIcon::FolioFlame => "Folio Flame",
            AppIcon::FolioEmber => "Folio Ember",
            AppIcon::FolioHotRod => "Folio Hot Rod",
            AppIcon::DrawerFlame => "Drawer Flame",
            AppIcon::DrawerEmber => "Drawer Ember",
            AppIcon::DrawerHotRod => "Drawer Hot Rod",
        }
    }
}

/// File types a default program can be chosen for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileCategory {
    Code,
    Text,
    Images,
    Videos,
    Audio,
    Documents,
    Spreadsheets,
    Archives,
}

impl FileCategory {
    pub const ALL: [FileCategory; 8] = [
        FileCategory::Code,
        FileCategory::Text,
        FileCategory::Images,
        FileCategory::Videos,
        FileCategory::Audio,
        FileCategory::Documents,
        FileCategory::Spreadsheets,
        FileCategory::Archives,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FileCategory::Code => "Code",
            FileCategory::Text => "Text",
            FileCategory::Images => "Images",
            FileCategory::Videos => "Videos",
            FileCategory::Audio => "Audio",
            FileCategory::Documents => "Documents",
            FileCategory::Spreadsheets => "Spreadsheets",
            FileCategory::Archives => "Archives",
        }
    }

    /// Lowercase extensions (without dot) belonging to the category.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            FileCategory::Code => &[
                "rs", "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "go", "c", "h", "cc", "cpp", "hpp", "java", "kt", "swift",
                "rb", "php", "sh", "zsh", "bash", "fish", "json", "toml", "yaml", "yml", "html", "css", "scss", "lua", "sql",
                "xml", "zig", "cs", "dart", "vue", "svelte", "ini", "conf", "gradle", "make", "cmake", "dockerfile",
            ],
            FileCategory::Text => &["txt", "md", "markdown", "log", "rtf", "csv", "tsv"],
            FileCategory::Images => &["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "svg", "bmp", "tif", "tiff", "ico", "avif", "raw", "psd"],
            FileCategory::Videos => &["mp4", "mov", "mkv", "avi", "webm", "m4v", "wmv", "flv"],
            FileCategory::Audio => &["mp3", "wav", "flac", "aac", "m4a", "ogg", "aiff", "opus"],
            FileCategory::Documents => &["pdf", "doc", "docx", "pages", "odt", "epub", "ppt", "pptx", "key"],
            FileCategory::Spreadsheets => &["xls", "xlsx", "numbers", "ods"],
            FileCategory::Archives => &["zip", "7z", "tar", "gz", "tgz", "br", "zst", "tzst", "bz2", "xz", "rar"],
        }
    }

    /// The category of a file by its extension (case-insensitive).
    pub fn of_extension(ext: &str) -> Option<FileCategory> {
        let ext = ext.to_ascii_lowercase();
        // Text formats that are also data (csv) count as text; code wins over text.
        Self::ALL.into_iter().find(|c| c.extensions().contains(&ext.as_str()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub show_hidden: bool,
    pub theme: ThemeMode,
    pub accent: Accent,
    pub density: Density,
    pub sort_key: SortKey,
    pub sort_order: SortOrder,
    pub confirm_trash: bool,
    pub favorites: Vec<PathBuf>,
    pub default_view: ViewKind,
    pub grid_icon_size: f32,
    pub show_thumbnails: bool,
    pub dual_pane: bool,
    pub show_preview_panel: bool,
    pub start_location: StartLocation,
    /// Folder opened on launch with [`StartLocation::Custom`].
    pub start_dir: Option<PathBuf>,
    pub last_location: Option<PathBuf>,
    /// Icon style of the whole UI.
    pub icon_set: IconSet,
    /// Dock icon artwork.
    pub app_icon: AppIcon,
    /// Translucent, blurred sidebar and header (popups remain opaque).
    pub glass: bool,
    /// Glass tint hue in degrees; zero uses the theme's neutral frost.
    pub glass_hue: u16,
    /// Percentage of the background visible through the glass (0–100).
    pub glass_translucency: u8,
    /// Shadows indicating sidebar items hidden above or below the viewport.
    pub sidebar_scroll_shadows: bool,
    /// Sidebar width chosen by dragging its edge (None: automatic).
    pub sidebar_width: Option<f32>,
    // Search
    pub search_respect_gitignore: bool,
    pub search_max_results: usize,
    pub search_use_spotlight: bool,
    /// Remember the contents of opened folders for instant search.
    pub index_discovered: bool,
    // AI
    pub ai_enabled: bool,
    /// None selects Apple Intelligence; otherwise a local GGUF file or MLX model folder.
    pub ai_model_path: Option<PathBuf>,
    /// llama.cpp runtime executable (a path or a program on PATH).
    pub ai_llama_server: String,
    /// MLX runtime executable, `mlx_lm.server` (a path or a program on PATH).
    pub ai_mlx_server: String,
    pub ai_thinking: AiThinking,
    /// Describe images and documents on device so AI search and the photo
    /// library can find them by content.
    pub ai_smart_tags: bool,
    // Updates
    pub update_check_on_start: bool,
    pub update_repo: String,
    pub update_prereleases: bool,
    // Plugins
    pub plugins_enabled: bool,
    pub disabled_plugins: Vec<String>,
    /// Built-in FTP/SFTP provider (Network section in the sidebar).
    pub remote_enabled: bool,
    /// Order of the sidebar sections (see [`Settings::sidebar_order`]).
    pub sidebar_sections: Vec<SidebarSection>,
    /// Sidebar sections the user collapsed to just their header.
    pub collapsed_sidebar_sections: Vec<SidebarSection>,
    /// Default program per file type (application bundle paths). Keys are
    /// a category, or a single extension as ".ext" which takes precedence.
    pub default_apps: std::collections::BTreeMap<String, PathBuf>,
    /// Nearby Devices: other Rile installations on the local network.
    pub nearby: NearbySettings,
}

/// A folder shared read-only with paired devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedFolder {
    /// Name other devices see (unique among the shares).
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NearbySettings {
    /// Find and be found by other Rile devices on the local network.
    pub enabled: bool,
    /// Name other devices see; empty uses the computer name.
    pub device_name: String,
    /// Where accepted files go; None uses the Downloads folder.
    pub inbox: Option<PathBuf>,
    pub shares: Vec<SharedFolder>,
}

impl Default for NearbySettings {
    fn default() -> Self {
        Self { enabled: true, device_name: String::new(), inbox: None, shares: Vec::new() }
    }
}

impl NearbySettings {
    pub fn inbox_dir(&self) -> PathBuf {
        self.inbox
            .clone()
            .or_else(dirs::download_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_else(std::env::temp_dir)
    }

    /// A share name for `path` that isn't taken yet.
    pub fn share_name_for(&self, path: &std::path::Path) -> String {
        let base = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Shared".into());
        let mut name = base.clone();
        let mut n = 2;
        while self.shares.iter().any(|s| s.name == name) {
            name = format!("{base} {n}");
            n += 1;
        }
        name
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_hidden: false,
            theme: ThemeMode::System,
            sort_key: SortKey::Name,
            sort_order: SortOrder::Ascending,
            confirm_trash: false,
            accent: Accent::Blue,
            density: Density::Comfortable,
            default_view: ViewKind::List,
            grid_icon_size: 64.,
            show_thumbnails: true,
            dual_pane: false,
            show_preview_panel: true,
            start_location: StartLocation::Home,
            start_dir: None,
            last_location: None,
            icon_set: IconSet::Outline,
            app_icon: AppIcon::FolioBolt,
            glass: false,
            glass_hue: 0,
            glass_translucency: 50,
            sidebar_scroll_shadows: true,
            sidebar_width: None,
            search_respect_gitignore: false,
            search_max_results: 5000,
            search_use_spotlight: false,
            index_discovered: true,
            ai_enabled: true,
            ai_model_path: None,
            ai_llama_server: "llama-server".into(),
            ai_mlx_server: "mlx_lm.server".into(),
            ai_thinking: AiThinking::default(),
            ai_smart_tags: true,
            update_check_on_start: true,
            update_repo: "RickyDane/rile".into(),
            update_prereleases: false,
            plugins_enabled: true,
            disabled_plugins: Vec::new(),
            remote_enabled: true,
            sidebar_sections: SidebarSection::ALL.to_vec(),
            nearby: NearbySettings::default(),
            collapsed_sidebar_sections: Vec::new(),
            default_apps: Default::default(),
            favorites: dirs::home_dir()
                .into_iter()
                .chain(dirs::desktop_dir())
                .chain(dirs::download_dir())
                .collect(),
        }
    }
}

impl Settings {
    /// The sidebar sections in display order: the saved order without
    /// duplicates, followed by any section it doesn't mention.
    pub fn sidebar_order(&self) -> Vec<SidebarSection> {
        let mut order: Vec<SidebarSection> = Vec::new();
        for s in self.sidebar_sections.iter().chain(SidebarSection::ALL.iter()) {
            if !order.contains(s) {
                order.push(*s);
            }
        }
        order
    }

    /// Moves `section` to where `target` is: after it when dragged
    /// downwards, before it when dragged upwards.
    pub fn move_sidebar_section(&mut self, section: SidebarSection, target: SidebarSection) {
        let mut order = self.sidebar_order();
        let (Some(from), Some(to)) = (order.iter().position(|s| *s == section), order.iter().position(|s| *s == target)) else {
            return;
        };
        let moved = order.remove(from);
        order.insert(to, moved);
        self.sidebar_sections = order;
    }

    pub fn sidebar_section_collapsed(&self, section: SidebarSection) -> bool {
        self.collapsed_sidebar_sections.contains(&section)
    }

    /// Collapses `section` if it is expanded, expands it otherwise.
    pub fn toggle_sidebar_section(&mut self, section: SidebarSection) {
        if let Some(ix) = self.collapsed_sidebar_sections.iter().position(|s| *s == section) {
            self.collapsed_sidebar_sections.remove(ix);
        } else {
            self.collapsed_sidebar_sections.push(section);
        }
    }

    /// `$RILE_CONFIG_DIR/settings.toml` if set, else the platform config dir.
    pub fn config_dir() -> Option<PathBuf> {
        match std::env::var_os("RILE_CONFIG_DIR") {
            Some(d) => Some(PathBuf::from(d)),
            None => Some(dirs::config_dir()?.join("rile")),
        }
    }

    /// Where large, machine-specific data lives (downloaded models and AI
    /// runtimes). Same as [`Self::config_dir`] except on Windows, where it is
    /// `%LOCALAPPDATA%\rile` so gigabytes don't roam with the profile.
    pub fn data_dir() -> Option<PathBuf> {
        match std::env::var_os("RILE_CONFIG_DIR") {
            Some(d) => Some(PathBuf::from(d)),
            None if cfg!(windows) => Some(dirs::data_local_dir()?.join("rile")),
            None => Self::config_dir(),
        }
    }

    pub fn path() -> Option<PathBuf> {
        let dir = match std::env::var_os("RILE_CONFIG_DIR") {
            Some(d) => PathBuf::from(d),
            None => dirs::config_dir()?.join("rile"),
        };
        Some(dir.join("settings.toml"))
    }

    /// Loads settings, falling back to defaults when missing or invalid.
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = Self::path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Settings key of a category in [`Settings::default_apps`].
    pub fn category_key(category: FileCategory) -> String {
        format!("{category:?}").to_lowercase()
    }

    /// The default program chosen for `path`: one set for its extension,
    /// else for its category. None means "use the system default".
    pub fn default_app_for(&self, path: &std::path::Path) -> Option<&PathBuf> {
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        self.default_apps
            .get(&format!(".{ext}"))
            .or_else(|| FileCategory::of_extension(&ext).and_then(|c| self.default_apps.get(&Self::category_key(c))))
            .filter(|app| app.exists())
    }

    pub fn toggle_favorite(&mut self, path: PathBuf) {
        if let Some(i) = self.favorites.iter().position(|p| *p == path) {
            self.favorites.remove(i);
        } else {
            self.favorites.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_order_is_complete_and_movable() {
        let mut s = Settings { sidebar_sections: vec![SidebarSection::Devices, SidebarSection::Devices], ..Default::default() };
        // Duplicates dropped, missing sections appended in their default order.
        use SidebarSection::*;
        assert_eq!(s.sidebar_order(), [Devices, Library, Favorites, Locations, Network, Nearby]);
        s.sidebar_sections = SidebarSection::ALL.to_vec();
        // Dragging down lands after the target, dragging up before it.
        s.move_sidebar_section(Favorites, Locations);
        assert_eq!(s.sidebar_order(), [Library, Locations, Favorites, Devices, Network, Nearby]);
        s.move_sidebar_section(Network, Library);
        assert_eq!(s.sidebar_order(), [Network, Library, Locations, Favorites, Devices, Nearby]);
        s.move_sidebar_section(Devices, Devices);
        assert_eq!(s.sidebar_order(), [Network, Library, Locations, Favorites, Devices, Nearby]);
    }

    #[test]
    fn default_apps_by_extension_then_category() {
        let t = std::env::temp_dir();
        let mut s = Settings::default();
        s.default_apps.insert(Settings::category_key(FileCategory::Code), t.clone());
        assert_eq!(s.default_app_for(std::path::Path::new("/x/main.RS")), Some(&t));
        assert_eq!(s.default_app_for(std::path::Path::new("/x/photo.png")), None);
        let other = t.parent().unwrap().to_path_buf();
        s.default_apps.insert(".rs".into(), other.clone());
        assert_eq!(s.default_app_for(std::path::Path::new("/x/main.rs")), Some(&other));
        // Maps serialize after the plain values.
        let parsed: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(parsed.default_apps.len(), 2);
    }

    #[test]
    fn roundtrip_and_partial() {
        let s = Settings { show_hidden: true, glass_hue: 215, glass_translucency: 72, sidebar_scroll_shadows: false,
            ai_model_path: Some(PathBuf::from("/models/chat.gguf")), ai_llama_server: "/opt/bin/llama-server".into(), ..Default::default() };
        let parsed: Settings = toml::from_str(&toml::to_string(&s).unwrap()).unwrap();
        assert!(parsed.show_hidden);
        assert_eq!(parsed.glass_hue, 215);
        assert_eq!(parsed.glass_translucency, 72);
        assert!(!parsed.sidebar_scroll_shadows);
        assert_eq!(parsed.ai_model_path, s.ai_model_path);
        assert_eq!(parsed.ai_llama_server, s.ai_llama_server);
        let partial: Settings = toml::from_str("theme = \"dark\"").unwrap();
        assert_eq!(partial.theme, ThemeMode::Dark);
        assert_eq!(partial.glass_hue, 0);
        assert_eq!(partial.glass_translucency, 50);
        assert!(partial.sidebar_scroll_shadows);
        assert_eq!(partial.app_icon, AppIcon::FolioBolt);
        assert_eq!(partial.ai_model_path, None);
        assert_eq!(partial.ai_llama_server, "llama-server");
        assert_eq!(partial.ai_thinking, AiThinking::Low);
    }

    #[test]
    fn thinking_levels_roundtrip() {
        for (level, budget) in [(AiThinking::None, 0), (AiThinking::Low, 64),
            (AiThinking::Medium, 128), (AiThinking::High, 256), (AiThinking::Unlimited, -1)] {
            let settings = Settings { ai_thinking: level, ..Default::default() };
            let parsed: Settings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
            assert_eq!(parsed.ai_thinking, level);
            assert_eq!(parsed.ai_thinking.token_budget(), budget);
        }
    }

    #[test]
    fn app_icon_uses_kebab_case() {
        let parsed: Settings = toml::from_str("app_icon = \"drawer-hot-rod\"").unwrap();
        assert_eq!(parsed.app_icon, AppIcon::DrawerHotRod);
        let s = Settings { app_icon: AppIcon::FolioEmber, ..Default::default() };
        assert!(toml::to_string(&s).unwrap().contains("app_icon = \"folio-ember\""));
    }
}
