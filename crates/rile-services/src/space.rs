//! Disk-usage analysis for the Space Analyzer page.
//!
//! [`scan`] walks a directory once, in parallel, into an in-memory [`Node`]
//! tree with the size of every file and folder. All views (space map, largest
//! files, cleanup suggestions) are queries on that tree, so drilling down or
//! going back never touches the filesystem again. After items are moved to the
//! Trash, [`Scan::remove`] patches the tree instead of rescanning.

use std::collections::{BinaryHeap, HashSet};
use std::cmp::Reverse;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rayon::prelude::*;

/// Deeper trees are counted but not descended into (guards against loops
/// through bind mounts and pathological nesting).
const MAX_DEPTH: usize = 256;
const DAY: u64 = 24 * 3600;

/// What a file is, for colouring the space map and the breakdown bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Category {
    Video,
    Images,
    Audio,
    Documents,
    Archives,
    Apps,
    Code,
    Other,
}

impl Category {
    pub const ALL: [Category; 8] =
        [Self::Video, Self::Images, Self::Audio, Self::Documents, Self::Archives, Self::Apps, Self::Code, Self::Other];

    pub fn label(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Images => "Images",
            Self::Audio => "Audio",
            Self::Documents => "Documents",
            Self::Archives => "Archives & installers",
            Self::Apps => "Apps",
            Self::Code => "Code & developer",
            Self::Other => "Other",
        }
    }

    pub fn of_file(name: &str) -> Self {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "mp4" | "mov" | "m4v" | "mkv" | "avi" | "webm" | "wmv" | "flv" | "mpg" | "mpeg" | "3gp" | "mts" | "braw" | "r3d" => Self::Video,
            "jpg" | "jpeg" | "png" | "gif" | "heic" | "heif" | "webp" | "tif" | "tiff" | "bmp" | "raw" | "cr2" | "cr3" | "nef" | "arw"
            | "dng" | "raf" | "orf" | "psd" | "svg" | "ico" | "icns" | "avif" | "exr" => Self::Images,
            "mp3" | "m4a" | "aac" | "wav" | "aif" | "aiff" | "flac" | "ogg" | "opus" | "alac" | "caf" | "mid" | "logicx" => Self::Audio,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "key" | "pages" | "numbers" | "txt" | "md" | "rtf" | "csv"
            | "epub" | "odt" | "ods" | "odp" => Self::Documents,
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "dmg" | "pkg" | "iso" | "xip" | "msi" | "img"
            | "sparseimage" | "sparsebundle" | "ipsw" => Self::Archives,
            "app" | "exe" | "appex" | "framework" | "dylib" | "so" | "dll" => Self::Apps,
            "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "rb" | "go" | "c" | "h" | "cpp" | "hpp" | "swift" | "kt" | "java" | "cs"
            | "json" | "toml" | "yaml" | "yml" | "lock" | "o" | "a" | "rlib" | "rmeta" | "wasm" | "class" | "jar" | "pyc" | "map"
            | "d" | "pdb" | "dsym" => Self::Code,
            _ => Self::Other,
        }
    }
}

/// One file or folder. Children are sorted largest first.
#[derive(Debug, Clone)]
pub struct Node {
    pub name: Box<str>,
    /// Bytes on disk (allocated blocks), including all descendants.
    pub size: u64,
    /// Modification time, seconds since the Unix epoch (0 = unknown).
    pub modified: u32,
    /// Number of files below (1 for a file).
    pub files: u32,
    pub is_dir: bool,
    /// What a file is (by name, or its location inside build/dependency
    /// folders). Folders use their breakdown instead.
    pub kind: Category,
    /// Bytes per [`Category`] (folders only; files use [`Node::category`]).
    pub breakdown: Option<Box<[u64; 8]>>,
    /// Boxed (not a `Vec`) so millions of nodes carry no spare capacity.
    pub children: Box<[Node]>,
}

impl Node {
    pub fn category(&self) -> Category {
        if self.is_dir {
            if is_bundle(self) { Category::Apps } else { self.dominant() }
        } else {
            self.kind
        }
    }

    /// The category that takes the most space below a folder.
    pub fn dominant(&self) -> Category {
        let Some(b) = &self.breakdown else { return Category::Other };
        let (ix, _) = b.iter().enumerate().max_by_key(|(_, v)| **v).unwrap_or((7, &0));
        Category::ALL[ix]
    }

