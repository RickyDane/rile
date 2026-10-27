//! Space Analyzer: find big files and folders worth cleaning up.
//!
//! Build: `cargo build --release --target wasm32-wasip2`
//!
//! Page events: `choose`, `up`, `tab:<overview|files|cleanup>`, `open:<dir>`,
//! `reveal:<path>`, `copy:<path>`. The selected tab is kept in the page state.

wit_bindgen::generate!({
    path: "../../crates/rile-plugins/wit",
    world: "plugin",
});

use rile::plugin::host;
use rile::plugin::types::{Bubble, Button, ListItem, Node, NodeKind, PageView};

/// Max entries visited while scanning for large files (keeps us inside the fuel budget).
const SCAN_LIMIT: usize = 20_000;
const SCAN_DEPTH: usize = 12;
const TOP_FILES: usize = 50;
/// Folder names that are usually safe to regenerate.
const JUNK_DIRS: &[(&str, &str)] = &[
    ("node_modules", "npm dependencies"),
    ("target", "Rust build output"),
    (".cache", "cache"),
    ("Caches", "cache"),
    ("DerivedData", "Xcode build data"),
    ("__pycache__", "Python bytecode"),
    (".venv", "Python virtualenv"),
    ("build", "build output"),
    ("dist", "build output"),
    (".gradle", "Gradle cache"),
    (".next", "Next.js build"),
    (".Trash", "Trash"),
];
const JUNK_EXTS: &[&str] = &["log", "tmp", "dmg", "iso", "zip", "pkg"];
const YEAR_SECS: u64 = 365 * 24 * 3600;

struct SpaceAnalyzer;

struct FileHit {
    path: String,
    size: u64,
    modified: Option<u64>,
}

impl Guest for SpaceAnalyzer {
    fn manifest() -> Manifest {
        Manifest {
            id: "rile.space-analyzer".into(),
            name: "Space Analyzer".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Find big files and folders to clean up".into(),
        }
    }

    fn commands(_ctx: Context) -> Vec<Command> {
        Vec::new()
    }

    fn run_command(id: String, _ctx: Context) -> Result<Vec<Action>, String> {
        Err(format!("unknown command {id}"))
    }

    fn preview(_path: String, _ext: String) -> Option<String> {
        None
    }

    fn pages() -> Vec<Page> {
        vec![Page { id: "space".into(), title: "Space Analyzer".into(), icon: Some("disk".into()) }]
    }

    fn render_page(req: PageRequest) -> Result<PageUpdate, String> {
        render(req.root, tab_of(&req.state))
    }

    fn page_event(req: PageRequest, event: String) -> Result<PageUpdate, String> {
        let mut tab = tab_of(&req.state);
        let mut actions = Vec::new();
        if event == "choose" {
            actions.push(Action::ChooseRoot);
        } else if event == "up" {
            if let Some(p) = req.root.as_deref().and_then(parent_dir) {
                actions.push(Action::SetRoot(p.to_string()));
            }
        } else if let Some(t) = event.strip_prefix("tab:") {
            tab = t.to_string();
        } else if let Some(dir) = event.strip_prefix("open:") {
            actions.push(Action::SetRoot(dir.to_string()));
        } else if let Some(path) = event.strip_prefix("reveal:") {
            if let Some(p) = parent_dir(path) {
                actions.push(Action::Navigate(p.to_string()));
            }
        } else if let Some(path) = event.strip_prefix("copy:") {
            actions.push(Action::CopyText(path.to_string()));
            actions.push(Action::Notify(format!("Copied {}", file_name(path))));
        }
        // Root changes re-render anyway; skip the (expensive) scan here.
        if actions.iter().any(|a| matches!(a, Action::SetRoot(_) | Action::ChooseRoot)) {
            return Ok(PageUpdate { view: PageView { nodes: Vec::new() }, state: tab, actions });
        }
        let mut update = render(req.root, tab)?;
        update.actions = actions;
        Ok(update)
    }
}

fn tab_of(state: &str) -> String {
    match state {
        "files" | "cleanup" => state.to_string(),
        _ => "overview".to_string(),
    }
}

struct View(Vec<Node>);

