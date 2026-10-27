//! Example Rile plugin.
//!
//! Build: `cargo build --release --target wasm32-wasip2`

wit_bindgen::generate!({
    path: "../../crates/rile-plugins/wit",
    world: "plugin",
});

use rile::plugin::host;

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