    pub fn modified_secs(&self) -> u64 {
        self.modified as u64
    }

    pub fn folders(&self) -> u64 {
        self.children.iter().filter(|c| c.is_dir).map(|c| 1 + c.folders()).sum()
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| &*c.name == name)
    }
}

/// Live counters while a scan runs; read from the UI thread.
#[derive(Debug, Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub current: Mutex<PathBuf>,
}

impl Progress {
    pub fn current(&self) -> PathBuf {
        self.current.lock().map(|p| p.clone()).unwrap_or_default()
    }
}

#[derive(Debug)]
pub enum ScanError {
    Cancelled,
    Io(std::io::Error),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "cancelled"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

/// A finished analysis of `root`.
#[derive(Debug, Clone)]
pub struct Scan {
    pub root: PathBuf,
    pub tree: Node,
    pub finished: SystemTime,
    pub took: Duration,
    /// Folders that could not be read (usually missing permissions).
    pub unreadable: u64,
    pub suggestions: Vec<Suggestion>,
}

struct Walker<'a> {
    cancel: &'a AtomicBool,
    progress: &'a Progress,
    /// (device, inode) of multiply linked files already counted.
    links: Mutex<HashSet<(u64, u64)>>,
    skip: Vec<PathBuf>,
}

/// Scans `root` (not following symlinks). Blocks; run it off the UI thread.
pub fn scan(root: &Path, cancel: &AtomicBool, progress: &Progress) -> Result<Scan, ScanError> {
    let start = Instant::now();
    let meta = std::fs::metadata(root).map_err(ScanError::Io)?;
    if !meta.is_dir() {
        return Err(ScanError::Io(std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a folder")));
    }
    let walker = Walker { cancel, progress, links: Mutex::new(HashSet::new()), skip: skipped_mounts(root) };
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
    let tree = walker.dir(root, name.into(), modified_secs(&meta), 0, false);
    if cancel.load(Ordering::Relaxed) {
        return Err(ScanError::Cancelled);
    }
    let mut scan = Scan {
        root: root.to_path_buf(),
        tree,
        finished: SystemTime::now(),
        took: start.elapsed(),
        unreadable: progress.errors.load(Ordering::Relaxed),
        suggestions: Vec::new(),
    };
    scan.suggestions = suggestions(&scan);
    Ok(scan)
}

/// Other filesystems visible below `/` that would be counted twice or are
/// not real files (APFS firmlinks expose the data volume a second time).
fn skipped_mounts(root: &Path) -> Vec<PathBuf> {
    ["/Volumes", "/System/Volumes", "/dev", "/proc", "/sys", "/net", "/Network", "/private/var/vm"]
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.starts_with(root) && p != root)
        .collect()
}

impl Walker<'_> {
    fn dir(&self, path: &Path, name: Box<str>, modified: u32, depth: usize, dev: bool) -> Node {
        // Everything inside build output, dependencies and VCS data is developer data.
        let dev = dev || is_dev_dir(&name);
        let mut node = Node { name, size: 0, modified, files: 0, is_dir: true, kind: Category::Other, breakdown: Some(Box::new([0; 8])), children: Box::default() };
        if self.cancel.load(Ordering::Relaxed) || depth > MAX_DEPTH || self.skip.iter().any(|s| s == path) {
            return node;
        }
        self.progress.dirs.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut current) = self.progress.current.try_lock() {
            current.clone_from(&path.to_path_buf());
        }
        let Ok(read) = std::fs::read_dir(path) else {
            self.progress.errors.fetch_add(1, Ordering::Relaxed);
            return node;
        };
        let mut subdirs = Vec::new();
        let mut children = Vec::new();
        for entry in read.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let name: Box<str> = entry.file_name().to_string_lossy().into();
            if meta.is_dir() {
                subdirs.push((entry.path(), name, modified_secs(&meta)));
            } else {
                let size = self.allocated(&meta);
                self.progress.files.fetch_add(1, Ordering::Relaxed);
                self.progress.bytes.fetch_add(size, Ordering::Relaxed);
                let kind = match Category::of_file(&name) {
                    Category::Other | Category::Documents if dev => Category::Code,
                    k => k,
                };
                children.push(Node { name, size, modified: modified_secs(&meta), files: 1, is_dir: false, kind, breakdown: None, children: Box::default() });
            }
        }
        let dirs: Vec<Node> = subdirs.into_par_iter().map(|(p, n, m)| self.dir(&p, n, m, depth + 1, dev)).collect();
        children.extend(dirs);
        children.sort_unstable_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        node.children = children.into_boxed_slice();
        let breakdown = node.breakdown.as_mut().unwrap();
        for c in &node.children {
            node.size += c.size;
            node.files += c.files;
            match &c.breakdown {
                Some(b) => b.iter().enumerate().for_each(|(i, v)| breakdown[i] += v),
                None => breakdown[c.kind as usize] += c.size,
            }
        }
        if is_bundle(&node) {
            // A bundle counts as an app, whatever is inside it.
            let mut apps = [0; 8];
            apps[Category::Apps as usize] = node.size;
            node.breakdown = Some(Box::new(apps));
        }
        node
    }

    /// Bytes actually allocated; hard links are counted once.
    #[cfg(unix)]
    fn allocated(&self, meta: &std::fs::Metadata) -> u64 {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 && !self.links.lock().map(|mut l| l.insert((meta.dev(), meta.ino()))).unwrap_or(true) {
            return 0;
        }
        meta.blocks() * 512
    }

    #[cfg(windows)]
    fn allocated(&self, meta: &std::fs::Metadata) -> u64 {
        use std::os::windows::fs::MetadataExt;
        let _ = &self.links;
        // OneDrive and other cloud placeholders take no space until opened.
        const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
        const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
        if meta.file_attributes() & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0 {
            return 0;
        }
        meta.len()
    }

    #[cfg(not(any(unix, windows)))]
    fn allocated(&self, meta: &std::fs::Metadata) -> u64 {
        let _ = &self.links;
        meta.len()
    }
}

