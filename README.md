# Rile

A fast, index-free file explorer written in Rust with [GPUI](https://gpui-kit.com).

## UI preview

The approved Native design lives as an interactive browser prototype in [`mockups/`](mockups/README.md) (`python3 -m http.server 4173 --bind 127.0.0.1 --directory mockups`, then visit http://localhost:4173). The native app implements it; the prototype's sample figures are illustrative only, while the app's Overview shows live data.

## Features
- List, grid (with image thumbnails) and Miller-column views; dual-pane mode (Tab, F5 copy / F6 move to other pane)
- Native dark chrome: sidebar (favorites, locations, devices with eject), toolbar with breadcrumbs, folder tabs (⌘T / ⌘W / ⌃Tab), inspector (⇧⌘P), path bar, icon-size zoom
- Overview start page (Home, separate from your home folder): live disk usage, the top level of your places by kind, recent changes from file dates and recently modified files — computed on demand, no index or history
- File-type artwork in the icon grid, back/forward history, `rile <folder>` opens a folder, `rile <file>` reveals a file
- Copy/cut/paste/duplicate/rename/trash/delete, new file/folder, with animated progress + cancel; drag & drop (Alt = copy), drops from Finder, spring-loaded sidebar targets; drag-to-select (rubber band)
- FTP / FTPS / SFTP connections (sidebar → Network): browse, Quick Look, upload/download by drag, F5/F6 or paste, rename, delete; passwords in the Keychain
- Real app icons, mouse back/forward buttons
- Quick Look (Space), preview panel, Properties (⌘I) with recursive folder totals
- Search runs on Enter, without filtering the current listing while you type; recursive search (name contains/glob/regex, content, size, date, type, .gitignore), optional Spotlight backend; folders you open are remembered so searching them again is instant (no background crawling, re-checked on disk, can be turned off or cleared in Settings → Search)
- Sidebar devices show how full they are; settings and a background jobs panel (copies, moves, server transfers with progress and cancel) sit at the bottom of the sidebar
- On-device AI (Apple Foundation Models via `spotlight-llm`, Apple Vision): type in the main search field and press Shift+Enter (or click the AI icon) for intelligent search, with results directly in the file list; Enter keeps regular recursive search. AI reviews remembered file/folder names by meaning across languages (e.g. “outgoing invoices” → “Ausgangsrechnungen”), then browses promising folders and uses smart tags; summaries of files and folders; "Describe & Tag with AI" for images (labels + OCR) and documents
- Photo library (sidebar → Photos): every image in your home folder and Photos library, grouped by month, searchable by content once described
- Centered transfer window (current file + overall progress, speed, ETA; "Run in Background"), compress/extract (zip, 7z, tar, tar.gz/br/zst, gz, br, zst with selectable level), Open With and per-type default programs
- Quick Look PDF viewer and syntax highlighting; recursive name search on servers
- Four icon styles (Lucide outline; Phosphor filled, duotone, light — MIT, regenerate with `crates/rile-app/scripts/gen_icon_sets.py`), glass effect, resizable sidebar, any startup folder
- Sandboxed WebAssembly plugins (context-menu commands, previewers) — see `plugins/README.md`
- Themes: system/light/dark, accent colors, density; GitHub-release self-updater; ⌘/ lists all shortcuts

## Build
```sh
cargo run -p rile-app            # Rust stable >= 1.99 (pinned in rust-toolchain.toml)
cargo run -p rile-app --no-default-features   # without AI (non-Apple platforms build this way automatically)
cargo test --workspace
```

### macOS app icon and bundle

The Pathline icons are embedded in the executable, so the Dock follows the
selected accent immediately, even with `cargo run`. The Finder icon uses the
default blue artwork; the running app's Dock icon reflects the selected accent.
Build a Finder-launchable app with `sh crates/rile-app/scripts/bundle_macos.sh`
(or add `--debug`); the result is `target/release/Rile.app` (`target/debug/Rile.app`
with `--debug`). Copy that bundle to Applications if desired.

The accent artwork is in `crates/rile-app/assets/app-icons/`. After editing it,
regenerate the checked-in `.icns` files with
`sh crates/rile-app/scripts/gen_app_icon.sh` (macOS and `brew install librsvg`).
Normal builds do not need the icon-generation tools.

## Layout
| Crate | Purpose |
|---|---|
| `rile-core` | listing, sorting, file ops, history, settings, watcher |
| `rile-services` | search, properties, preview loading, updater, volume unmount |
| `rile-plugins` | wasmtime component host + WIT (`wit/rile-plugin.wit`) |
| `rile-remote` | FTP/FTPS (suppaftp) and SFTP (russh) behind a blocking `RemoteFs` API, keychain credentials |
| `rile-app` | GPUI application |

Settings live in `~/Library/Application Support/rile/settings.toml` (override with `RILE_CONFIG_DIR`); plugins in its `plugins/` folder.
