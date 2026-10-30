# Rile explorer preview

An interactive workspace aligned with the current native app and `docs/screenshots/native-*.png`: Home, Photos, native Space Analyzer, Backups, network connections, Appearance, and the file explorer. Both platform previews share the design-guide control language, legible typography, current sidebar organization, and sample data.

```sh
python3 -m http.server 4173 --bind 127.0.0.1 --directory mockups
```

Open http://localhost:4173. No packages, build step, external fonts, or image services are required.

### Windows preview

Open http://localhost:4173/?platform=windows for Home, or http://localhost:4173/?platform=windows&preview=files for the explorer. Windows uses Segoe typography, right-aligned caption controls, This PC, NTFS storage, Windows paths, and Ctrl+F hints. Caption controls are illustrative, not native window actions. Use the sidebar footer to switch platforms.

### Current screens

Use the sidebar, or deep-link with `?page=photos`, `?page=space`, `?page=backups`, `?page=network`, or `?page=settings`. Add `&platform=windows` to any page.

- Home has six metrics, recent activity, storage, recent files, and links to workspace tools.
- Space Analyzer starts with explicit folder selection. Explore calculated sample sizes, drill into folders, switch to largest files, or review archive cleanup with confirmation. `space-analyzer.html` now forwards here; `?legacy=1` retains the historical plugin study.
- Backups supports editing a sample plan, simulated run history, and a restore confirmation. No real backup is performed.
- Photos supports filename filtering, illustrative image previews, and revealing an image in its folder.
- Network offers a preview-only connection form. No network requests or credentials are stored.
- Appearance previews accent colors, comfortable/compact density, and platform switching for this session.

`design-system.html` remains the control-language reference. `tabs.html`, `storage-indicators.html`, and `app-icons/` are isolated design studies, not the current application shell.

`shelf.html` previews an inset shelf popup: a padded collection tray, individual remove controls, an empty state, and separated file/device actions. Use Reset preview to restore its sample files; no file operations are performed.

## Interface

- A dedicated Home/start page, separate from `rickyperlick`'s home directory, opens by default and in new tabs. It includes storage cards, discovery and file-type graphs, workspace counts, recent files, and location shortcuts.
- The Overview uses original, inline vector illustrations and accessible, data-driven charts; no image downloads or third-party artwork are required.
- The Native explorer keeps the conventional layout: sidebar on the left, toolbar and tabs on top, files in the center, and inspector on the right.
- The Home page uses the redesigned Overview. Add `?preview=files` to open the sample project instead.
- Graphite chrome, a persistent places sidebar, dimensional blue folders, and file-type icons.
- One compact navigation toolbar, folder tabs, icon/list/column views, breadcrumbs, and a file inspector.
- A sample `rile` project with source files, folders, images, and documents—not a branding portfolio.
- Narrow layouts use an icon rail and an inspector that can be opened as an overlay.

## Interactions

Navigate with the sidebar, breadcrumbs, double-clicked folders, or back/forward buttons. Add, switch, and close folder tabs. Search filters the current view; sort by name, modification date, or kind. The options menu copies the current path or shows hidden files. Use the zoom slider to adjust the desktop icon grid.

On Home, search matches files across the sample workspace. Storage cards open their disks, recent files reveal their parent folder, and shortcuts open everyday locations (including the user's home directory). File and folder counts and the file-type chart are derived from the current in-memory data. Disk capacities, usage, and the seven-day discovery timeline are explicitly illustrative—not live telemetry or an indexer.

Select a file to update the inspector. Favorite it, reveal its parent folder, or open an illustrative Quick Look. Favorites and tag locations reflect the sample data. Create folders within a directory using the new-folder button; duplicate and invalid names are rejected.

Keyboard: `⌘F` / `Ctrl+F` or `/` focuses search; arrow keys move between files; Enter opens a selection; Space opens Quick Look; Escape dismisses menus and dialogs. Left/right arrows switch focused tabs.

Run the dependency-free sample-model and renderer regression checks with `node --test mockups/workspace.test.cjs` from the repository root. Layout and interactive states should also be reviewed in the browser.

**This remains a browser prototype, not the Rust/GPUI application.** All file data, storage figures, dates, and previews are illustrative. No real files are accessed or modified. Changes last until reload. Clipboard access is used only when you choose “Copy folder path.”
