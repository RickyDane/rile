#!/usr/bin/env python3
"""Generates the alternative icon sets (Phosphor fill / duotone / light) for the
Lucide icons Rile uses. Usage: gen_icon_sets.py <phosphor-core>/assets

Writes assets/icons/<set>/<lucide-name>.svg and src/icon_sets.rs."""
import os, re, sys

# Lucide name -> Phosphor name
MAP = {
    "folder": "folder", "folder-open": "folder-open", "folder-plus": "folder-plus",
    "folder-symlink": "folder-simple-dashed", "folder-up": "folder-simple", "file": "file",
    "file-text": "file-text", "file-code": "file-code", "file-archive": "file-zip",
    "file-image": "file-image", "file-music": "file-audio", "file-video-camera": "file-video",
    "file-spreadsheet": "file-xls", "file-symlink": "file-dashed", "house": "house",
    "star": "star", "hard-drive": "hard-drive", "usb": "usb", "arrow-left": "arrow-left",
    "arrow-right": "arrow-right", "arrow-up": "arrow-up", "chevron-right": "caret-right",
    "layout-list": "list-bullets", "monitor": "monitor", "download": "download-simple",
    "image": "image", "music": "music-notes", "clapperboard": "film-slate", "settings": "gear",
    "sun": "sun", "moon": "moon", "sun-moon": "circle-half", "eye": "eye", "eye-off": "eye-slash",
    "rotate-cw": "arrow-clockwise", "copy": "copy", "scissors": "scissors",
    "clipboard-paste": "clipboard-text", "pencil": "pencil-simple", "trash": "trash", "x": "x",
    "search": "magnifying-glass", "keyboard": "keyboard", "sliders-horizontal": "sliders-horizontal",
    "server": "hard-drives", "globe": "globe", "plus": "plus", "layout-grid": "squares-four",
    "columns-2": "columns", "columns-3": "kanban", "panel-right": "sidebar", "panel-left": "sidebar-simple",
    "sparkles": "sparkle", "eject": "eject", "info": "info", "arrow-down-wide-narrow": "sort-descending",
    "atom": "atom", "calendar": "calendar-blank", "chevron-left": "caret-left", "ellipsis": "dots-three",
    "heart": "heart", "maximize-2": "arrows-out-simple", "tag": "tag", "circle-x": "x-circle",
    "puzzle": "puzzle-piece", "triangle-alert": "warning", "activity": "pulse",
    "arrow-down-up": "arrows-down-up", "check": "check",
    # context menus, archives, gallery, AI
    "archive": "archive", "package-open": "package", "app-window": "app-window",
    "external-link": "arrow-square-out", "images": "images", "files": "files", "link": "link",
    "file-plus": "file-plus", "wand-sparkles": "magic-wand", "layout-dashboard": "squares-four",
    "trash-2": "trash-simple", "text-search": "text-aa", "brain": "brain", "grip-vertical": "dots-six-vertical",
    "app-window-mac": "app-store-logo",
}
SETS = {"fill": "fill", "duotone": "duotone", "light": "light"}

# Glyphs whose duotone tint is a free-standing square behind the strokes
# rather than the inside of a shape; at UI sizes it reads as a background
# spilling out of the icon, so they keep only the strokes.
DUOTONE_NO_TINT = {
    "plus", "x", "check", "download", "ellipsis", "maximize-2", "grip-vertical", "arrow-down-wide-narrow",
}
# Phosphor tints only a file's dog-ear / a folder's tab; in the file list
# that reads as nearly outline-only, so shade the whole body instead.
FILE_BODY = '<path d="M208,88V216a8,8,0,0,1-8,8H56a8,8,0,0,1-8-8V40a8,8,0,0,1,8-8h96v56Z" opacity="0.2"/>'
FOLDER_BODY = '<path d="M216,80H128L98.34,50.34A8,8,0,0,0,92.69,48H40a8,8,0,0,0-8,8V200a8,8,0,0,0,8,8H216a8,8,0,0,0,8-8V88A8,8,0,0,0,216,80Z" opacity="0.2"/>'
DUOTONE_BODY = {n: FILE_BODY for n in ("file", "file-archive", "file-code", "file-image", "file-music", "file-plus",
                                       "file-spreadsheet", "file-symlink", "file-text", "file-video-camera")}
DUOTONE_BODY.update({"folder": FOLDER_BODY, "folder-plus": FOLDER_BODY})
TINT = re.compile(r'<(path|rect|circle|polygon|ellipse)\b[^>]*opacity="0\.2"[^>]*/>')

def main():
    src = sys.argv[1]
    here = os.path.dirname(os.path.abspath(__file__))
    crate = os.path.dirname(here)
    entries = []
    missing = []
    for set_name, weight in SETS.items():
        out = os.path.join(crate, "assets", "icons", set_name)
        os.makedirs(out, exist_ok=True)
        for lucide, phos in sorted(MAP.items()):
            f = os.path.join(src, weight, f"{phos}-{weight}.svg")
            if not os.path.exists(f):
                missing.append(f"{weight}/{phos}")
                continue
            svg = open(f).read()
            if set_name == "duotone" and lucide in DUOTONE_NO_TINT:
                svg = TINT.sub("", svg)
                assert "opacity" not in svg, lucide
            if set_name == "duotone" and lucide in DUOTONE_BODY:
                svg = TINT.sub("", svg).replace(">", ">" + DUOTONE_BODY[lucide], 1)
            open(os.path.join(out, f"{lucide}.svg"), "w").write(svg)
            entries.append((f"icons/{set_name}/{lucide}.svg", f"../assets/icons/{set_name}/{lucide}.svg"))
    if missing:
        sys.exit("missing: " + ", ".join(missing))
    with open(os.path.join(crate, "src", "icon_sets.rs"), "w") as rs:
        rs.write("// @generated by scripts/gen_icon_sets.py (Phosphor Icons, MIT). Do not edit.\n\n")
        rs.write("pub static ICONS: &[(&str, &[u8])] = &[\n")
        for path, file in entries:
            rs.write(f'    ("{path}", include_bytes!("{file}")),\n')
        rs.write("];\n")
    print(f"{len(entries)} icons")

main()
