# Rile

**A fast file explorer built in Rust with [GPUI](https://gpui-kit.com).**

Browse local files and remote servers, preview what matters, and search your files. Rile brings tabs, multiple views, file operations, and on-device intelligence together in a native desktop interface.

[**Download for macOS and Windows**](https://github.com/RickyDane/rile/releases/latest) · [What's new in v0.7.0](CHANGELOG.md)

## See it in action

Real captures of the native macOS app, using an illustrative Studio workspace and generated sample artwork. Storage totals, devices, and other system information reflect the capture machine—not demo values painted over the UI. Click any image for a closer look.

![Rile's thumbnail grid with sample landscape artwork and an image inspector](docs/screenshots/native-grid.png)

### Find your flow

Browse with an inspector at your side, or put two folders next to each other.

| List view and inspector | Dual-pane browsing |
| :---: | :---: |
| ![File list with a Markdown project brief selected and its details in the inspector](docs/screenshots/native-list.png) | ![Photography and Studio folders open side by side in Rile](docs/screenshots/native-dual-pane.png) |

### Make room for what matters

See where storage goes, then review cleanup suggestions before moving anything to the Trash.

| Space Analyzer | Cleanup suggestions |
| :---: | :---: |
| ![Space Analyzer showing a folder's category breakdown, bubble map, and largest subfolders](docs/screenshots/native-space-analyzer.png) | ![Cleanup suggestions identifying regenerable dependencies in the sample workspace](docs/screenshots/native-cleanup.png) |

### Keep an eye on your files

Back up your work and check its run history. Get a live overview of storage and file activity from Home.

| Backups | Home |
| :---: | :---: |
| ![A completed Studio backup with retained versions and a successful run in its history](docs/screenshots/native-backups.png) | ![Home dashboard with storage totals, recent file activity, and a file-type breakdown](docs/screenshots/native-home.png) |

### Look closer. Make it yours.

Read Markdown without leaving your folder, and choose the icons and layout that suit you.

| Quick Look | Appearance |
| :---: | :---: |
| ![Quick Look rendering the sample Markdown project brief](docs/screenshots/native-preview.png) | ![Appearance settings with interface icon styles, eight Dock icon designs, and layout controls](docs/screenshots/native-appearance.png) |

An interactive design prototype is also available in [`mockups/`](mockups/README.md); its sample data is illustrative and is not the native application.

## What you can do

- **Browse your way:** switch between list, thumbnail grid, and Miller-column views; open folder tabs or work in two panes. Navigate with breadcrumbs, history, favorites, and mounted devices.
- **Work with files:** copy, move, rename, duplicate, trash, or delete; drag and drop between folders, Finder, and remote servers. Transfers support progress, cancellation, and background execution.
- **Inspect and preview:** use Quick Look, the inspector, or Properties for files and recursive folder totals. Preview PDFs, images, and syntax-highlighted text.
- **Search on demand:** search recursively by name, glob, regex, content, size, date, and type, with `.gitignore` support and an optional Spotlight backend. A discovery index remembers visited folders for faster subsequent searches; it can be disabled or cleared in Settings.
- **Nearby Devices:** other computers running Rile on your network show up automatically. Pair once with a 6-digit code, then browse the folders they share (read-only) and send each other files, which the receiver accepts first. Everything travels directly over the local network with mutual TLS.
- **Connect to servers:** browse FTP, FTPS, SFTP, and SMB2/3 locations, transfer files, and manage remote entries. Choose nearby servers advertised over Bonjour/mDNS in the connection dialog, or enter a server manually. SMB connections show available disk shares; enter `smb://server/share` or set the start folder to `/share/folder` to connect directly. Credentials are stored in the Keychain.
- **Explore storage and photos:** the Overview shows live disk usage and recent file activity; Photos groups images from your home folder and Photos library by month.
- **Free up space:** the Space Analyzer measures a folder once, then lets you explore it as a bubble map, list its largest files, and review cleanup suggestions (caches, build output, installers, stale downloads) before moving them to the Trash.
- **Back up your work:** create incremental, versioned backups to local drives or saved server connections. Run them manually, on a schedule, or after changes; review history and restore files from the Backups page.
- **Use on-device AI:** on supported Apple systems, search by meaning, summarize files and folders, and describe or tag images and documents. Press **Shift+Enter** in search for AI search; **Enter** runs regular search.
- **Make it yours:** choose themes, accents, density, icon styles, and a startup folder. Extend context menus and previews with sandboxed [WebAssembly plugins](plugins/README.md).
- **Handle archives:** compress and extract ZIP, 7z, tar and supported compressed formats, with selectable compression levels.

## Getting started

### Run from source

Install a recent stable Rust toolchain (the workspace selects `stable` in [`rust-toolchain.toml`](rust-toolchain.toml)), then run:

```sh
cargo run -p rile-app
```

To start in a specific folder, pass its path after `--`. Passing a file reveals it instead:

```sh
cargo run -p rile-app -- ~/Documents
```

The default `ai` feature uses Apple Foundation Models on Apple Silicon with macOS 26 or later. To build without it:

```sh
cargo run -p rile-app --no-default-features
```

You can also use local GGUF models or download them from Hugging Face in **Settings → AI**.
Install the llama.cpp runtime directly from AI Settings or use your own executable; see [local model setup](docs/local-models.md).

### Build a macOS app

```sh
sh crates/rile-app/scripts/bundle_macos.sh
```

The bundle is written to `target/release/Rile.app`. Add `--debug` for `target/debug/Rile.app`. The release packaging script, [`scripts/bundle-macos.sh`](scripts/bundle-macos.sh), also supports signing, notarization, and DMG creation. Normal builds do not require the icon-generation tools; see [`gen_app_icon.sh`](crates/rile-app/scripts/gen_app_icon.sh) if you change the icon artwork.

### Build for Windows

On Windows 10 or 11 with the Rust MSVC toolchain and the Windows SDK (gpui compiles its shaders with `fxc.exe`):

```powershell
cargo run -p rile-app
pwsh scripts/bundle-windows.ps1   # dist/rile-x86_64-pc-windows-msvc.zip and the installer (needs Inno Setup 6)
```

The installer installs per user to `%LOCALAPPDATA%\Programs\Rile` without administrator rights. It adds **Open in Rile** to the context menu of folders and drives, and registers `rile://` links. Background backups run as a Task Scheduler task (`Rile\dev.rile.backup`), and the backup status helper sits in the notification area. Search through the OS index uses Windows Search; photo descriptions use the Windows text recognizer, and Apple Intelligence is not available. For AI features, choose a llama.cpp model in **Settings → AI**. From macOS you can type-check the Windows build with `cargo xwin check --workspace --target x86_64-pc-windows-msvc`.

### Use Rile as the default file manager

On Windows, choose **Make Rile Default** in **Settings → General → Windows Integration**. Folders opened from Explorer, shortcuts and other apps then open in Rile. **Restore Explorer** undoes it. Win+E and the desktop still use Explorer.

On macOS 12 or later, move `Rile.app` to Applications and launch that copy. In **Settings → General → macOS Integration**, choose **Make Rile Default** and approve macOS's confirmation if prompted. This changes folder opening and compatible **Reveal/Show in Finder** actions. Choose **Restore Finder** in the same section to revert; do this before uninstalling Rile.

Other applications may cache the previous viewer: restart them, or log out and back in, if necessary. Finder still manages the desktop, system Open/Save dialogs are unchanged, and applications that explicitly target Finder may continue to use it. Running the bare binary with `cargo run` does not provide the app-bundle registration required to become the default.

Some macOS versions reject folder associations, even when restoring Finder. In that case Rile still updates the separate file-viewer preference for compatible **Reveal/Show in Finder** actions and reports the partial change. Folder opening remains unchanged; this is not a full default-file-manager replacement. **Restore Finder** also restores the file-viewer preference in this situation. Cancelling a consent prompt does not apply this fallback.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `⌘T` / `⌘W` | Open / close a tab |
| `⌃Tab` | Switch tabs |
| `Tab` | Switch panes in dual-pane mode |
| `F5` / `F6` | Copy / move to the other pane |
| `Space` | Quick Look |
| `⌘I` | Properties |
| `⇧⌘P` | Toggle inspector |
| `⌘/` | Show all shortcuts |

## Configuration and plugins

On macOS, settings are stored in `~/Library/Application Support/rile/settings.toml`. On Windows they are in `%APPDATA%\rile\settings.toml`, and downloaded models and AI runtimes are in `%LOCALAPPDATA%\rile`. Set `RILE_CONFIG_DIR` to use another configuration directory. Plugins live in its `plugins/` folder; see the [plugin guide](plugins/README.md) for development and installation details.

## Development

```sh
cargo build --workspace --locked
cargo test --workspace --locked
```

| Crate | Responsibility |
| --- | --- |
| `rile-app` | GPUI application and interface |
| `rile-core` | File listing, operations, settings, history, and watching |
| `rile-backup` | Incremental backups, retained versions, restore, and background scheduling |
| `rile-services` | Search, previews, archives, properties, and system services |
| `rile-remote` | FTP/FTPS, SFTP, and SMB backends, Nearby Devices (Rile ↔ Rile), local-network discovery, and credentials |
| `rile-plugins` | Wasmtime component host and plugin interface |

The workspace declares the MIT license in [`Cargo.toml`](Cargo.toml).
