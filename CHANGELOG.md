# Changelog

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