impl View {
    fn push(&mut self, parent: Option<u32>, kind: NodeKind) -> u32 {
        self.0.push(Node { parent, kind });
        (self.0.len() - 1) as u32
    }
    fn button(&mut self, parent: u32, label: &str, event: String) {
        self.push(Some(parent), NodeKind::Button(Button { label: label.into(), event }));
    }
}

fn render(root: Option<String>, tab: String) -> Result<PageUpdate, String> {
    let mut v = View(Vec::new());
    let bar = v.push(None, NodeKind::Row);
    v.button(bar, "Choose Folder…", "choose".into());
    v.button(bar, "Up", "up".into());
    let Some(root) = root else {
        v.push(None, NodeKind::Muted("Choose a folder to analyze.".into()));
        return Ok(PageUpdate { view: PageView { nodes: v.0 }, state: tab, actions: Vec::new() });
    };
    let tabs = v.push(None, NodeKind::Row);
    for (id, label) in [("overview", "Overview"), ("files", "Largest Files"), ("cleanup", "Cleanup")] {
        let label = if id == tab { format!("● {label}") } else { label.to_string() };
        v.button(tabs, &label, format!("tab:{id}"));
    }
    v.push(None, NodeKind::Heading(root.clone()));
    match tab.as_str() {
        "files" => files_tab(&mut v, &root),
        "cleanup" => cleanup_tab(&mut v, &root),
        _ => overview_tab(&mut v, &root)?,
    }
    Ok(PageUpdate { view: PageView { nodes: v.0 }, state: tab, actions: Vec::new() })
}

fn overview_tab(v: &mut View, root: &str) -> Result<(), String> {
    let mut items: Vec<(String, String, bool, u64)> = host::read_dir(root)?
        .into_iter()
        .map(|e| {
            let path = join(root, &e.name);
            let size = if e.is_dir { host::dir_size(&path).unwrap_or(0) } else { e.size };
            (e.name, path, e.is_dir, size)
        })
        .collect();
    items.sort_by(|a, b| b.3.cmp(&a.3));
    let total: u64 = items.iter().map(|i| i.3).sum::<u64>().max(1);
    v.push(None, NodeKind::Text(format!("{} in {} items", human_size(total), items.len())));
    v.push(
        None,
        NodeKind::Bubbles(
            items
                .iter()
                .take(60)
                .map(|(name, path, is_dir, size)| Bubble {
                    label: name.clone(),
                    detail: human_size(*size),
                    value: *size as f64,
                    color: junk_reason(name, *is_dir).map(|_| 0xE0_8A_3C),
                    event: is_dir.then(|| format!("open:{path}")),
                })
                .collect(),
        ),
    );
    for (name, path, is_dir, size) in items.iter().take(100) {
        let share = *size as f64 / total as f64;
        v.push(
            None,
            NodeKind::ListItem(ListItem {
                title: name.clone(),
                detail: format!("{} · {:.1}%", if *is_dir { "Folder" } else { "File" }, share * 100.0),
                trailing: human_size(*size),
                event: Some(if *is_dir { format!("open:{path}") } else { format!("reveal:{path}") }),
            }),
        );
        v.push(None, NodeKind::Progress(share as f32));
    }
    Ok(())
}

fn files_tab(v: &mut View, root: &str) {
    let (mut files, _, truncated) = scan(root);
    files.sort_by(|a, b| b.size.cmp(&a.size));
    files.truncate(TOP_FILES);
    if truncated {
        v.push(None, NodeKind::Muted(format!("Scan stopped after {SCAN_LIMIT} entries; open a subfolder for complete results.")));
    }
    if files.is_empty() {
        v.push(None, NodeKind::Muted("No files found.".into()));
    }
    for f in &files {
        file_row(v, root, f, None);
    }
}

