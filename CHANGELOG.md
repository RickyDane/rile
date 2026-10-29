# Changelog

## Unreleased

## v0.8.0-beta.5

### Rile Server (Linux)
- New headless Nearby device for Linux servers. It runs as a systemd service and shows up under **Nearby** like any other Rile device.
- Install with `curl -fsSL https://raw.githubusercontent.com/RickyDane/rile/master/packaging/linux/install.sh | sudo sh`. The interactive setup lets you share the whole machine, one folder or several. Folders can be picked from suggestions, typed with Tab completion, or browsed.
- `rilectl` changes shares, access, name, inbox and port later, pairs devices (`rilectl pair` shows the code to confirm), lists and unpairs devices, follows the log, and uninstalls.
- Files sent to the server are saved in its inbox automatically.
- Releases include static `rile-server-<triple>.tar.gz` builds for x86_64 and aarch64 Linux.

### Nearby Devices
- Shared folders can now be read & write. On devices that allow it, you can create folders, upload, rename and delete in Rile. Older devices stay read-only.
- Pairing with a Rile Server explains how to confirm the code on the server.
- Folders you share with Nearby devices show a "shared" badge in the list, grid and column views.
- Device details are more compact.

### Updates
- Fix "Restart Now" after an update sometimes not quitting the app.

## v0.8.0-beta.4

### Windows
- Enlarge the Folded Folio icon to fill the taskbar icon area, reducing transparent padding and increasing its height.

### macOS
- Fix hardened-runtime crashes when running WebAssembly plugins by enabling executable memory for the plugin JIT.

## v0.8.0-beta.3

### Nearby Devices
- Switch between card and compact list views, with the selected view remembered.
- Show device hardware specifications, including model or processor, memory and form factor, when available.

## v0.8.0-beta.2

### Windows
- Remove app icon customization; use the default icon consistently. macOS icon customization is unchanged.
- Use the transparent Folded Folio icon for the executable, installer, About dialog, backup tray and drag preview, without the macOS background tile.

## v0.8.0-beta.1

### Cloud storage
- Add Google Drive and OneDrive connections for browsing and transferring files.

### Interface and fixes
- Refine the sidebar, device views, backup menus and status bar.
- Fix restarting the app after an update on macOS.

### Nearby Devices
- Find other computers running Rile on the local network (Bonjour `_rile._tcp`). They appear under **Nearby** in the sidebar and on the new **Nearby Devices** page, a card for each device with its model, system, storage and shared folders.
- Pair two devices once by confirming the same 6-digit code on both screens. Devices talk over mutual TLS 1.3 with a key generated on each computer; the code comes from the encrypted session, so a device in the middle would show a different one.
- Share folders read-only with paired devices (right-click a folder → Share with Nearby Devices, or Settings → Nearby). Paired devices browse, preview and copy from them like a server.
- Send files and folders to a paired device by dropping them on it, with **Send to Device** in the context menu, or **Send Files…**. The other side accepts or declines; accepted files go to Downloads (configurable) and appear as a transfer job.

## v0.7.4

### macOS
- Space the sidebar toggle button evenly from the top and right edges of the sidebar header at every sidebar width.

## v0.7.3

### Drag and drop
- Fix Rile freezing and crashing on Windows when dragging files out to the desktop or another app. The native drag now runs outside GPUI's event handling, so the window keeps drawing during the drag.

### Windows
- Make the sidebar toggle button clickable on Windows; the header around it still drags the window.

## v0.7.2

### Drag and drop
- Restore dragging local files and folders from Rile onto the desktop and into other apps on macOS and Windows using native drag sessions.
- Preserve multi-selection and internal folder drops, and clear drag previews and hover state when handing files to the operating system.
- Add regression coverage for native handoff at all four window edges.

## v0.7.1

### Backups
- Show an estimated time remaining during backup transfers on the Backups page and in the backup status menu.

## v0.7.0