fn modified_secs(meta: &std::fs::Metadata) -> u32 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs().min(u32::MAX as u64) as u32)
}

/// A file found by [`Scan::largest_files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHit {
    pub path: PathBuf,
    pub size: u64,
    pub modified: u64,
    pub category: Category,
}

impl Scan {
    /// The node for `path`, which must be `root` or below it.
    pub fn node(&self, path: &Path) -> Option<&Node> {
        let mut node = &self.tree;
        for part in rel_components(&self.root, path)? {
            node = node.child(&part)?;
        }
        Some(node)
    }

    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }

    /// The `limit` largest files below `path`, largest first.
    pub fn largest_files(&self, path: &Path, limit: usize) -> Vec<FileHit> {
        let Some(node) = self.node(path) else { return Vec::new() };
        let mut heap: BinaryHeap<Reverse<(u64, Vec<usize>)>> = BinaryHeap::with_capacity(limit + 1);
        let mut trail = Vec::new();
        collect_largest(node, &mut trail, &mut heap, limit);
        let mut hits: Vec<FileHit> = heap
            .into_iter()
            .map(|Reverse((_, trail))| {
                let mut n = node;
                let mut p = path.to_path_buf();
                for ix in trail {
                    n = &n.children[ix];
                    p.push(&*n.name);
                }
                FileHit { path: p, size: n.size, modified: n.modified_secs(), category: n.category() }
            })
            .collect();
        hits.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
        hits
    }

    /// Drops `path` from the tree after it was deleted, subtracting its size
    /// from every ancestor. Returns the bytes it accounted for.
    pub fn remove(&mut self, path: &Path) -> Option<u64> {
        let parts = rel_components(&self.root, path)?;
        if parts.is_empty() {
            return None;
        }
        let removed = remove_below(&mut self.tree, &parts)?;
        self.suggestions.retain(|s| !s.path.starts_with(path));
        Some(removed.size)
    }

    /// Suggestions at or below `path`.
    pub fn suggestions_in<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Suggestion> + 'a {
        self.suggestions.iter().filter(move |s| s.path.starts_with(path))
    }
}

fn remove_below(node: &mut Node, parts: &[String]) -> Option<Node> {
    let ix = node.children.iter().position(|c| *c.name == *parts[0])?;
    let removed = if parts.len() == 1 {
        let mut children = std::mem::take(&mut node.children).into_vec();
        let removed = children.remove(ix);
        node.children = children.into_boxed_slice();
        removed
    } else {
        remove_below(&mut node.children[ix], &parts[1..])?
    };
    node.size = node.size.saturating_sub(removed.size);
    node.files = node.files.saturating_sub(removed.files);
    if let Some(b) = node.breakdown.as_mut() {
        match &removed.breakdown {
            Some(r) => b.iter_mut().zip(r.iter()).for_each(|(v, d)| *v = v.saturating_sub(*d)),
            None => {
                let v = &mut b[removed.kind as usize];
                *v = v.saturating_sub(removed.size);
            }
        }
    }
    Some(removed)
}

