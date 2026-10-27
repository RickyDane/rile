//! Example Rile plugin.
//!
//! Build: `cargo build --release --target wasm32-wasip2`

wit_bindgen::generate!({
    path: "../../crates/rile-plugins/wit",
    world: "plugin",
});

use rile::plugin::host;
use rile::plugin::types::{Bubble, Button, ListItem, Node, NodeKind, PageView};

const MAX_PREVIEW_ROWS: usize = 50;
const MAX_CELL_WIDTH: usize = 40;

struct Hello;

impl Guest for Hello {
    fn manifest() -> Manifest {
        Manifest {
            id: "rile.hello".into(),
            name: "Hello".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Example plugin: copy file names, count lines, CSV preview".into(),
        }
    }

    fn commands(ctx: Context) -> Vec<Command> {
        if ctx.selection.is_empty() {
            return Vec::new();
        }
        vec![
            Command {
                id: "copy-names".into(),
                label: "Copy file names".into(),
            },
            Command {
                id: "count-lines".into(),
                label: "Count lines".into(),
            },
        ]
    }

    fn run_command(id: String, ctx: Context) -> Result<Vec<Action>, String> {
        match id.as_str() {
            "copy-names" => {
                let names: Vec<&str> = ctx.selection.iter().map(|p| file_name(p)).collect();
                Ok(vec![Action::CopyText(names.join("\n"))])
            }
            "count-lines" => {
                let mut total = 0usize;
                for path in &ctx.selection {
                    match host::read_text(path, 64 * 1024 * 1024) {
                        Ok(text) => total += text.lines().count(),
                        Err(e) => host::log(&format!("skipping {path}: {e}")),
                    }
                }
                host::notify(&format!("{total} lines"));
                Ok(Vec::new())
            }
            other => Err(format!("unknown command `{other}`")),
        }
    }

    fn preview(path: String, ext: String) -> Option<String> {
        if ext != "csv" {
            return None;
        }
        let text = host::read_text(&path, 1024 * 1024).ok()?;
        Some(render_csv(&text))
    }

    fn pages() -> Vec<Page> {
        vec![Page {
            id: "sizes".into(),
            title: "Folder Sizes".into(),
            icon: Some("chart".into()),
        }]
    }

    fn render_page(req: PageRequest) -> Result<PageUpdate, String> {
        sizes_page(req.root)
    }

    /// Events: `choose` (pick a folder), `up`, `open:<dir>` (drill down).
    fn page_event(req: PageRequest, event: String) -> Result<PageUpdate, String> {
        let mut update = sizes_page(req.root.clone())?;
        if event == "choose" {
            update.actions.push(Action::ChooseRoot);
        } else if event == "up" {
            if let Some(parent) = req.root.as_deref().and_then(parent_dir) {
                update.actions.push(Action::SetRoot(parent.to_string()));
            }
        } else if let Some(dir) = event.strip_prefix("open:") {
            update.actions.push(Action::SetRoot(dir.to_string()));
        }
        Ok(update)
    }
}

/// Example page: the entries of the root as bubbles sized by disk usage.
fn sizes_page(root: Option<String>) -> Result<PageUpdate, String> {
    let mut nodes = Vec::new();
    let mut push = |parent: Option<u32>, kind: NodeKind| {
        nodes.push(Node { parent, kind });
        (nodes.len() - 1) as u32
    };
    let bar = push(None, NodeKind::Row);
    push(Some(bar), NodeKind::Button(Button { label: "Choose Folder…".into(), event: "choose".into() }));
    push(Some(bar), NodeKind::Button(Button { label: "Up".into(), event: "up".into() }));
    let Some(root) = root else {
        push(None, NodeKind::Muted("Choose a folder to analyze.".into()));
        return Ok(PageUpdate { view: PageView { nodes }, state: String::new(), actions: Vec::new() });
    };
    push(None, NodeKind::Heading(file_name(&root).to_string()));
    let sep = if root.ends_with(['/', '\\']) { "" } else { "/" };
    let mut items: Vec<(String, String, bool, u64)> = host::read_dir(&root)?
        .into_iter()
        .map(|e| {
            let path = format!("{root}{sep}{}", e.name);
            let size = if e.is_dir { host::dir_size(&path).unwrap_or(0) } else { e.size };
            (e.name, path, e.is_dir, size)
        })
        .collect();
    items.sort_by(|a, b| b.3.cmp(&a.3));
    let total: u64 = items.iter().map(|i| i.3).sum();
    push(None, NodeKind::Muted(format!("{} in {} items", human_size(total), items.len())));
    push(
        None,
        NodeKind::Bubbles(
            items
                .iter()
                .take(40)
                .map(|(name, path, is_dir, size)| Bubble {
                    label: name.clone(),
                    detail: human_size(*size),
                    value: *size as f64,
                    color: None,
                    event: is_dir.then(|| format!("open:{path}")),
                })
                .collect(),
        ),
    );
    for (name, path, is_dir, size) in items.iter().take(100) {
        push(
            None,
            NodeKind::ListItem(ListItem {
                title: name.clone(),
                detail: if *is_dir { "Folder".into() } else { "File".into() },
                trailing: human_size(*size),
                event: is_dir.then(|| format!("open:{path}")),
            }),
        );
    }
    Ok(PageUpdate { view: PageView { nodes }, state: String::new(), actions: Vec::new() })
}

fn parent_dir(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let idx = trimmed.rfind(['/', '\\'])?;
    Some(if idx == 0 { "/" } else { &trimmed[..idx] })
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

fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

/// Splits one CSV line into fields (supports double-quoted fields).
fn split_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => fields.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

fn truncate_cell(s: &str) -> String {
    if s.chars().count() > MAX_CELL_WIDTH {
        let mut out: String = s.chars().take(MAX_CELL_WIDTH - 1).collect();
        out.push('…');
        out
    } else {
        s.to_string()
    }
}

fn render_csv(text: &str) -> String {
    let rows: Vec<Vec<String>> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(MAX_PREVIEW_ROWS)
        .map(|l| {
            split_csv_line(l.trim_end_matches('\r'))
                .iter()
                .map(|f| truncate_cell(f))
                .collect()
        })
        .collect();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; columns];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for (r, row) in rows.iter().enumerate() {
        let cells: Vec<String> = (0..columns)
            .map(|i| {
                let cell = row.get(i).map(String::as_str).unwrap_or("");
                let pad = widths[i] - cell.chars().count();
                format!("{cell}{}", " ".repeat(pad))
            })
            .collect();
        out.push_str(cells.join(" | ").trim_end());
        out.push('\n');
        if r == 0 && rows.len() > 1 {
            let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
            out.push_str(&sep.join("-+-"));
            out.push('\n');
        }
    }
    out
}

export!(Hello);