### Windows support
- Run Rile natively on Windows 10 and 11. The toolbar holds the window's minimize, maximize and close buttons and drags the window, with Snap support. Shortcuts use Ctrl and follow Explorer's keys (Alt+←/→/↑, Alt+D, Ctrl+Shift+C to copy a path), and AltGr keeps typing characters. Labels use Windows terms such as Recycle Bin and Preview.
- Show Explorer's icons and thumbnails, Open With from the file type's registered apps or any Start menu program, PDF previews (Windows.Data.Pdf), Windows Search for index search and the photo library, photo metadata, text recognition in photos, owner, attributes and size on disk in Properties, and Safely Remove for USB drives.
- Hide files that have the Hidden or System attribute. Reject file names that Windows doesn't allow. Copy and delete read-only files, and skip symlinks when Windows doesn't allow creating them.
- One Rile per session: opening folders, `rile://` links or Rile again reuses the running window. Explorer gets **Open in Rile**, and Rile can optionally become the default folder handler.
- Background backups through Task Scheduler and a backup status icon in the notification area. Stale backup locks are detected on Windows too.
- Store downloaded models and llama.cpp runtimes in `%LOCALAPPDATA%`, show installed memory, and stop the runtime together with Rile.
- Ship a per-user Inno Setup installer and a `rile.exe` zip for the in-app updater; CI also builds and tests on Windows.

### Fixes on all platforms
- Skip remote files whose names would write outside the download folder.
- Keep a copied file's modification date when the file is read-only.
- Preview UTF-16 text files (with a byte order mark) as text.
- Store SSH known hosts in Rile's configuration folder, so `RILE_CONFIG_DIR` applies to them too.

## v0.6.2

### Search fixes
- Match multi-word filename searches across spaces, underscores, hyphens, and dots, consistently in live searches and discovery memory; preserve case-sensitive matching and literal glob/regex behavior.
- Check the requested scope for matching filenames before Smart Search consults the AI model, so undiscovered files are not missed; preserve search filters, cancellation, and streamed results.
- Prevent malformed AI replies or invalid folder-opening actions from returning unrelated files as Smart Search results.

### AI settings
- Redesign Settings → AI to the design guide: an overview with the master switch and live model status, Apple Intelligence / Local model tabs with a numbered runtime and model-file setup, and a Photo descriptions group with **Describe All** and a confirmed **Clear…**. Photo actions explain why they're unavailable when AI is off.
- Pick any already downloaded GGUF model from a list in Local model mode; vision projectors and later shards of split models are left out.
- Run MLX models from Hugging Face on Apple silicon: a new **MLX** tab installs the mlx-lm runtime (checksum-verified uv with its own Python; no Homebrew or Terminal), downloads a whole MLX repository such as `mlx-community/Qwen3-4B-4bit` with SHA-256 checks, and lists downloaded MLX folders. Intelligent search and summaries stream from `mlx_lm.server` on loopback.
- Load the selected local model in the background when Rile starts and keep it in memory, so intelligent search and summaries no longer wait for the model on every request. The AI overview shows when the model is loading or loaded. Turning AI off or choosing Apple Intelligence frees the memory.
- Ask before switching away from the local model in use, since switching unloads it and loads the new selection.
- Delete downloaded GGUF and MLX models from the Downloaded models list after a confirmation; split models go with all their shards, emptied folders are removed, and deleting the model in use unloads it first.
- Overhaul model downloads: each local tab lists recommended models (Qwen 3.5 2B/4B/9B, Gemma 4 E4B, Qwen 3.8 27B) with download size, memory needs checked against this computer, and one-click **Download**. Progress, speed and **Cancel** show on the model's own row, downloaded models are grouped under **On this computer**, and runtime installs show progress on the Runtime step. **Custom from Hugging Face** lists a repository's GGUF files to pick from instead of typing the exact filename.
- Model downloads and runtime installs keep running after Settings closes; reopening Settings shows their progress again, and a notification reports the result. Progress now shows transfer speed (MB/s) and time left.

### macOS integration
- Apply the file-viewer preference independently when macOS rejects folder bindings with an invalid-parameter error; report partial integration without claiming folder opening changed, and allow restoring the viewer to Finder.
- Set and detect the default file manager using the registered folder content type instead of inspecting a sample folder, avoiding access to the protected `/Users` directory.
- Check system API availability before offering to make Rile the default file manager, and include the macOS error domain and code when changing the folder handler fails.

### Maintenance
- Add regression tests for filename matching, discovery-memory consistency, Smart Search fallbacks, and folder-type association; update macOS type-identifier dependencies.

## v0.6.1

- Refine the toolbar update button with a compact, accent-tinted appearance and a download icon.

## v0.6.0