fn collect_largest(node: &Node, trail: &mut Vec<usize>, heap: &mut BinaryHeap<Reverse<(u64, Vec<usize>)>>, limit: usize) {
    for (ix, child) in node.children.iter().enumerate() {
        // Children are sorted, so nothing further down this list can qualify.
        if heap.len() >= limit && heap.peek().is_some_and(|Reverse((min, _))| child.size <= *min) {
            break;
        }
        trail.push(ix);
        if child.is_dir {
            collect_largest(child, trail, heap, limit);
        } else {
            heap.push(Reverse((child.size, trail.clone())));
            if heap.len() > limit {
                heap.pop();
            }
        }
        trail.pop();
    }
}

fn rel_components(root: &Path, path: &Path) -> Option<Vec<String>> {
    let rel = path.strip_prefix(root).ok()?;
    rel.components()
        .map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// Why something is suggested for cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SuggestionKind {
    /// Build output and dependencies that a build/install recreates.
    Developer,
    /// Application caches.
    Caches,
    /// Disk images and installers that already did their job.
    Installers,
    /// Old downloads nobody opened in months.
    Downloads,
    /// Large files untouched for over a year.
    Stale,
    /// Log files.
    Logs,
}

impl SuggestionKind {
    pub const ALL: [SuggestionKind; 6] = [Self::Developer, Self::Caches, Self::Installers, Self::Downloads, Self::Stale, Self::Logs];

    pub fn title(self) -> &'static str {
        match self {
            Self::Developer => "Build output & dependencies",
            Self::Caches => "Caches",
            Self::Installers => "Installers & disk images",
            Self::Downloads => "Old downloads",
            Self::Stale => "Large files you haven't touched",
            Self::Logs => "Logs",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Developer => "Recreated by the next build or package install.",
            Self::Caches => "Apps rebuild these when needed; they may start a little slower once.",
            Self::Installers => "Usually not needed once the app is installed.",
            Self::Downloads => "In Downloads and not modified for over 90 days.",
            Self::Stale => "Over 100 MB and not modified for more than a year. Make sure you have a copy.",
            Self::Logs => "Diagnostic logs that apps write over time.",
        }
    }

    /// Whether removing it loses nothing that can't be regenerated.
    pub fn regenerable(self) -> bool {
        matches!(self, Self::Developer | Self::Caches | Self::Logs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub path: PathBuf,
    pub size: u64,
    pub kind: SuggestionKind,
    /// Short reason, e.g. "npm packages".
    pub note: String,
    pub modified: u64,
    pub is_dir: bool,
}

/// Minimum sizes below which a suggestion is not worth showing.
const MIN_DIR: u64 = 1_000_000;
const MIN_INSTALLER: u64 = 10_000_000;
const MIN_DOWNLOAD: u64 = 10_000_000;
const MIN_STALE: u64 = 100_000_000;
const MIN_LOG: u64 = 1_000_000;

fn suggestions(scan: &Scan) -> Vec<Suggestion> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut out = Vec::new();
    let mut path = scan.root.clone();
    let in_downloads = scan.root.file_name().is_some_and(|n| n == "Downloads");
    visit(&scan.tree, &mut path, in_downloads, now, &mut out);
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(b.size.cmp(&a.size)));
    out
}