fn cleanup_tab(v: &mut View, root: &str) {
    let (files, junk_dirs, truncated) = scan(root);
    let now = files.iter().filter_map(|f| f.modified).max().unwrap_or(0);
    let mut dirs: Vec<(String, &str, u64)> = junk_dirs
        .into_iter()
        .map(|(p, reason)| {
            let size = host::dir_size(&p).unwrap_or(0);
            (p, reason, size)
        })
        .collect();
    dirs.sort_by(|a, b| b.2.cmp(&a.2));
    let mut stale: Vec<&FileHit> = files
        .iter()
        .filter(|f| f.size >= 50_000_000 && f.modified.is_some_and(|m| now.saturating_sub(m) > YEAR_SECS))
        .collect();
    stale.sort_by(|a, b| b.size.cmp(&a.size));
    let mut by_ext: Vec<&FileHit> = files
        .iter()
        .filter(|f| f.size >= 1_000_000 && JUNK_EXTS.contains(&ext(&f.path).as_str()))
        .collect();
    by_ext.sort_by(|a, b| b.size.cmp(&a.size));

    let reclaim: u64 = dirs.iter().map(|d| d.2).sum::<u64>()
        + stale.iter().map(|f| f.size).sum::<u64>()
        + by_ext.iter().map(|f| f.size).sum::<u64>();
    v.push(None, NodeKind::Text(format!("Up to {} could be reclaimed", human_size(reclaim))));
    v.push(None, NodeKind::Muted("Rile never deletes anything automatically — review each item first.".into()));
    if truncated {
        v.push(None, NodeKind::Muted(format!("Scan stopped after {SCAN_LIMIT} entries.")));
    }

    v.push(None, NodeKind::Spacer);
    v.push(None, NodeKind::Heading(format!("Regenerable folders ({})", dirs.len())));
    for (path, reason, size) in dirs.iter().take(40) {
        v.push(
            None,
            NodeKind::ListItem(ListItem {
                title: rel(root, path),
                detail: reason.to_string(),
                trailing: human_size(*size),
                event: Some(format!("reveal:{path}")),
            }),
        );
    }
    v.push(None, NodeKind::Spacer);
    v.push(None, NodeKind::Heading(format!("Large, untouched for a year ({})", stale.len())));
    for f in stale.iter().take(30) {
        file_row(v, root, f, Some("not modified for 1+ year"));
    }
    v.push(None, NodeKind::Spacer);
    v.push(None, NodeKind::Heading(format!("Logs, archives & installers ({})", by_ext.len())));
    for f in by_ext.iter().take(30) {
        file_row(v, root, f, None);
    }
}

fn file_row(v: &mut View, root: &str, f: &FileHit, note: Option<&str>) {
    let row = v.push(None, NodeKind::Row);
    v.push(
        Some(row),
        NodeKind::ListItem(ListItem {
            title: rel(root, &f.path),
            detail: note.map(str::to_string).unwrap_or_else(|| ext(&f.path).to_uppercase()),
            trailing: human_size(f.size),
            event: Some(format!("reveal:{}", f.path)),
        }),
    );
    v.button(row, "Copy Path", format!("copy:{}", f.path));
}

/// Walks below `root`, collecting files and junk folders (which are not descended into).
fn scan(root: &str) -> (Vec<FileHit>, Vec<(String, &'static str)>, bool) {
    let mut files = Vec::new();
    let mut junk = Vec::new();
    let mut stack = vec![(root.to_string(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = host::read_dir(&dir) else { continue };
        for e in entries {
            seen += 1;
            if seen > SCAN_LIMIT {
                return (files, junk, true);
            }
            let path = join(&dir, &e.name);
            if e.is_dir {
                if let Some(reason) = junk_reason(&e.name, true) {
                    junk.push((path, reason));
                } else if depth < SCAN_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else {
                files.push(FileHit { path, size: e.size, modified: e.modified });
            }
        }
    }
    (files, junk, false)
}

fn junk_reason(name: &str, is_dir: bool) -> Option<&'static str> {
    if !is_dir {
        return None;
    }
    JUNK_DIRS.iter().find(|(n, _)| *n == name).map(|(_, r)| *r)
}

fn join(dir: &str, name: &str) -> String {
    let sep = if dir.ends_with(['/', '\\']) { "" } else { "/" };
    format!("{dir}{sep}{name}")
}

fn rel(root: &str, path: &str) -> String {
    path.strip_prefix(root).map(|p| p.trim_start_matches(['/', '\\']).to_string()).unwrap_or_else(|| path.to_string())
}

fn ext(path: &str) -> String {
    let name = file_name(path);
    name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

fn parent_dir(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let idx = trimmed.rfind(['/', '\\'])?;
    Some(if idx == 0 { "/" } else { &trimmed[..idx] })
}

fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path)
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

export!(SpaceAnalyzer);
