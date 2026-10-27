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
| `list-dir(path)` | sorted entry names of a directory | current dir, selected entries, their parent dirs, and direct children of those |

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