fn visit(node: &Node, path: &mut PathBuf, in_downloads: bool, now: u64, out: &mut Vec<Suggestion>) {
    for child in &node.children {
        path.push(&*child.name);
        let age = now.saturating_sub(child.modified_secs());
        let mut push = |kind, note: &str| {
            out.push(Suggestion { path: path.clone(), size: child.size, kind, note: note.into(), modified: child.modified_secs(), is_dir: child.is_dir });
        };
        let parent = path.parent().and_then(Path::file_name).and_then(|n| n.to_str()).unwrap_or("");
        let grandparent = path.parent().and_then(Path::parent).and_then(Path::file_name).and_then(|n| n.to_str()).unwrap_or("");
        if child.is_dir && is_package_store(parent, &child.name) {
            // Toolchains and module caches are read-only and managed by their
            // tools (`go clean -modcache`, `rustup`); nothing inside is suggested.
        } else if is_protected(path) {
            // System-managed folders can't be trashed; look inside instead.
            if child.is_dir {
                visit(child, path, false, now, out);
            }
        } else if parent == "Logs" && grandparent == "Library" {
            // Each app's logs inside ~/Library/Logs are offered separately.
            if child.size >= MIN_LOG {
                push(SuggestionKind::Logs, "App logs");
            }
        } else if child.is_dir {
            if &*child.name == ".Trash" || child.name.eq_ignore_ascii_case("$Recycle.Bin") || is_bundle(child) || child.name.ends_with(".photoslibrary") {
                // The Trash is already on its way out; bundles are managed by their app.
            } else if let Some(note) = developer_dir(node, child).filter(|_| child.size >= MIN_DIR) {
                push(SuggestionKind::Developer, note);
            } else if let Some(note) = cache_dir(path, child).filter(|_| child.size >= MIN_DIR) {
                push(SuggestionKind::Caches, note);
            } else if matches!(&*child.name, "Logs" | "logs" | "log") && log_dir(child) && child.size >= MIN_LOG {
                push(SuggestionKind::Logs, "Log folder");
            } else {
                visit(child, path, &*child.name == "Downloads", now, out);
            }
        } else {
            let ext = child.name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
            if matches!(ext.as_str(), "dmg" | "pkg" | "iso" | "xip" | "msi" | "mpkg") && child.size >= MIN_INSTALLER {
                push(SuggestionKind::Installers, "Installer");
            } else if in_downloads && age > 90 * DAY && child.size >= MIN_DOWNLOAD {
                push(SuggestionKind::Downloads, &format!("{} days old", age / DAY));
            } else if child.modified > 0 && age > 365 * DAY && child.size >= MIN_STALE {
                push(SuggestionKind::Stale, &format!("Last modified {} years ago", (age / (365 * DAY)).max(1)));
            } else if ext == "log" && child.size >= MIN_LOG {
                push(SuggestionKind::Logs, "Log file");
            }
        }
        path.pop();
    }
}

/// Folders macOS (or the system) manages: a `Library` folder and its direct
/// children (`Caches`, `Logs`, …), top-level system folders and home folders.
/// They are never suggested and never moved to the Trash; their contents may be.
pub fn is_protected(path: &Path) -> bool {
    let name = |p: &Path| p.file_name().and_then(|n| n.to_str()).map(str::to_owned);
    let depth = path.components().count();
    if cfg!(windows) {
        return is_protected_windows(path, depth, name);
    }
    depth <= 2
        || name(path).as_deref() == Some("Library")
        || path.parent().and_then(name).as_deref() == Some("Library")
        || path.parent().is_some_and(|p| p == Path::new("/Users"))
        || dirs::home_dir().is_some_and(|h| h == path)
}

/// Windows: drive roots and their top-level folders (`C:\Windows`,
/// `C:\Users`, `pagefile.sys`…), user profiles, `AppData` and its
/// `Local`/`Roaming`/`LocalLow`, the direct children of `Program Files`, and
/// everything inside the Windows folder.
fn is_protected_windows(path: &Path, depth: usize, name: impl Fn(&Path) -> Option<String>) -> bool {
    let lower = |s: Option<String>| s.map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let parent = path.parent();
    let system_root = std::env::var_os("SystemRoot").map(PathBuf::from);
    // Prefix + root is the drive itself; one more is a top-level item.
    depth <= 3
        || system_root.is_some_and(|w| crate::path_key(path).starts_with(crate::path_key(&w)))
        || dirs::home_dir().is_some_and(|h| crate::path_key(&h) == crate::path_key(path))
        || lower(name(path)) == "appdata"
        || parent.is_some_and(|p| lower(name(p)) == "appdata")
        || parent.is_some_and(|p| {
            p.components().count() == 3 && matches!(lower(name(p)).as_str(), "users" | "program files" | "program files (x86)" | "programdata")
        })
}

/// Package-manager stores whose contents look like projects (Go's module cache
/// holds the toolchain's own `vendor` folders) but must not be picked apart.
fn is_package_store(parent: &str, name: &str) -> bool {
    matches!((parent, name), ("pkg", "mod") | (_, ".rustup") | (_, ".cargo") | (_, ".m2") | (_, ".pnpm-store") | ("Library", "pnpm"))
}

