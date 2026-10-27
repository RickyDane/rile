# Rile plugins

Rile plugins are sandboxed **WebAssembly components** implementing the
`rile:plugin@0.1.0` world defined in
[`crates/rile-plugins/wit/rile-plugin.wit`](../crates/rile-plugins/wit/rile-plugin.wit).
They run inside wasmtime with no filesystem preopens, no environment variables
and no network. The only way to touch files is the `host` interface:

| Import | Purpose | Sandbox rule |
| --- | --- | --- |
| `log(msg)` | write to Rile's log (stderr) | - |
| `notify(msg)` | queue a user-visible notification (delivered after the call) | max 64 per call |
| `read-text(path, max-bytes)` | read a file as lossy UTF-8 (capped at 16 MiB) | path must be in the current selection (for `preview`, the previewed file) |
| `list-dir(path)` | sorted entry names of a directory | current dir, selected entries, their parent dirs, and direct children of those; in page calls also anything below the page root |
| `read-dir(path)` | entries with `is-dir`, `size`, `modified` (symlinks not followed) | same as `list-dir` |
| `dir-size(path)` | total bytes below a path, computed by the host | page calls only, path must be below the page root |

Paths are canonicalized before checking, so `..` and symlinks cannot escape.
Each call gets a fuel budget (~5 billion wasm instructions, roughly half a
second) and a 256 MiB memory cap; a plugin that exceeds it traps, the error is
reported, and the plugin is re-instantiated on the next call. Don't rely on
state persisting between calls.

A plugin exports:

- `manifest()` – `id` (unique, e.g. `com.example.foo`), `name`, `version`, `description`.
- `commands(ctx)` – commands to offer for the current `context` (`current-dir`, `selection`).
- `run-command(id, ctx)` – returns a list of actions: `open(path)`, `navigate(dir)`,
  `refresh`, `copy-text(text)`, `notify(msg)`.
- `preview(path, ext)` – a text preview for the file (`ext` is lowercase, without dot),
  or `none` if the plugin does not handle it. The first enabled plugin returning
  `some` wins.
- `pages()` – custom pages shown in the sidebar (`id`, `title`, optional `icon`: `chart`, `disk`, `folder`).
- `render-page(req)` / `page-event(req, event)` – return a `page-update`: the
  page UI, a new opaque `state` string (handed back next call, kept by the host
  so it survives re-instantiation) and actions. Pages may also return
  `choose-root` (folder picker) and `set-root(dir)`; both re-render the page.

### Pages

A page works on a **root** folder (initially the explorer's current folder).
During page calls the plugin may `list-dir`/`read-dir`/`dir-size` anything below it.
The UI is a flat node list; each node names its parent container by index
(`none` = top level, laid out in a column):

| Node | Renders as |
| --- | --- |
| `column`, `row` | containers (rows wrap) |
| `heading`, `text`, `muted` | text |
| `button { label, event }` | button; click sends `event` |
| `list-item { title, detail, trailing, event? }` | list row, clickable when `event` is set |
| `progress(f32)` | 0..1 bar |
| `bubbles(list<bubble>)` | packed bubble chart, area ∝ `value`; bubbles with `event` are clickable |
| `spacer` | vertical gap |

Invalid parents become top-level; views are capped at 20 000 nodes, 500 bubbles
per chart and 4 MiB of state. The `hello` plugin's "Folder Sizes" page is a
minimal example.

## Writing a plugin in Rust

```sh
rustup target add wasm32-wasip2
cargo new --lib my-plugin
```

`Cargo.toml`:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.57"
```

Copy (or reference) the `wit/` directory and implement the generated `Guest` trait:

```rust
wit_bindgen::generate!({ path: "wit", world: "plugin" });

struct MyPlugin;

impl Guest for MyPlugin {
    fn manifest() -> Manifest { /* ... */ }
    fn commands(ctx: Context) -> Vec<Command> { /* ... */ }
    fn run_command(id: String, ctx: Context) -> Result<Vec<Action>, String> { /* ... */ }
    fn preview(path: String, ext: String) -> Option<String> { None }
}

export!(MyPlugin);
```

Host functions are available as `rile::plugin::host::{log, notify, read_text, list_dir}`.
See [`hello/`](hello/) for a complete example (copy file names, count lines,
CSV table preview).

Any language that can produce a WebAssembly component for this world works
(e.g. via `wasm-tools component new`, `componentize-py`, `jco`).

## Build and install

```sh
cd plugins/hello
cargo build --release --target wasm32-wasip2
mkdir -p ~/Library/Application\ Support/rile/plugins        # macOS
cp target/wasm32-wasip2/release/rile_plugin_hello.wasm ~/Library/Application\ Support/rile/plugins/
```

Rile loads every `*.wasm` file in its plugin directory:

- `$RILE_CONFIG_DIR/plugins` if `RILE_CONFIG_DIR` is set, otherwise
- `<config dir>/rile/plugins` – `~/Library/Application Support/rile/plugins` on
  macOS, `~/.config/rile/plugins` on Linux, `%APPDATA%\rile\plugins` on Windows.

Files that fail to load are reported individually; the rest still load.
Plugin ids must be unique.