### Backups
- New built-in Backups page and Settings tab for creating, editing, pausing, and deleting backup jobs.
- Incremental backups of multiple files or folders to local folders, mounted drives, or saved FTP/FTPS, SFTP, and SMB connections. Backups remain ordinary, browsable files.
- Manual, hourly, daily, and weekly runs; optional automatic runs after source changes; macOS launch agents for schedules while Rile is closed.
- Optional login-started menu bar helper with job status and shortcuts into Rile.
- Configurable exclusion globs and retention of replaced/deleted files in dated version folders.
- Run history, live progress, transfer rates, cancellation, verification, and repair of missing destination files.
- Restore the latest backup or a run's retained versions to original locations or another folder; optionally Trash replaced local files.
- Delete retained versions, individual run records, or an entire backup.
- Destination ownership/nesting checks, missing-source protection, duplicate-source-name handling, partial-write handling, and per-file error reporting.
- Dedicated remote connections keep backups from blocking file browsing.

### Space Analyzer
- Replaces the bundled WebAssembly Space Analyzer plugin with a native page and sidebar entry; hides retired plugin pages to prevent duplicates.
- Parallel, cancellable folder scanning into a cached tree; drill down and use back/forward without rescanning.
- Interactive bubble map, category breakdown, folder contents, largest-file list, and cleanup suggestions for caches, build output, installers, and stale downloads.
- Explicit folder selection through Rile's explorer, rescan controls, multi-selection, and confirmed Trash actions.
- Updates cached usage after cleanup instead of scanning again; reports cleanup failures and tracks space reclaimed and folders analyzed in achievements.

### Search and macOS integration
- AI search can match remembered subject folders across languages, then recursively stream their files with type and modification-date filters.
- Preserves complete directory listings in search memory; rejects malformed AI folder selections and retains the browsing fallback for content-specific requests.
- Settings controls to make the installed Rile.app the default folder viewer or restore Finder, including compatible Reveal/Show in Finder actions.
- App-bundle folder registration, open-document/URL handling, grouped file reveals, and cold-start request handling.
- Reopens or activates the main workspace when requested; saves workspace state when its window closes.
- Adds `rile://` links for opening Backups and its settings from the helper.

### Appearance and plugin pages
- Eight selectable Dock icon designs: Folio/Drawer Bolt, Flame, Ember, and Hot Rod, each following the eight accent colors.
- New default app icon, pre-rendered icon assets, generation script, selection previews, settings persistence, and asset tests.
- More consistent pointer/disabled-button behavior, menu icons, and Apple Intelligence glyphs across settings, previews, updates, gallery, and overview controls.
- Plugin pages no longer implicitly scan the current folder: choose a root explicitly using the explorer, with cancel/return handling.
- Plugin bubble-chart zoom is host-managed; root changes reset zoom, and picker-only updates preserve the existing page.

### Fixes, build, and documentation
- FTP LIST fallback probes support for `LIST -a`, so hidden folders such as `.git` remain visible on compatible servers.
- Merges master’s Avrea runner migration for CI and release builds; CI installs the wasm32-wasip2 target.
- Adds the rile-backup workspace crate and supporting dependencies; updates the lockfile and removes the retired Space Analyzer plugin source.
- Adds regression coverage for backups/FTP, semantic retrieval, folder opening/reveals, icons, and UI behavior.
- Adds design guidance, a design-system mockup, shared mockup icon helpers, four rounds of app-icon studies, and an updated Space Analyzer mockup.
- Documents default-file-manager setup and native Space Analyzer usage; updates plugin documentation and removes an unused SMB import.
- Includes the existing tracked Finder metadata change and repository design instructions.

### Also included since the last published release (v0.4.0)

The v0.5.0 tag was not published as a GitHub release. This release also includes:
- SMB2/3 connections, available-share browsing, direct share paths, saved credentials, and mount integration.
- Bonjour/mDNS discovery of nearby file servers, with local-network permission declarations.
- Smooth photo zoom and bounded drag-to-pan in the gallery.
- Markdown previews resolving relative/local images and embedded image data URLs.
- Syntax-aware Quick Look editing for code, plain text, Markdown, and RTF, including Visual Studio solution highlighting and expanded language recognition.
- Remote-operation, sidebar, updater, and regression-test improvements, plus the vendored SMB compatibility patch.