fn is_dev_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules" | "target" | ".git" | ".svn" | ".hg" | "DerivedData" | ".gradle" | "__pycache__" | ".venv" | "venv" | "Pods"
            | ".next" | ".nuxt" | ".turbo" | ".cargo" | ".rustup" | ".npm" | ".pnpm-store" | ".yarn" | "site-packages" | ".m2"
            | "zig-cache" | ".zig-cache" | ".build" | "_build"
    )
}

/// A macOS app bundle (not merely a folder whose name ends in `.app`).
fn is_bundle(dir: &Node) -> bool {
    dir.is_dir && dir.name.ends_with(".app") && has(dir, "Contents")
}

fn has(dir: &Node, name: &str) -> bool {
    dir.children.iter().any(|c| &*c.name == name)
}

/// Build/dependency folders, only where the project file that recreates them
/// sits next to them (a folder called `build` alone is not enough).
fn developer_dir(parent: &Node, dir: &Node) -> Option<&'static str> {
    Some(match &*dir.name {
        "node_modules" => "npm packages",
        "target" if has(parent, "Cargo.toml") => "Rust build output",
        "DerivedData" => "Xcode build data",
        ".gradle" => "Gradle cache",
        "build" if has(parent, "build.gradle") || has(parent, "build.gradle.kts") => "Gradle build output",
        "build" | "dist" | "out" if has(parent, "package.json") => "JavaScript build output",
        ".next" | ".nuxt" | ".svelte-kit" | ".turbo" | ".parcel-cache" | ".angular" => "Framework build cache",
        "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" => "Python cache",
        ".venv" | "venv" if has(dir, "pyvenv.cfg") => "Python virtual environment",
        "Pods" if has(parent, "Podfile") => "CocoaPods dependencies",
        ".build" if has(parent, "Package.swift") => "Swift build output",
        "zig-cache" | ".zig-cache" | "zig-out" => "Zig build output",
        "_build" if has(parent, "mix.exs") => "Elixir build output",
        "vendor" if has(parent, "go.mod") && has(dir, "modules.txt") => "Go vendored modules",
        _ => return None,
    })
}

fn cache_dir(path: &Path, dir: &Node) -> Option<&'static str> {
    // Each app's folder inside a `Caches` folder is offered separately.
    if path.parent().and_then(Path::file_name).is_some_and(|n| n == "Caches") {
        return Some("App cache");
    }
    matches!(&*dir.name, ".cache" | "Cache" | "CachedData" | "GPUCache" | "Code Cache" | "ShaderCache" | "_cacache").then_some("Cache")
}

/// A folder named "Logs" counts only if it really holds mostly logs.
fn log_dir(dir: &Node) -> bool {
    dir.breakdown.as_ref().is_none_or(|b| b[Category::Video as usize] + b[Category::Images as usize] + b[Category::Audio as usize] < dir.size / 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7u8; bytes]).unwrap();
    }

    fn run(root: &Path) -> Scan {
        scan(root, &AtomicBool::new(false), &Progress::default()).unwrap()
    }

    #[test]
    fn sizes_roll_up_and_children_are_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        write(&root.join("big/a.mov"), 300_000);
        write(&root.join("big/deep/b.png"), 100_000);
        write(&root.join("small.txt"), 10);
        let scan = run(&root);
        assert_eq!(scan.tree.files, 3);
        assert_eq!(scan.tree.children[0].name.as_ref(), "big");
        let big = scan.node(&root.join("big")).unwrap();
        assert!(big.size >= 400_000);
        assert_eq!(big.size, big.children.iter().map(|c| c.size).sum::<u64>());
        assert_eq!(big.dominant(), Category::Video);
        assert_eq!(scan.node(&root.join("big/deep/b.png")).unwrap().category(), Category::Images);
        assert!(scan.node(&root.join("missing")).is_none());
        assert!(scan.node(Path::new("/elsewhere")).is_none());
    }

    #[test]
    fn largest_files_and_removal_patch_the_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        for i in 0..20 {
            write(&root.join(format!("d{}/f{i}.bin", i % 3)), 10_000 * (i + 1));
        }
        let mut scan = run(&root);
        let top = scan.largest_files(&root, 5);
        assert_eq!(top.len(), 5);
        assert!(top.windows(2).all(|w| w[0].size >= w[1].size));
        assert!(top[0].path.ends_with("f19.bin"));
        let before = scan.tree.size;
        let d1 = scan.node(&root.join("d1")).unwrap().clone();
        assert_eq!(scan.remove(&root.join("d1")), Some(d1.size));
        assert_eq!(scan.tree.size, before - d1.size);
        assert_eq!(scan.tree.files, 20 - d1.files);
        assert!(scan.node(&root.join("d1")).is_none());
        let f = root.join("d0/f0.bin");
        let fsize = scan.node(&f).unwrap().size;
        scan.remove(&f);
        assert_eq!(scan.tree.size, before - d1.size - fsize);
        assert_eq!(scan.tree.breakdown.as_ref().unwrap().iter().sum::<u64>(), scan.tree.size);
    }

    #[test]
    fn suggestions_need_their_project_file_and_do_not_nest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        write(&root.join("app/package.json"), 10);
        write(&root.join("app/node_modules/x/node_modules/y/big.js"), 2_000_000);
        write(&root.join("crate/Cargo.toml"), 10);
        write(&root.join("crate/target/debug/bin"), 2_000_000);
        write(&root.join("notes/target/keep.txt"), 2_000_000);
        write(&root.join("Library/Caches/com.app/blob"), 2_000_000);
        write(&root.join("Setup.dmg"), 20_000_000);
        write(&root.join("Logs/app.log"), 2_000_000);
        write(&root.join("Library/Logs/SomeApp/a.log"), 2_000_000);
        write(&root.join("Library/Logs/b.log"), 2_000_000);
        write(&root.join("go/pkg/mod/golang.org/toolchain@v1/src/cmd/go.mod"), 10);
        write(&root.join("go/pkg/mod/golang.org/toolchain@v1/src/cmd/vendor/modules.txt"), 2_000_000);
        let scan = run(&root);
        let paths: Vec<_> = scan.suggestions.iter().map(|s| (s.kind, s.path.strip_prefix(&root).unwrap().to_path_buf())).collect();
        assert!(paths.contains(&(SuggestionKind::Developer, "app/node_modules".into())));
        assert!(paths.contains(&(SuggestionKind::Developer, "crate/target".into())));
        assert!(paths.contains(&(SuggestionKind::Caches, "Library/Caches/com.app".into())));
        assert!(paths.contains(&(SuggestionKind::Installers, "Setup.dmg".into())));
        assert!(paths.contains(&(SuggestionKind::Logs, "Logs".into())));
        assert!(paths.contains(&(SuggestionKind::Logs, "Library/Logs/SomeApp".into())));
        assert!(paths.contains(&(SuggestionKind::Logs, "Library/Logs/b.log".into())));
        assert!(!paths.iter().any(|(_, p)| p == Path::new("Library/Logs") || p == Path::new("Library/Caches")), "protected folders are never offered");
        assert!(!paths.iter().any(|(_, p)| p.starts_with("notes")), "target without Cargo.toml is user data");
        assert!(!paths.iter().any(|(_, p)| p.starts_with("go")), "module caches are left to their tools");
        assert!(!paths.iter().any(|(_, p)| p.ends_with("y")), "nested suggestions are not double counted");
    }

    #[test]
    fn system_folders_are_protected() {
        for p in ["/", "/Users", "/System", "/Users/me", "/Users/me/Library", "/Users/me/Library/Logs", "/Library/Caches"] {
            assert!(is_protected(Path::new(p)), "{p}");
        }
        for p in ["/Users/me/Library/Logs/App", "/Users/me/Library/Caches/com.app", "/Users/me/Coding/x/node_modules"] {
            assert!(!is_protected(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn cancel_stops_the_scan() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/b"), 1);
        let err = scan(tmp.path(), &AtomicBool::new(true), &Progress::default()).unwrap_err();
        assert!(matches!(err, ScanError::Cancelled));
        assert!(matches!(scan(&tmp.path().join("a/b"), &AtomicBool::new(false), &Progress::default()), Err(ScanError::Io(_))));
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_and_symlinks_count_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        write(&root.join("a"), 100_000);
        fs::hard_link(root.join("a"), root.join("b")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        let scan = run(&root);
        let a = scan.node(&root.join("a")).unwrap().size + scan.node(&root.join("b")).unwrap().size;
        assert!(a >= 100_000 && a < 200_000);
        assert!(!scan.node(&root.join("loop")).unwrap().is_dir);
    }
}
