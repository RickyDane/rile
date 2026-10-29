"""Five alternative styles of the full-color Overview symbols (64 x 64 artboards).

Usage: python3 -I generate.py <out dir> [--app]

With --app, the suites offered in Settings > Appearance > Symbol style are
also copied into crates/rile-app/assets/overview/<style>/ (art.rs embeds them).

Every suite draws the same 11 symbols as crates/rile-app/assets/overview/.
free-space.svg keeps the hooks art.rs rewrites per capacity level: the arc
path "M23.5 40.5a12 12 0 0 1 17-17" with stroke="url(#rile-free-arc)", the
tint colors #5fd39a / #85c6fa / #2e80cc (used only in that gradient) and the
needle d="M32 32 38.5 25.5".
"""
import os
import sys

NAMES = [
    "storage", "files", "folder", "computer", "external-drive", "discovered",
    "free-space", "network-share", "nearby-computer", "nearby-server", "cloud-drive", "desktop",
]


# ---------------------------------------------------------------- helpers
def n(v):
    return f"{v:.2f}".rstrip("0").rstrip(".") if isinstance(v, float) else str(v)


def stops(items):
    out = ""
    for s in items:
        off, col, op = (s + (None,))[:3]
        out += f'<stop offset="{n(off)}" stop-color="{col}"' + (f' stop-opacity="{n(op)}"' if op is not None else "") + "/>"
    return out


def lg(id, x1, y1, x2, y2, *s):
    return f'<linearGradient id="{id}" x1="{n(x1)}" y1="{n(y1)}" x2="{n(x2)}" y2="{n(y2)}" gradientUnits="userSpaceOnUse">{stops(s)}</linearGradient>'


def rg(id, cx, cy, r, *s, units="userSpaceOnUse"):
    return f'<radialGradient id="{id}" cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" gradientUnits="{units}">{stops(s)}</radialGradient>'


def blur(id, sd):
    return (f'<filter id="{id}" filterUnits="userSpaceOnUse" x="-8" y="-8" width="80" height="80">'
            f'<feGaussianBlur stdDeviation="{n(sd)}"/></filter>')


def svg(defs, body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs>{defs}</defs>{body}</svg>\n'


def rect(x, y, w, h, r):
    """A rounded rectangle as path data (so it can be reused anywhere a path goes)."""
    return (f"M{n(x + r)} {n(y)}h{n(w - 2 * r)}a{n(r)} {n(r)} 0 0 1 {n(r)} {n(r)}v{n(h - 2 * r)}"
            f"a{n(r)} {n(r)} 0 0 1 {n(-r)} {n(r)}h{n(-(w - 2 * r))}a{n(r)} {n(r)} 0 0 1 {n(-r)} {n(-r)}"
            f"v{n(-(h - 2 * r))}a{n(r)} {n(r)} 0 0 1 {n(r)} {n(-r)}Z")


def page(x, y, w, h, fold=8, r=4):
    """A page with a folded top-right corner."""
    return (f"M{n(x + r)} {n(y)}H{n(x + w - fold)}L{n(x + w)} {n(y + fold)}V{n(y + h - r)}"
            f"a{n(r)} {n(r)} 0 0 1 {n(-r)} {n(r)}H{n(x + r)}a{n(r)} {n(r)} 0 0 1 {n(-r)} {n(-r)}"
            f"V{n(y + r)}a{n(r)} {n(r)} 0 0 1 {n(r)} {n(-r)}Z")


def fold(x, y, w, f=8):
    return f"M{n(x + w - f)} {n(y)}V{n(y + f - 2)}a2 2 0 0 0 2 2H{n(x + w)}Z"


def circle(cx, cy, r):
    return f"M{n(cx - r)} {n(cy)}a{n(r)} {n(r)} 0 1 0 {n(2 * r)} 0a{n(r)} {n(r)} 0 1 0 {n(-2 * r)} 0Z"


# Shared silhouettes (the current set's proportions).
FOLDER_BACK = "M7 18a5 5 0 0 1 5-5h12l5 5h22a5 5 0 0 1 5 5v24a5 5 0 0 1-5 5H12a5 5 0 0 1-5-5Z"
FOLDER_FRONT = "M7 26a5 5 0 0 1 5-5h39a5 5 0 0 1 5 5v20a6 6 0 0 1-6 6H13a6 6 0 0 1-6-6Z"
HEX = "M30 5a4 4 0 0 1 4 0l19 11a4 4 0 0 1 2 3.5v22a4 4 0 0 1-2 3.5L34 56a4 4 0 0 1-4 0L11 45a4 4 0 0 1-2-3.5v-22a4 4 0 0 1 2-3.5Z"
HEX_IN = "m32 13 15 8.5v17L32 47 17 38.5v-17Z"
CLOUD_FRONT = "M15 50a12 12 0 0 1-1.3-23.9A16.5 16.5 0 0 1 45 22.6 13.7 13.7 0 0 1 44 50Z"
CLOUD_BACK = "M27 40a9 9 0 0 1 1-17.9A12.5 12.5 0 0 1 51.6 21 9.6 9.6 0 0 1 50 40Z"
LAPTOP_BASE = "M10 43h44l5 6a2 2 0 0 1-2 3H7a2 2 0 0 1-2-3Z"
NEAR_BASE = "M8 47h40l4 5a2 2 0 0 1-2 3H6a2 2 0 0 1-2-3Z"
WIFI_ARCS = "M39.5 22.5a5 5 0 0 1 5 5M39.5 18.5a9 9 0 0 1 9 9"
SYNC_ARCS = "M41.6 44.4a5.6 5.6 0 0 1 10.2-1.6M52.4 47.6a5.6 5.6 0 0 1-10.2 1.6"
SYNC_HEADS = "M52.3 39.6v3.5h-3.5M41.7 52.4v-3.5h3.5"
LENS = "M33.5 36.5a6 6 0 0 1 7-2.5"
SCENE = "M14 35 26 23l9 9 4-4 9 9"

# Free-space gauge hooks (rewritten by art.rs).
ARC_GRAD = lg("rile-free-arc", 20, 34, 44, 34, (0, "#5fd39a"), (0.6, "#85c6fa"), (1, "#2e80cc"))
TRACK = "M23.5 40.5a12 12 0 1 1 17 0"
ARC = '<path d="M23.5 40.5a12 12 0 0 1 17-17" fill="none" stroke="url(#rile-free-arc)" stroke-width="{w}" stroke-linecap="round"/>'
NEEDLE = '<path d="M32 32 38.5 25.5" stroke="{c}" stroke-width="{w}" stroke-linecap="round"/>'

RACK_Y = (22, 35, 48)

# Desktop: a monitor on a stand (x 5-43) beside a tower (x 46-59). Screens reuse SCENE, scaled to fit.
DT_SCENE = "translate(10.5 15.3) scale(.8) translate(-14 -23)"


# ================================================================== GLASS
def glass():
    """Tinted glass: every part is a colored, translucent glass body with a top
    gloss and a bright rim; lighter frosted panes sit inside the darker ones."""
    base = ('<linearGradient id="gloss" x1="0" y1="0" x2="0" y2="1"><stop stop-color="#fff" stop-opacity=".55"/>'
            '<stop offset=".42" stop-color="#fff" stop-opacity=".1"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></linearGradient>'
            '<linearGradient id="rim" x1="0" y1="0" x2=".6" y2="1"><stop stop-color="#fff" stop-opacity=".95"/>'
            '<stop offset=".5" stop-color="#fff" stop-opacity=".18"/><stop offset="1" stop-color="#fff" stop-opacity=".55"/></linearGradient>'
            + blur("soft", 2.6))

    def tint(id, top, bottom, a=0.92, b=0.78):
        return (f'<linearGradient id="{id}" x1="0" y1="0" x2=".45" y2="1"><stop stop-color="{top}" stop-opacity="{n(a)}"/>'
                f'<stop offset="1" stop-color="{bottom}" stop-opacity="{n(b)}"/></linearGradient>')

    def body(d, id, shadow=None, extra="", rim=1.1):
        s = f'<g{extra}>'
        if shadow:
            s += f'<path d="{d}" fill="{shadow}" opacity=".45" filter="url(#soft)" transform="translate(0 2.5)"/>'
        s += (f'<path d="{d}" fill="url(#{id})"/><path d="{d}" fill="url(#gloss)"/>'
              f'<path d="{d}" fill="none" stroke="#1d2540" stroke-opacity=".22" stroke-width="{n(rim + 0.8)}"/>'
              f'<path d="{d}" fill="none" stroke="url(#rim)" stroke-width="{n(rim)}"/></g>')
        return s

    def frost(d, op=0.3):
        return (f'<path d="{d}" fill="#ffffff" fill-opacity="{n(op)}"/><path d="{d}" fill="url(#gloss)"/>'
                f'<path d="{d}" fill="none" stroke="#ffffff" stroke-opacity=".55" stroke-width=".8"/>')

    W = 'stroke="#ffffff" stroke-linecap="round" stroke-linejoin="round" fill="none"'
    BLUE = tint("blue", "#62d4ff", "#3f5bff")
    VIOLET = tint("violet", "#b79bff", "#6a3dff")
    PINK = tint("pink", "#ffb0d9", "#ff5f9e")
    CORAL = tint("coral", "#ffb07a", "#ff4f7b")
    TEAL = tint("teal", "#5ef0d2", "#2f7bff")
    MINT = tint("mint", "#7ff5c4", "#12a581", 1, 1)
    ICE = tint("ice", "#eaf4ff", "#9fc2ff", 0.85, 0.6)
    LILAC = tint("lilac", "#f3eeff", "#bba8ff", 0.88, 0.62)
    SLATE = tint("slate", "#c9d6ea", "#7d8fb0", 0.9, 0.75)
    ORANGE = tint("orange", "#ffd07a", "#ff6a5a", 1, 1)
    CYAN = tint("cyan", "#8af0ff", "#2f7bff", 1, 1)

    def badge(cx, cy, r, id):
        return body(circle(cx, cy, r), id, rim=1.2)

    wifi = f'<circle cx="39.5" cy="27.5" r="1.7" fill="#fff"/><path d="{WIFI_ARCS}" {W} stroke-width="1.8"/>'
    sync = f'<path d="{SYNC_ARCS}" {W} stroke-width="1.8"/><path d="{SYNC_HEADS}" {W} stroke-width="1.8"/>'
    mag = '<circle cx="44" cy="44" r="3" fill="none" stroke="#fff" stroke-width="1.8"/><path d="m46.3 46.3 2.5 2.5" stroke="#fff" stroke-width="1.8" stroke-linecap="round"/>'

    def laptop(lid, screen, base_d, scene_t):
        return (body(lid, "blue", "#3a4bff") + frost(screen, 0.22)
                + f'<path d="{SCENE}" transform="{scene_t}" {W} stroke-width="2"/>' + body(base_d, "slate", "#3d4b6b"))

    def rack(dx, dy):
        s = ""
        for y in RACK_Y:
            s += body(rect(8 + dx, y + dy, 40, 11, 3), "teal", "#1d5fd6" if y == RACK_Y[-1] else None)
            s += f'<path d="M{13 + dx} {y + dy + 5.5}h16" stroke="#ffffff" stroke-opacity=".75" stroke-dasharray="2 2" stroke-linecap="round"/>'
            s += f'<circle cx="{38 + dx}" cy="{y + dy + 5.5}" r="1.6" fill="#b6ffe4"/><circle cx="{42.5 + dx}" cy="{y + dy + 5.5}" r="1.6" fill="#ffffff"/>'
        return s

    out = {}
    out["folder"] = svg(base + BLUE + ICE, body(FOLDER_BACK, "blue", "#2a4bff") + body(FOLDER_FRONT, "ice")
        + '<path d="M13 46h14" stroke="#3f6fe0" stroke-opacity=".7" stroke-width="2" stroke-linecap="round"/>')

    out["files"] = svg(base + VIOLET + LILAC + ORANGE,
        body(page(13, 11, 26, 38), "violet", "#6a3dff", ' transform="rotate(-10 26 30)"')
        + body(page(22, 9, 26, 40), "lilac") + f'<path d="{fold(22, 9, 26)}" fill="#ffffff" fill-opacity=".7"/>'
        + '<path d="M28 23h13M28 28h13M28 33h8" stroke="#7b5cff" stroke-opacity=".75" stroke-width="2" stroke-linecap="round"/>'
        + badge(45, 45, 8.5, "orange") + mag)

    out["storage"] = svg(base + TEAL + rg("c", 32, 30, 8, (0, "#ffffff"), (0.55, "#d6fbff"), (1, "#7fe3ff", 0)),
        body(HEX, "teal", "#1d6bd6") + frost(HEX_IN, 0.18)
        + '<circle cx="32" cy="30" r="8" fill="url(#c)"/><circle cx="32" cy="30" r="3.2" fill="#ffffff"/>')

    out["computer"] = svg(base + BLUE + SLATE,
        laptop(rect(10, 9, 44, 34, 5), rect(13.5, 12.5, 37, 27, 2.5), LAPTOP_BASE, "translate(1 0)")
        + '<path d="M25 46h14" stroke="#ffffff" stroke-opacity=".75" stroke-width="1.6" stroke-linecap="round"/>')

    out["external-drive"] = svg(base + CORAL, body(rect(12, 10, 40, 45, 9), "coral", "#ff3d6e")
        + frost(rect(16, 14, 32, 28, 6), 0.2)
        + '<path d="M23 48h12" stroke="#ffffff" stroke-width="2.2" stroke-linecap="round"/><circle cx="41" cy="48" r="1.9" fill="#fff6c2"/>')

    out["discovered"] = svg(base + VIOLET + LILAC + CYAN,
        body(rect(9, 9, 30, 38, 5), "violet", "#5b3bff", ' transform="rotate(-9 24 28)"') + body(rect(14, 12, 30, 38, 5), "lilac")
        + '<path d="M20 21h18M20 27h14M20 33h9" stroke="#7b5cff" stroke-opacity=".75" stroke-width="2.2" stroke-linecap="round"/>'
        + '<path d="m47.5 47.5 6.5 6.5" stroke="#4b5a86" stroke-width="3.4" stroke-linecap="round"/>'
        + body(circle(40, 40, 9.5), "cyan", rim=1.3) + f'<path d="{LENS}" {W} stroke-width="2"/>')

    out["free-space"] = svg(base + VIOLET + ARC_GRAD, body(circle(32, 32, 24), "violet", "#4b2bff")
        + '<circle cx="32" cy="32" r="17.5" fill="#0d1730" fill-opacity=".82" stroke="#ffffff" stroke-opacity=".45"/>'
        + f'<path d="{TRACK}" fill="none" stroke="#ffffff" stroke-opacity=".16" stroke-width="5" stroke-linecap="round"/>'
        + ARC.format(w=5) + NEEDLE.format(c="#ffffff", w=2.2) + '<circle cx="32" cy="32" r="3" fill="#ffffff"/>')

    out["network-share"] = svg(base + TEAL, rack(4, -10))
    out["nearby-computer"] = svg(base + BLUE + SLATE + MINT,
        laptop(rect(8, 19, 40, 28, 4), rect(11.5, 22.5, 33, 21, 2), NEAR_BASE, "translate(-2 8)") + badge(43, 24, 10, "mint") + wifi)
    out["nearby-server"] = svg(base + TEAL + MINT, rack(0, 0) + badge(43, 24, 10, "mint") + wifi)
    out["cloud-drive"] = svg(base + VIOLET + ICE + CYAN, body(CLOUD_BACK, "violet", "#4b2bff") + body(CLOUD_FRONT, "ice", "#3d5bd6")
        + badge(47, 46, 10.5, "cyan") + sync)
    out["desktop"] = svg(base + BLUE + SLATE + MINT,
        body(rect(21.5, 36, 6, 8, 1.5), "slate") + body(rect(13, 43, 22, 5, 2.5), "slate", "#3d4b6b")
        + body(rect(5, 9, 38, 28, 4.5), "blue", "#3a4bff") + frost(rect(8.5, 12.5, 31, 21, 2.5), 0.22)
        + f'<path d="{SCENE}" transform="{DT_SCENE}" {W} stroke-width="2.4"/>'
        + body(rect(46, 12, 13, 41, 4), "slate", "#3d4b6b")
        + '<path d="M49.5 18h6" stroke="#ffffff" stroke-opacity=".8" stroke-width="2" stroke-linecap="round"/>'
        + '<path d="M49.5 23h6" stroke="#ffffff" stroke-opacity=".5" stroke-width="1.4" stroke-linecap="round"/>'
        + '<circle cx="52.5" cy="46" r="2" fill="#7ff5c4"/>')
    return out


# =================================================================== CLAY
def clay():
    """Soft matte 3D: chunky shapes with a thickness edge, top-left sheen and a ground shadow."""
    base = (rg("hl", 0.28, 0.12, 0.95, (0, "#ffffff", 0.55), (0.55, "#ffffff", 0), units="objectBoundingBox")
            + rg("sd", 0.9, 1.05, 0.85, (0, "#0b1430", 0.22), (1, "#0b1430", 0), units="objectBoundingBox")
            + blur("soft", 2.4))

    def blob(d, fill, edge, depth=2.6, extra=""):
        return (f'<g{extra}><path d="{d}" fill="{edge}" transform="translate(0 {depth})"/>'
                f'<path d="{d}" fill="{fill}"/><path d="{d}" fill="url(#hl)"/><path d="{d}" fill="url(#sd)"/></g>')

    def ground(cx, cy, rx, ry=3.2):
        return f'<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="#1b2340" opacity=".26" filter="url(#soft)"/>'

    def badge(cx, cy, r, fill, edge):
        return blob(circle(cx, cy, r), fill, edge, 2)

    out = {}
    out["folder"] = svg(base, ground(32, 56, 22) + blob(FOLDER_BACK, "#4a86ec", "#3363c4")
        + '<path d="M12 19h38" stroke="#fff" stroke-opacity=".8" stroke-width="3" stroke-linecap="round" transform="translate(0 2)"/>'
        + blob(FOLDER_FRONT, "#62a6ff", "#3d74d6", 3))

    out["files"] = svg(base, ground(32, 56, 20)
        + blob(page(13, 11, 26, 38), "#ffc867", "#e0a03e", 2.4, ' transform="rotate(-10 26 30)"')
        + blob(page(22, 9, 26, 40), "#fff7e8", "#e2cfae", 2.6) + f'<path d="{fold(22, 9, 26)}" fill="#f0dcb8"/>'
        + '<path d="M28 23h13M28 28h13M28 33h8" stroke="#e7a84b" stroke-width="2.6" stroke-linecap="round"/>'
        + badge(45, 45, 8.5, "#ff8a7a", "#dd6352")
        + '<circle cx="44" cy="44" r="3" fill="none" stroke="#fff" stroke-width="2"/><path d="m46.4 46.4 2.4 2.4" stroke="#fff" stroke-width="2" stroke-linecap="round"/>')

    out["storage"] = svg(base, ground(32, 57, 20) + blob(HEX, "#4fd1c5", "#2ea398", 3)
        + f'<path d="{HEX_IN}" fill="#2c3d6b"/><path d="{HEX_IN}" fill="url(#sd)"/>'
        + blob(circle(32, 30, 7), "#ffd76a", "#d9a93a", 1.6) + '<circle cx="29.8" cy="27.6" r="1.8" fill="#fff" opacity=".9"/>')

    out["computer"] = svg(base, ground(32, 57, 25)
        + blob(rect(10, 8, 44, 35, 6), "#3d4d70", "#2a3653", 2)
        + f'<path d="{rect(14, 12, 36, 27, 3)}" fill="#7cc4ff"/><path d="{rect(14, 12, 36, 27, 3)}" fill="url(#hl)"/>'
        + f'<path d="{SCENE}" transform="translate(1 0)" fill="none" stroke="#ffffff" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>'
        + blob(LAPTOP_BASE, "#d3dbe8", "#a3afc4", 2.4) + '<path d="M25 46h14" stroke="#9aa7bd" stroke-width="2" stroke-linecap="round"/>')

    out["external-drive"] = svg(base, ground(32, 59, 18) + blob(rect(13, 9, 38, 45, 10), "#ff8f7a", "#dc6553", 3)
        + '<path d="M21 18h22" stroke="#fff" stroke-opacity=".7" stroke-width="2.4" stroke-linecap="round"/>'
        + '<path d="M24 45h12" stroke="#c45444" stroke-width="3" stroke-linecap="round"/><circle cx="42" cy="45" r="2.2" fill="#fff4b0"/>')

    out["discovered"] = svg(base, ground(30, 56, 20)
        + blob(rect(8, 9, 30, 38, 6), "#b69cff", "#8b6fe6", 2.4, ' transform="rotate(-9 23 28)"')
        + blob(rect(14, 12, 30, 38, 6), "#f6f2ff", "#d2c8f2", 2.6)
        + '<path d="M20 21h18M20 27h14M20 33h9" stroke="#a48cf2" stroke-width="2.6" stroke-linecap="round"/>'
        + '<path d="m47 47 6.5 6.5" stroke="#5d6b8f" stroke-width="4.4" stroke-linecap="round"/>'
        + blob(circle(40, 40, 9.5), "#6fd0ff", "#3b9fdc", 2) + f'<path d="{LENS}" fill="none" stroke="#fff" stroke-width="2.2" stroke-linecap="round"/>')

    out["free-space"] = svg(base + ARC_GRAD, ground(32, 59, 20)
        + blob(circle(32, 32, 24), "#e7ebf3", "#b5bed1", 3)
        + '<circle cx="32" cy="32" r="17.5" fill="#2c3d6b"/><circle cx="32" cy="32" r="17.5" fill="url(#sd)"/>'
        + f'<path d="{TRACK}" fill="none" stroke="#45588a" stroke-width="5.4" stroke-linecap="round"/>'
        + ARC.format(w=5.4) + NEEDLE.format(c="#ffffff", w=2.6) + '<circle cx="32" cy="32" r="3.4" fill="#ffd76a"/>')

    def rack(dx, dy):
        s = ""
        for y in RACK_Y:
            s += blob(rect(8 + dx, y + dy, 40, 11, 4), "#5d7092", "#43536f", 2)
            s += f'<path d="M{13 + dx} {y + dy + 5.5}h15" stroke="#91a5c8" stroke-width="2" stroke-linecap="round" stroke-dasharray="1.5 3.2"/>'
            s += f'<circle cx="{37.5 + dx}" cy="{y + dy + 5.5}" r="1.9" fill="#7ef0c0"/><circle cx="{42.5 + dx}" cy="{y + dy + 5.5}" r="1.9" fill="#ffd76a"/>'
        return s

    out["network-share"] = svg(base, ground(32, 55, 21) + rack(4, -10))

    wifi = f'<circle cx="39.5" cy="27.5" r="1.8" fill="#fff"/><path d="{WIFI_ARCS}" fill="none" stroke="#fff" stroke-width="2.1" stroke-linecap="round"/>'
    out["nearby-computer"] = svg(base, ground(28, 59, 23)
        + blob(rect(8, 19, 40, 29, 5), "#3d4d70", "#2a3653", 2)
        + f'<path d="{rect(11.5, 22.5, 33, 22, 2.5)}" fill="#7cc4ff"/><path d="{rect(11.5, 22.5, 33, 22, 2.5)}" fill="url(#hl)"/>'
        + f'<path d="{SCENE}" transform="translate(-2 9)" fill="none" stroke="#fff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>'
        + blob(NEAR_BASE, "#d3dbe8", "#a3afc4", 2.2) + badge(43, 24, 10, "#4fd8a6", "#2aa97c") + wifi)

    out["nearby-server"] = svg(base, ground(28, 62, 21) + rack(0, 0) + badge(43, 24, 10, "#4fd8a6", "#2aa97c") + wifi)

    out["cloud-drive"] = svg(base, ground(32, 57, 22)
        + blob(CLOUD_BACK, "#8fb6ff", "#6690e3", 2.4) + blob(CLOUD_FRONT, "#eef5ff", "#c3d3ef", 3)
        + badge(47, 46, 10.5, "#5b9dff", "#3a74d6")
        + f'<path d="{SYNC_ARCS}" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round"/><path d="{SYNC_HEADS}" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>')
    out["desktop"] = svg(base, ground(32, 56, 27)
        + blob(rect(21.5, 36, 6, 8, 1.5), "#d3dbe8", "#a3afc4", 1.6) + blob(rect(13, 43, 22, 5, 2.5), "#d3dbe8", "#a3afc4", 2)
        + blob(rect(5, 9, 38, 28, 6), "#3d4d70", "#2a3653", 2)
        + f'<path d="{rect(8.5, 12.5, 31, 21, 3)}" fill="#7cc4ff"/><path d="{rect(8.5, 12.5, 31, 21, 3)}" fill="url(#hl)"/>'
        + f'<path d="{SCENE}" transform="{DT_SCENE}" fill="none" stroke="#ffffff" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>'
        + blob(rect(46, 12, 13, 41, 5), "#d3dbe8", "#a3afc4", 2.4)
        + '<path d="M49.5 18h6" stroke="#9aa7bd" stroke-width="2.4" stroke-linecap="round"/><circle cx="52.5" cy="46" r="2.2" fill="#4fd8a6"/>')
    return out


# =================================================================== LINE
def line():
    """Hierarchical outline: one confident stroke color per symbol over a soft tint of itself."""
    def L(d, c, op=0.16, w=2.6, extra=""):
        return f'<path d="{d}" fill="{c}" fill-opacity="{op}" stroke="{c}" stroke-width="{w}" stroke-linejoin="round" stroke-linecap="round"{extra}/>'

    def S(d, c, w=2.6, op=1, extra=""):
        return f'<path d="{d}" fill="none" stroke="{c}" stroke-opacity="{n(op)}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"{extra}/>'

    out = {}
    B = "#3b82f6"
    out["folder"] = svg("", L(FOLDER_BACK, B, 0.12) + L(FOLDER_FRONT, B, 0.28) + S("M14 46h12", B))
    V = "#6366f1"
    out["files"] = svg("", L(page(13, 11, 26, 38), V, 0.1, extra=' transform="rotate(-10 26 30)"')
        + L(page(22, 9, 26, 40), V, 0.24) + S("M40 9v6a2 2 0 0 0 2 2h6", V)
        + S("M28 24h13M28 29h13M28 34h7", V, 2.4, 0.6) + L(circle(45, 45, 8.5), V, 1) + S("M46.4 46.4l2.6 2.6", "#fff", 2) + '<circle cx="44" cy="44" r="3" fill="none" stroke="#fff" stroke-width="2"/>')
    T = "#14b8a6"
    out["storage"] = svg("", L(HEX, T, 0.12) + L(HEX_IN, T, 0.24, 2.2) + L(circle(32, 30, 6.5), T, 1, 2.2) + '<circle cx="32" cy="30" r="2.2" fill="#fff"/>')
    C = "#0ea5e9"
    out["computer"] = svg("", L(rect(10, 9, 44, 33, 5), C, 0.12) + L(rect(14.5, 13.5, 35, 24, 2), C, 0.22, 1.8)
        + S(SCENE, C, 2.4, 0.7, ' transform="translate(1 -1)"') + L(LAPTOP_BASE, C, 0.28) + S("M26 46.5h12", C, 2.2, 0.6))
    O = "#f97316"
    out["external-drive"] = svg("", L(rect(13, 9, 38, 46, 9), O, 0.14) + S("M20 18h24", O, 2.2, 0.5) + S("M22 46h14", O, 2.8) + '<circle cx="42" cy="46" r="2" fill="#f97316"/>')
    P = "#8b5cf6"
    out["discovered"] = svg("", L(rect(9, 9, 30, 38, 5), P, 0.1, extra=' transform="rotate(-9 24 28)"') + L(rect(14, 12, 30, 38, 5), P, 0.22)
        + S("M20 21h18M20 27h14M20 33h9", P, 2.4, 0.6) + S("m47.5 47.5 6.5 6.5", P, 3.6) + L(circle(40, 40, 9.5), P, 0.35)
        + S(LENS, "#ffffff", 2) )
    G = "#64748b"
    out["free-space"] = svg(ARC_GRAD, L(circle(32, 32, 24), G, 0.1) + f'<path d="{TRACK}" fill="none" stroke="{G}" stroke-opacity=".28" stroke-width="5" stroke-linecap="round"/>'
        + ARC.format(w=5) + NEEDLE.format(c=G, w=2.6) + f'<circle cx="32" cy="32" r="3.2" fill="{G}"/>')
    R = "#64748b"

    def rack(dx, dy):
        s = ""
        for y in RACK_Y:
            s += L(rect(8 + dx, y + dy, 40, 11, 3), R, 0.14, 2.2)
            s += S(f"M{13 + dx} {y + dy + 5.5}h14", R, 2, 0.45, ' stroke-dasharray=".5 3.5"')
            s += f'<circle cx="{37.5 + dx}" cy="{y + dy + 5.5}" r="1.7" fill="#10b981"/><circle cx="{42.5 + dx}" cy="{y + dy + 5.5}" r="1.7" fill="#0ea5e9"/>'
        return s

    out["network-share"] = svg("", rack(4, -10))
    M = "#10b981"
    badge = L(circle(43, 24, 10), M, 1, 2) + f'<circle cx="39.5" cy="27.5" r="1.7" fill="#fff"/>' + S(WIFI_ARCS, "#fff", 1.9)
    halo = '<circle cx="43" cy="24" r="12.4" fill="#ffffff" fill-opacity="0"/>'
    out["nearby-computer"] = svg("", L(rect(8, 19, 40, 28, 4), C, 0.12) + L(rect(12, 23, 32, 20, 2), C, 0.2, 1.8)
        + S(SCENE, C, 2.2, 0.7, ' transform="translate(-2 8)"') + L(NEAR_BASE, C, 0.28) + halo + badge)
    out["nearby-server"] = svg("", rack(0, 0) + badge)
    out["cloud-drive"] = svg("", L(CLOUD_BACK, B, 0.12) + L(CLOUD_FRONT, B, 0.24) + L(circle(47, 46, 10.5), B, 1, 2)
        + S(SYNC_ARCS, "#fff", 1.9) + S(SYNC_HEADS, "#fff", 1.9))
    out["desktop"] = svg("", L(rect(21.5, 36, 6, 8, 1.5), C, 0.2, 2) + L(rect(13, 43, 22, 5, 2.5), C, 0.28)
        + L(rect(5, 9, 38, 28, 5), C, 0.12) + L(rect(9.5, 13.5, 29, 19, 2), C, 0.22, 1.8)
        + S(SCENE, C, 2.2, 0.7, f' transform="{DT_SCENE}"') + L(rect(46, 12, 13, 41, 4), C, 0.18)
        + S("M49.5 18.5h6", C, 2.2, 0.6) + f'<circle cx="52.5" cy="46" r="2" fill="{M}"/>')
    return out


# =================================================================== NEON
def neon():
    """Dark glass bodies outlined in glowing neon tubes."""
    base = blur("glow", 1.7) + lg("body", 10, 6, 54, 58, (0, "#1a2340"), (1, "#0b1020"))

    def N(d, c, w=1.7, fill=True, extra=""):
        body = f'<path d="{d}" fill="url(#body)" fill-opacity=".94"{extra}/>' if fill else ""
        return (body + f'<path d="{d}" fill="none" stroke="{c}" stroke-width="{n(w + 1.4)}" stroke-linejoin="round" stroke-linecap="round" filter="url(#glow)" opacity=".95"{extra}/>'
                + f'<path d="{d}" fill="none" stroke="{c}" stroke-width="{w}" stroke-linejoin="round" stroke-linecap="round"{extra}/>'
                + f'<path d="{d}" fill="none" stroke="#ffffff" stroke-opacity=".55" stroke-width=".5" stroke-linejoin="round" stroke-linecap="round"{extra}/>')

    def dot(cx, cy, r, c):
        return f'<circle cx="{cx}" cy="{cy}" r="{n(r + 1.2)}" fill="{c}" filter="url(#glow)"/><circle cx="{cx}" cy="{cy}" r="{r}" fill="{c}"/><circle cx="{cx}" cy="{cy}" r="{n(r * 0.45)}" fill="#fff" opacity=".8"/>'

    CY, MG, LM, OR, VI = "#22e4ff", "#ff3df2", "#9dff5a", "#ffb02e", "#a46bff"
    out = {}
    out["folder"] = svg(base, N(FOLDER_BACK, CY) + N(FOLDER_FRONT, CY) + N("M14 45h12", MG, 2, False))
    out["files"] = svg(base, N(page(13, 11, 26, 38), VI, extra=' transform="rotate(-10 26 30)"') + N(page(22, 9, 26, 40), MG)
        + N("M28 24h13M28 29h13M28 34h7", MG, 1.6, False) + N(circle(45, 45, 8), CY) + N("M43.8 43.8m-2.8 0a2.8 2.8 0 1 0 5.6 0a2.8 2.8 0 1 0-5.6 0M46 46l2.6 2.6", CY, 1.5, False))
    out["storage"] = svg(base, N(HEX, CY) + N(HEX_IN, VI, 1.4) + N(circle(32, 30, 6), MG) + dot(32, 30, 2.2, MG))
    out["computer"] = svg(base, N(rect(10, 9, 44, 33, 5), CY) + N(rect(14, 13, 36, 25, 2), VI, 1.2)
        + N(SCENE, MG, 1.6, False, ' transform="translate(1 -1)"') + N(LAPTOP_BASE, CY) + N("M26 46.5h12", CY, 1.4, False))
    out["external-drive"] = svg(base, N(rect(13, 9, 38, 46, 9), OR) + N("M20 18h24", OR, 1.2, False) + N("M22 46h13", OR, 2, False) + dot(42, 46, 1.8, LM))
    out["discovered"] = svg(base, N(rect(9, 9, 30, 38, 5), VI, extra=' transform="rotate(-9 24 28)"') + N(rect(14, 12, 30, 38, 5), MG)
        + N("M20 21h18M20 27h14M20 33h9", MG, 1.6, False) + N("m47.5 47.5 6 6", CY, 2.6, False) + N(circle(40, 40, 9.5), CY) + N(LENS, "#ffffff", 1.3, False))
    out["free-space"] = svg(base + ARC_GRAD, N(circle(32, 32, 24), VI) + N(circle(32, 32, 17), CY, 1)
        + f'<path d="{TRACK}" fill="none" stroke="#2a3558" stroke-width="5" stroke-linecap="round"/>'
        + ARC.replace("/>", ' filter="url(#glow)"/>').format(w=6) + ARC.format(w=4.2)
        + NEEDLE.format(c="#ffffff", w=2) + dot(32, 32, 2.4, MG))

    def rack(dx, dy):
        s = ""
        for y in RACK_Y:
            s += N(rect(8 + dx, y + dy, 40, 11, 3), CY, 1.5)
            s += f'<path d="M{13 + dx} {y + dy + 5.5}h15" stroke="{VI}" stroke-width="1.4" stroke-dasharray="2 2" stroke-linecap="round"/>'
            s += dot(37.5 + dx, y + dy + 5.5, 1.4, LM) + dot(42.5 + dx, y + dy + 5.5, 1.4, MG)
        return s

    out["network-share"] = svg(base, rack(4, -10))
    badge = N(circle(43, 24, 10), LM) + dot(39.5, 27.5, 1.5, LM) + N(WIFI_ARCS, LM, 1.6, False)
    out["nearby-computer"] = svg(base, N(rect(8, 19, 40, 28, 4), CY) + N(rect(12, 23, 32, 20, 2), VI, 1.2)
        + N(SCENE, MG, 1.5, False, ' transform="translate(-2 8)"') + N(NEAR_BASE, CY) + badge)
    out["nearby-server"] = svg(base, rack(0, 0) + badge)
    out["cloud-drive"] = svg(base, N(CLOUD_BACK, VI) + N(CLOUD_FRONT, CY) + N(circle(47, 46, 10.5), MG)
        + N(SYNC_ARCS, MG, 1.6, False) + N(SYNC_HEADS, MG, 1.6, False))
    out["desktop"] = svg(base, N(rect(21.5, 36, 6, 8, 1.5), CY, 1.2) + N(rect(13, 43, 22, 5, 2.5), CY, 1.4)
        + N(rect(5, 9, 38, 28, 5), CY) + N(rect(9, 13, 30, 20, 2), VI, 1.2)
        + N(SCENE, MG, 1.5, False, f' transform="{DT_SCENE}"') + N(rect(46, 12, 13, 41, 4), CY)
        + N("M49.5 18.5h6", VI, 1.4, False) + dot(52.5, 46, 1.7, LM))
    return out


# ============================================================== ISOMETRIC
def iso():
    """Isometric objects: crisp three-face shading, like little desk models."""
    C30, S30 = 0.866, 0.5

    def P(ox, oy, x, y, z):
        return (ox + (x - y) * C30, oy + (x + y) * S30 - z)

    def poly(pts, fill, extra=""):
        return f'<path d="M{" L".join(f"{n(round(a, 2))} {n(round(b, 2))}" for a, b in pts)}Z" fill="{fill}"{extra}/>'

    def box(o, x, y, z, w, d, h, top, left, right, edge=None):
        ox, oy = o
        t = [P(ox, oy, x, y, z + h), P(ox, oy, x + w, y, z + h), P(ox, oy, x + w, y + d, z + h), P(ox, oy, x, y + d, z + h)]
        lf = [P(ox, oy, x, y + d, z), P(ox, oy, x + w, y + d, z), P(ox, oy, x + w, y + d, z + h), P(ox, oy, x, y + d, z + h)]
        rt = [P(ox, oy, x + w, y, z), P(ox, oy, x + w, y + d, z), P(ox, oy, x + w, y + d, z + h), P(ox, oy, x + w, y, z + h)]
        s = poly(lf, left) + poly(rt, right) + poly(t, top)
        if edge:
            a, b, c = P(ox, oy, x, y + d, z + h), P(ox, oy, x + w, y + d, z + h), P(ox, oy, x + w, y, z + h)
            s += f'<path d="M{n(round(a[0], 2))} {n(round(a[1], 2))}L{n(round(b[0], 2))} {n(round(b[1], 2))}L{n(round(c[0], 2))} {n(round(c[1], 2))}" fill="none" stroke="{edge}" stroke-width=".8" stroke-linejoin="round"/>'
        return s

    def face_left(o, x, y, z, w, h, fill, extra=""):
        """A rectangle drawn on a front-left face (plane y = const)."""
        ox, oy = o
        return poly([P(ox, oy, x, y, z), P(ox, oy, x + w, y, z), P(ox, oy, x + w, y, z + h), P(ox, oy, x, y, z + h)], fill, extra)

    def face_top(o, x, y, z, w, d, fill, extra=""):
        ox, oy = o
        return poly([P(ox, oy, x, y, z), P(ox, oy, x + w, y, z), P(ox, oy, x + w, y + d, z), P(ox, oy, x, y + d, z)], fill, extra)

    def line_left(o, x1, x2, y, z, c, w=1.2, extra=""):
        ox, oy = o
        a, b = P(ox, oy, x1, y, z), P(ox, oy, x2, y, z)
        return f'<path d="M{n(round(a[0], 2))} {n(round(a[1], 2))}L{n(round(b[0], 2))} {n(round(b[1], 2))}" stroke="{c}" stroke-width="{w}" stroke-linecap="round"{extra}/>'

    def line_top(o, x1, y1, x2, y2, z, c, w=1.4):
        ox, oy = o
        a, b = P(ox, oy, x1, y1, z), P(ox, oy, x2, y2, z)
        return f'<path d="M{n(round(a[0], 2))} {n(round(a[1], 2))}L{n(round(b[0], 2))} {n(round(b[1], 2))}" stroke="{c}" stroke-width="{w}" stroke-linecap="round"/>'

    def top_ellipse(o, x, y, z, r, fill, extra=""):
        cx, cy = P(o[0], o[1], x, y, z)
        return f'<ellipse cx="{n(round(cx, 2))}" cy="{n(round(cy, 2))}" rx="{n(round(r * 1.2247, 2))}" ry="{n(round(r * 0.7071, 2))}" fill="{fill}"{extra}/>'

    def shadow(o, x, y, w, d):
        return face_top(o, x + 1.5, y + 1.5, 0, w, d, "#0b1430", ' opacity=".16"')

    out = {}
    defs = rg("glow", 0.5, 0.5, 0.5, (0, "#ffffff"), (0.4, "#c9f2ff"), (1, "#5cc8ff", 0), units="objectBoundingBox")

    # folder: an upright folder with a tab and a sheet peeking out
    o = (32, 44)
    out["folder"] = svg("", shadow(o, -16, -3, 32, 8)
        + box(o, -16, -3, 0, 32, 3, 25, "#9bd2ff", "#3d8ff0", "#2a6fcf")
        + box(o, -16, -3, 25, 11, 3, 3, "#9bd2ff", "#3d8ff0", "#2a6fcf")
        + box(o, -13, 0, 2, 26, 1.2, 25, "#ffffff", "#eef3fb", "#cfd9e8")
        + box(o, -16, 1.2, 0, 32, 3.6, 21, "#a8dbff", "#5aa9ff", "#3a86e6", "#d4ecff")
        + line_left(o, -12, -2, 4.8, 4.5, "#d4ecff", 1.6))

    # storage: three stacked platters with a glowing core on top
    o = (32, 40)
    s = shadow(o, -13, -13, 26, 26)
    for i, (t, l, r) in enumerate([("#6fe0d3", "#2fb3a6", "#238c82")] * 3):
        s += box(o, -13, -13, i * 8, 26, 26, 6, t, l, r, "#c9fff7" if i == 2 else None)
        s += line_left(o, -9, -3, 13, i * 8 + 3, "#e9fffb", 1.3, ' opacity=".8"')
        cx, cy = P(o[0], o[1], 9, 13, i * 8 + 3)
        s += f'<circle cx="{n(round(cx, 2))}" cy="{n(round(cy, 2))}" r="1.1" fill="#ffe27a"/>'
    s += top_ellipse(o, 0, 0, 22, 7.5, "#1f4d63") + top_ellipse(o, 0, 0, 22, 6, "url(#glow)")
    out["storage"] = svg(defs, s)

    # computer: an open laptop
    o = (32, 38)
    s = shadow(o, -15, -10, 30, 22)
    s += box(o, -15, -10, 0, 30, 22, 2.5, "#dfe6f0", "#a7b3c5", "#8592a6")
    s += face_top(o, -12, -3, 2.5, 24, 10, "#b8c3d4") + face_top(o, -5, 8, 2.5, 10, 3, "#c9d2df")
    s += box(o, -15, -12, 0.5, 30, 2, 21, "#56677f", "#2c3a52", "#22304a")
    s += face_left(o, -13, -10, 3, 26, 17, "#4fa3ec")
    ox, oy = o
    a, b, c = P(ox, oy, -11, -10, 6), P(ox, oy, -3, -10, 14), P(ox, oy, 3, -10, 9)
    d = P(ox, oy, 11, -10, 15)
    s += f'<path d="M{n(round(a[0], 2))} {n(round(a[1], 2))}L{n(round(b[0], 2))} {n(round(b[1], 2))}L{n(round(c[0], 2))} {n(round(c[1], 2))}L{n(round(d[0], 2))} {n(round(d[1], 2))}" fill="none" stroke="#d6efff" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round"/>'
    out["computer"] = svg("", s)

    # external drive: an upright enclosure
    o = (32, 47)
    out["external-drive"] = svg("", shadow(o, -11, -5, 22, 10)
        + box(o, -11, -5, 0, 22, 10, 32, "#ffb3a3", "#ff7a63", "#e05a45", "#ffd9cf")
        + line_left(o, -7, 3, 5, 6, "#b9443a", 2) + face_left(o, 6, 5, 5, 2.4, 2.4, "#fff3a8")
        + line_left(o, -7, 7, 5, 27, "#ffd2c7", 1, ' opacity=".8"'))

    # files: a stack of sheets
    o = (30, 38)
    s = shadow(o, -12, -14, 24, 30)
    for i, (t, l, r) in enumerate([("#c9d6ff", "#97a8e6", "#7f90cf"), ("#e1e9ff", "#a9b8ec", "#8d9dd8"), ("#ffffff", "#c3cdea", "#a6b2d6")]):
        s += box(o, -12 + i * 1.5, -14 - i * 1.5, i * 4, 24, 30, 1.6, t, l, r)
    for k, ln in enumerate((16, 16, 10)):
        s += line_top(o, -6, -10 + k * 5, -6 + ln, -10 + k * 5, 9.6, "#7c8fd6", 1.6)
    s += '<circle cx="46" cy="46" r="8.5" fill="#ff8a6a"/><circle cx="46" cy="46" r="8.5" fill="none" stroke="#e0664a" stroke-width="1.2"/>'
    s += '<circle cx="45" cy="45" r="3" fill="none" stroke="#fff" stroke-width="1.8"/><path d="m47.3 47.3 2.5 2.5" stroke="#fff" stroke-width="1.8" stroke-linecap="round"/>'
    out["files"] = svg("", s)

    # discovered: a card lying flat with a magnifier hovering above it
    o = (28, 36)
    s = shadow(o, -12, -15, 24, 30)
    s += box(o, -12, -15, 0, 24, 30, 2, "#f1ecff", "#b9a6f5", "#9d88e6")
    for k, ln in enumerate((16, 12, 8)):
        s += line_top(o, -7, -9 + k * 5, -7 + ln, -9 + k * 5, 2, "#9c84f0", 1.8)
    s += '<path d="m45 45 7 7" stroke="#47566f" stroke-width="4.4" stroke-linecap="round"/>'
    s += '<circle cx="40" cy="40" r="10" fill="#2f7fd0"/><circle cx="40" cy="38.6" r="9.2" fill="#7fd8ff"/><path d="M34.6 36.2a6 6 0 0 1 6.5-3.3" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round"/>'
    out["discovered"] = svg("", s)

    # free space: a gauge puck with depth
    s = ''.join(f'<circle cx="32" cy="{n(32 + k * 0.5)}" r="24" fill="#3a4a63"/>' for k in range(1, 9))
    s += '<circle cx="32" cy="32" r="24" fill="#e4e9f1"/><circle cx="32" cy="32" r="18" fill="#1b2a42"/><path d="M14 32a18 18 0 0 1 36 0" fill="none" stroke="#2c3e5c" stroke-width="1"/>'
    s += f'<path d="{TRACK}" fill="none" stroke="#3c4f6f" stroke-width="5" stroke-linecap="round"/>' + ARC.format(w=5)
    s += NEEDLE.format(c="#ffffff", w=2.4) + '<circle cx="32" cy="32" r="3" fill="#ffffff"/>'
    out["free-space"] = svg(ARC_GRAD, s)

    def rack(o):
        s = shadow(o, -15, -10, 30, 20)
        for i in range(3):
            z = i * 9
            s += box(o, -15, -10, z, 30, 20, 7, "#8aa0bb", "#4c5f78", "#3a4a61", "#a9bbd2")
            for k in range(4):
                s += line_left(o, -12 + k * 3, -12 + k * 3 + 1.2, 10, z + 3.5, "#93a8c2", 1.3)
            ox, oy = o
            for x, c in ((5, "#7ef0c0"), (9, "#98d8ff")):
                cx, cy = P(ox, oy, x, 10, z + 3.5)
                s += f'<circle cx="{n(round(cx, 2))}" cy="{n(round(cy, 2))}" r="1.3" fill="{c}"/>'
        return s

    out["network-share"] = svg("", rack((32, 42)))
    badge = ('<circle cx="45" cy="19" r="10" fill="#1f9d75"/><circle cx="45" cy="17.6" r="9.4" fill="#4fd8a6"/>'
             '<g transform="translate(2 -6.5)"><circle cx="39.5" cy="27.5" r="1.6" fill="#fff"/>'
             f'<path d="{WIFI_ARCS}" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round"/></g>')

    o = (28, 42)
    s = shadow(o, -13, -9, 26, 19)
    s += box(o, -13, -9, 0, 26, 19, 2.2, "#dfe6f0", "#a7b3c5", "#8592a6")
    s += face_top(o, -10, -3, 2.2, 20, 8, "#b8c3d4")
    s += box(o, -13, -11, 0.4, 26, 2, 18, "#56677f", "#2c3a52", "#22304a")
    s += face_left(o, -11, -9, 2.6, 22, 14, "#4fa3ec")
    out["nearby-computer"] = svg("", s + badge)
    out["nearby-server"] = svg("", rack((28, 46)) + badge)

    s = ''.join(f'<path d="{CLOUD_FRONT}" fill="#5f86c9" transform="translate({n(k * 0.35)} {n(k * 0.45)})"/>' for k in range(1, 9))
    s += f'<path d="{CLOUD_FRONT}" fill="#eaf3ff"/><path d="M15 50a12 12 0 0 1-1.3-23.9" fill="none" stroke="#ffffff" stroke-width="1.4"/>'
    s += '<circle cx="47" cy="47.6" r="10.5" fill="#2556a8"/><circle cx="47" cy="46" r="10.5" fill="#4d8dff"/>'
    s += f'<path d="{SYNC_ARCS}" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round"/><path d="{SYNC_HEADS}" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>'
    out["cloud-drive"] = svg("", s)
    # desktop: a monitor on a stand beside a tower
    o = (30, 45)
    s = shadow(o, -17, -3, 36, 15)
    s += box(o, -12, -1, 0, 12, 8, 1.6, "#dfe6f0", "#a7b3c5", "#8592a6")
    s += box(o, -7, 2, 1.6, 3, 2, 6, "#c9d2df", "#8d9aae", "#75839a")
    s += box(o, -18, 1, 7, 22, 2.4, 17, "#56677f", "#2c3a52", "#22304a")
    s += face_left(o, -16.4, 3.4, 8.6, 19, 13.4, "#4fa3ec")
    ox, oy = o
    a, b, c2, d2 = P(ox, oy, -14, 3.4, 11), P(ox, oy, -8, 3.4, 17), P(ox, oy, -4, 3.4, 13), P(ox, oy, 0, 3.4, 18)
    s += f'<path d="M{n(round(a[0], 2))} {n(round(a[1], 2))}L{n(round(b[0], 2))} {n(round(b[1], 2))}L{n(round(c2[0], 2))} {n(round(c2[1], 2))}L{n(round(d2[0], 2))} {n(round(d2[1], 2))}" fill="none" stroke="#d6efff" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round"/>'
    s += box(o, 7, -3, 0, 9, 12, 27, "#dfe6f0", "#a7b3c5", "#8592a6", "#f4f7fb")
    s += line_left(o, 9, 14, 9, 21, "#6b7a90", 1.8)
    cx, cy = P(ox, oy, 12.5, 9, 6)
    s += f'<circle cx="{n(round(cx, 2))}" cy="{n(round(cy, 2))}" r="1.4" fill="#4fd8a6"/>'
    out["desktop"] = svg("", s)
    return out


# ================================================================== TILT
def tilt():
    """Front-facing 3D, camera slightly above: every object shows its front
    face plus a foreshortened top face receding toward a vanishing point high
    above the artboard (32, -70)."""
    VX, VY = 32, -70

    def recede(s):
        return f"matrix({n(round(s, 4))} 0 0 {n(round(s, 4))} {n(round(VX * (1 - s), 3))} {n(round(VY * (1 - s), 3))})"

    base = (blur("soft", 2.4)
            + '<linearGradient id="edge" x1="0" y1="0" x2="0" y2="1"><stop stop-color="#fff" stop-opacity=".75"/>'
            '<stop offset=".18" stop-color="#fff" stop-opacity="0"/></linearGradient>'
            '<linearGradient id="sheen" x1="0" y1="0" x2="1" y2="1"><stop stop-color="#fff" stop-opacity=".28"/>'
            '<stop offset=".5" stop-color="#fff" stop-opacity="0"/></linearGradient>')

    def grad(id, top, bottom):
        return f'<linearGradient id="{id}" x1="0" y1="0" x2="0" y2="1"><stop stop-color="{top}"/><stop offset="1" stop-color="{bottom}"/></linearGradient>'

    def solid(d, front, top, depth=0.05, steps=7, extra="", sheen=True):
        """Extrude d away from the camera: top bands first, then the front face."""
        s = f"<g{extra}>"
        for i in range(steps, 0, -1):
            s += f'<path d="{d}" fill="url(#{top})" transform="{recede(1 - depth * i / steps)}"/>'
        s += f'<path d="{d}" fill="url(#{front})"/>'
        if sheen:
            s += f'<path d="{d}" fill="url(#sheen)"/>'
        s += f'<path d="{d}" fill="none" stroke="url(#edge)" stroke-width="1"/></g>'
        return s

    def ground(cx, cy, rx, ry=3):
        return f'<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="#0b1226" opacity=".32" filter="url(#soft)"/>'

    def cylinder(cx, top_y, rx, h, side, cap, ry_k=0.3):
        ry = rx * ry_k
        d = f"M{n(cx - rx)} {n(top_y)}v{n(h)}a{n(rx)} {n(round(ry, 2))} 0 0 0 {n(2 * rx)} 0v{n(-h)}Z"
        return (f'<path d="{d}" fill="url(#{side})"/><path d="{d}" fill="url(#sheen)"/>'
                f'<ellipse cx="{cx}" cy="{top_y}" rx="{rx}" ry="{n(round(ry, 2))}" fill="url(#{cap})"/>'
                f'<ellipse cx="{cx}" cy="{top_y}" rx="{rx}" ry="{n(round(ry, 2))}" fill="none" stroke="#fff" stroke-opacity=".55" stroke-width=".8"/>')

    W = 'stroke="#ffffff" stroke-linecap="round" stroke-linejoin="round" fill="none"'
    G = {
        "blueF": ("#5fb8ff", "#2363d1"), "blueT": ("#bfe5ff", "#7cc2ff"),
        "skyF": ("#e9f5ff", "#a9cdf2"), "skyT": ("#ffffff", "#d9ecff"),
        "slateF": ("#5d6f88", "#2b3850"), "slateT": ("#a9b9cf", "#7d8ea8"),
        "deckF": ("#c3ccd9", "#8e9ab0"), "deckT": ("#eef2f8", "#cfd7e3"),
        "screen": ("#8fd3ff", "#2f6fd0"),
        "tealF": ("#3fd3c0", "#1b7f8f"), "tealT": ("#b5fff1", "#6fe6d6"),
        "coralF": ("#ff8c74", "#d4434f"), "coralT": ("#ffd2c4", "#ffa996"),
        "paperF": ("#ffffff", "#dfe5f0"), "paperT": ("#ffffff", "#f1f4f9"),
        "violetF": ("#b49bff", "#6a48e0"), "violetT": ("#e6dcff", "#c7b6ff"),
        "cyanF": ("#7fe0ff", "#2b84e0"), "cyanT": ("#d6f6ff", "#a6e9ff"),
        "mintF": ("#5fe3b0", "#16916d"), "mintT": ("#c8ffe9", "#8af0c8"),
        "orangeF": ("#ffb36b", "#f0624f"), "orangeT": ("#ffe2bf", "#ffc890"),
        "dial": ("#22324c", "#0f1828"), "frameF": ("#6a7f9b", "#26344b"), "frameT": ("#c5d2e3", "#93a6c0"),
        "core": ("#e6f8ff", "#5cc0ff"),
    }

    def defs(*ids):
        return base + "".join(grad(i, *G[i]) for i in ids)

    def badge(cx, cy, r, f, t):
        return solid(circle(cx, cy, r), f, t, 0.03, 5)

    wifi = f'<circle cx="39.5" cy="27.5" r="1.7" fill="#fff"/><path d="{WIFI_ARCS}" {W} stroke-width="1.9"/>'
    sync = f'<path d="{SYNC_ARCS}" {W} stroke-width="1.9"/><path d="{SYNC_HEADS}" {W} stroke-width="1.9"/>'
    mag = '<circle cx="44" cy="44" r="3" fill="none" stroke="#fff" stroke-width="1.9"/><path d="m46.3 46.3 2.5 2.5" stroke="#fff" stroke-width="1.9" stroke-linecap="round"/>'

    def laptop(x0, x1, deck_back, deck_front, top):
        """Lid standing on the back edge of a deck seen from above."""
        inset = (x1 - x0) * 0.09
        bx0, bx1 = x0 + inset, x1 - inset
        deck = f"M{n(bx0)} {n(deck_back)}H{n(bx1)}L{n(x1 + 2)} {n(deck_front)}H{n(x0 - 2)}Z"
        band = f"M{n(x0 - 2)} {n(deck_front)}H{n(x1 + 2)}v2.2a1.6 1.6 0 0 1-1.6 1.6H{n(x0 - 0.4)}a1.6 1.6 0 0 1-1.6-1.6Z"
        kb_y0, kb_y1 = deck_back + 1.4, deck_front - 2.2
        kb = f"M{n(bx0 + 4)} {n(kb_y0)}H{n(bx1 - 4)}L{n(x1 - 3.5)} {n(kb_y1)}H{n(x0 + 3.5)}Z"
        lid = rect(bx0, top, bx1 - bx0, deck_back - top + 0.6, 3.5)
        scr = rect(bx0 + 2.6, top + 2.6, bx1 - bx0 - 5.2, deck_back - top - 4.6, 1.6)
        sw, sh = bx1 - bx0 - 5.2, deck_back - top - 4.6
        k = sw * 0.78 / 34
        tx, ty = bx0 + 2.6 + sw * 0.11, top + 2.6 + sh - 14 * k - sh * 0.16
        sc = (f'<path d="{SCENE}" transform="translate({n(round(tx, 2))} {n(round(ty, 2))}) scale({n(round(k, 3))}) translate(-14 -23)" {W} stroke-width="{n(round(2 / k, 2))}" stroke-opacity=".9"/>')
        return (f'<path d="{band}" fill="url(#deckF)"/><path d="{deck}" fill="url(#deckT)"/><path d="{kb}" fill="#aeb9ca" opacity=".55"/>'
                + solid(lid, "slateF", "slateT", 0.02, 4) + f'<path d="{scr}" fill="url(#screen)"/><path d="{scr}" fill="url(#sheen)"/>' + sc)

    def rack(x, ys, w=40, h=11):
        s = ""
        for y in reversed(ys):
            s += solid(rect(x, y, w, h, 2.6), "slateF", "slateT", 0.035, 5)
            s += f'<path d="M{x + 5} {y + 5.5}h15" stroke="#9fb3cc" stroke-opacity=".7" stroke-dasharray="2 2" stroke-linecap="round"/>'
            s += f'<circle cx="{x + 30}" cy="{y + 5.5}" r="1.5" fill="#7ef0c0"/><circle cx="{x + 34.5}" cy="{y + 5.5}" r="1.5" fill="#98d8ff"/>'
        return s

    out = {}
    out["folder"] = svg(defs("blueF", "blueT", "skyF", "skyT", "paperF", "paperT"), ground(32, 56, 24)
        + solid(FOLDER_BACK, "blueF", "blueT", 0.045)
        + solid(rect(12, 16, 38, 20, 2), "paperF", "paperT", 0.01, 2, sheen=False)
        + solid(FOLDER_FRONT, "blueF", "blueT", 0.03, 5)
        + '<path d="M13 47h12" stroke="#d8efff" stroke-opacity=".8" stroke-width="2" stroke-linecap="round"/>')

    out["storage"] = svg(defs("tealF", "tealT", "core"), ground(32, 58, 20)
        + cylinder(32, 42, 19, 8, "tealF", "tealT") + cylinder(32, 31, 19, 8, "tealF", "tealT") + cylinder(32, 20, 19, 8, "tealF", "tealT")
        + '<ellipse cx="32" cy="20" rx="8" ry="2.4" fill="#0f5b66" opacity=".6"/><ellipse cx="32" cy="19.6" rx="5.5" ry="1.6" fill="url(#core)"/>'
        + '<circle cx="45" cy="30.5" r="1.3" fill="#ffe27a"/><circle cx="45" cy="41.5" r="1.3" fill="#ffe27a"/><circle cx="45" cy="52.5" r="1.3" fill="#ffe27a"/>')

    out["computer"] = svg(defs("slateF", "slateT", "deckF", "deckT", "screen"), ground(32, 56, 27)
        + laptop(9, 55, 41, 50, 9))

    out["external-drive"] = svg(defs("coralF", "coralT"), ground(32, 59, 18)
        + solid(rect(14, 12, 36, 44, 8), "coralF", "coralT", 0.06)
        + '<path d="M21 21h22" stroke="#ffd9cf" stroke-opacity=".6" stroke-width="1.4" stroke-linecap="round"/>'
        + '<path d="M23 47h12" stroke="#b83a3f" stroke-width="2.6" stroke-linecap="round"/><circle cx="41" cy="47" r="2" fill="#fff3a8"/>')

    out["files"] = svg(defs("violetF", "violetT", "paperF", "paperT", "orangeF", "orangeT"), ground(32, 57, 20)
        + solid(page(14, 13, 26, 38), "violetF", "violetT", 0.03, 4)
        + solid(page(21, 10, 26, 40), "paperF", "paperT", 0.03, 4) + f'<path d="{fold(21, 10, 26)}" fill="#d5dcea"/>'
        + '<path d="M27 24h13M27 29h13M27 34h8" stroke="#8a74e6" stroke-width="2.2" stroke-linecap="round"/>'
        + badge(45, 45, 8.5, "orangeF", "orangeT") + mag)

    out["discovered"] = svg(defs("violetF", "violetT", "paperF", "paperT", "cyanF", "cyanT"), ground(30, 57, 20)
        + solid(rect(9, 11, 30, 38, 5), "violetF", "violetT", 0.03, 4, ' transform="rotate(-7 24 30)"')
        + solid(rect(15, 13, 30, 38, 5), "paperF", "paperT", 0.03, 4)
        + '<path d="M21 22h18M21 28h14M21 34h9" stroke="#8a74e6" stroke-width="2.4" stroke-linecap="round"/>'
        + '<path d="m47 47 6.5 6.5" stroke="#3d4a66" stroke-width="4.2" stroke-linecap="round"/>'
        + solid(circle(40, 40, 9.5), "cyanF", "cyanT", 0.04, 5) + f'<path d="{LENS}" {W} stroke-width="2"/>')

    out["free-space"] = svg(defs("frameF", "frameT", "dial") + ARC_GRAD, ground(32, 59, 20)
        + solid(circle(32, 32, 24), "frameF", "frameT", 0.07, 8)
        + '<circle cx="32" cy="32" r="17.5" fill="url(#dial)"/><circle cx="32" cy="32" r="17.5" fill="none" stroke="#000" stroke-opacity=".35"/>'
        + f'<path d="{TRACK}" fill="none" stroke="#3a4b66" stroke-width="5" stroke-linecap="round"/>'
        + ARC.format(w=5) + NEEDLE.format(c="#ffffff", w=2.3) + '<circle cx="32" cy="32" r="3" fill="#e6f4ff"/>')

    out["network-share"] = svg(defs("slateF", "slateT"), ground(32, 55, 22) + rack(12, (13.5, 26.5, 39.5)))
    out["nearby-computer"] = svg(defs("slateF", "slateT", "deckF", "deckT", "screen", "mintF", "mintT"), ground(28, 59, 24)
        + laptop(8, 48, 47, 54, 21) + badge(43, 24, 10, "mintF", "mintT") + wifi)
    out["nearby-server"] = svg(defs("slateF", "slateT", "mintF", "mintT"), ground(28, 62, 21)
        + rack(8, RACK_Y) + badge(43, 24, 10, "mintF", "mintT") + wifi)
    out["cloud-drive"] = svg(defs("blueF", "blueT", "skyF", "skyT", "cyanF", "cyanT"), ground(30, 57, 22)
        + solid(CLOUD_BACK, "blueF", "blueT", 0.05) + solid(CLOUD_FRONT, "skyF", "skyT", 0.06)
        + badge(47, 46, 10.5, "cyanF", "cyanT") + sync)
    out["desktop"] = svg(defs("slateF", "slateT", "deckF", "deckT", "screen"), ground(30, 56, 28)
        + solid(rect(21.5, 36, 6, 9, 1.5), "deckF", "deckT", 0.02, 3) + solid(rect(13, 43, 22, 5, 2.5), "deckF", "deckT", 0.03, 4)
        + solid(rect(5, 9, 38, 28, 4), "slateF", "slateT", 0.045, 6)
        + f'<path d="{rect(8.5, 12.5, 31, 21, 2)}" fill="url(#screen)"/><path d="{rect(8.5, 12.5, 31, 21, 2)}" fill="url(#sheen)"/>'
        + f'<path d="{SCENE}" transform="{DT_SCENE}" {W} stroke-width="2.2" stroke-opacity=".9"/>'
        + solid(rect(46, 14, 13, 39, 3), "deckF", "deckT", 0.05, 6)
        + '<path d="M49.5 20h6" stroke="#4a5670" stroke-width="2.2" stroke-linecap="round"/><circle cx="52.5" cy="46" r="1.8" fill="#4da3ff"/>')
    return out


# ======================================================= MORE STYLES (own geometry)
CL = "M16 46a10 10 0 0 1 0-20 14 14 0 0 1 26-5 10 10 0 0 1 8 25Z"


def star(cx, cy, r, k=0.45, points=5, rot=-90):
    import math
    pts = []
    for i in range(points * 2):
        rr = r if i % 2 == 0 else r * k
        a = math.radians(rot + i * 180 / points)
        pts.append(f"{n(round(cx + rr * math.cos(a), 2))} {n(round(cy + rr * math.sin(a), 2))}")
    return "M" + "L".join(pts) + "Z"


# ------------------------------------------------------------- PAPER CUT
def paper():
    """Layered cut paper in craft colors; every layer casts a short soft shadow."""
    NAVY, TERRA, SAND, SAGE, CREAM, SKY, TEAL = "#3d405b", "#e07a5f", "#f2cc8f", "#81b29a", "#f4f1de", "#8ecae6", "#2a9d8f"
    base = blur("ps", 1.1)

    def L(d, fill, lift=1.3, extra=""):
        return (f'<g{extra}><path d="{d}" fill="#2b1d10" opacity=".32" filter="url(#ps)" transform="translate(0 {lift})"/>'
                f'<path d="{d}" fill="{fill}"/></g>')

    def S(d, color, w, lift=1, extra=""):
        return (f'<path d="{d}" fill="none" stroke="#2b1d10" stroke-opacity=".3" stroke-width="{w}" stroke-linecap="round" filter="url(#ps)" transform="translate(0 {lift})"{extra}/>'
                f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"{extra}/>')

    def ring(cx, cy, r1, r2):
        return circle(cx, cy, r1) + circle(cx, cy, r2).replace("a", "a", 1)

    def monitor(x, y, w, h):
        sx, sy, sw, sh = x + 4, y + 4, w - 8, h - 8
        hills = (f"M{sx} {sy + sh}V{n(sy + sh * 0.62)}l{n(sw * 0.24)} {n(-sh * 0.36)} {n(sw * 0.22)} {n(sh * 0.3)}"
                 f" {n(sw * 0.16)} {n(-sh * 0.14)} {n(sw * 0.38)} {n(sh * 0.32)}V{sy + sh}Z")
        return (L(rect(x, y, w, h, 4), NAVY) + f'<path d="{rect(sx, sy, sw, sh, 2)}" fill="{SKY}"/>'
                + f'<circle cx="{n(sx + sw * 0.78)}" cy="{n(sy + sh * 0.3)}" r="{n(sh * 0.16)}" fill="{SAND}"/>'
                + L(hills, SAGE, 0.8) + L(f"M{sx} {sy + sh}V{n(sy + sh * 0.8)}q{n(sw * 0.3)} {n(-sh * 0.25)} {n(sw * 0.55)} 0t{n(sw * 0.45)} {n(-sh * 0.05)}V{sy + sh}Z", TEAL, 0.8)
                + L(rect(x + w / 2 - 4, y + h, 8, 6, 1), "#5b5f7a", 0.8) + L(rect(x + w / 2 - 12, y + h + 5, 24, 4, 2), NAVY))

    def waves(cx, cy):
        return (L(circle(cx, cy, 10), TEAL) + f'<circle cx="{cx - 3.5}" cy="{cy + 3.5}" r="1.8" fill="{CREAM}"/>'
                + f'<path d="M{cx - 3.5} {cy - 1.5}a5 5 0 0 1 5 5M{cx - 3.5} {cy - 5.5}a9 9 0 0 1 9 9" fill="none" stroke="{CREAM}" stroke-width="2.2" stroke-linecap="round"/>')

    def slabs(x, ys, w, h):
        s = ""
        for y, c in zip(ys, (SAGE, TEAL, NAVY)):
            s += L(rect(x, y, w, h, 3), c)
            s += f'<path d="M{x + 5} {y + h / 2}h{n(w * 0.38)}" stroke="{CREAM}" stroke-opacity=".7" stroke-width="2" stroke-linecap="round" stroke-dasharray="3 2.5"/>'
            s += f'<circle cx="{n(x + w - 9)}" cy="{n(y + h / 2)}" r="1.8" fill="{SAND}"/><circle cx="{n(x + w - 4.5)}" cy="{n(y + h / 2)}" r="1.8" fill="{CREAM}"/>'
        return s

    out = {}
    out["folder"] = svg(base, L("M8 16a4 4 0 0 1 4-4h12l4 4h24a4 4 0 0 1 4 4v28a4 4 0 0 1-4 4H12a4 4 0 0 1-4-4Z", NAVY)
        + L(rect(13, 18, 38, 24, 2), CREAM, 1, ' transform="rotate(-4 32 30)"') + L(rect(15, 20, 36, 22, 2), SAND, 1, ' transform="rotate(3 32 30)"')
        + L("M8 30q6-4 12 0t12 0 12 0 12 0v18a4 4 0 0 1-4 4H12a4 4 0 0 1-4-4Z", TERRA, 1.6))
    out["storage"] = svg(base, L(rect(8, 8, 48, 48, 11), NAVY) + L(rect(12, 12, 40, 40, 9), TEAL)
        + "".join(f'<circle cx="{x}" cy="{y}" r="1.6" fill="{CREAM}"/>' for x, y in ((17, 17), (47, 17), (17, 47), (47, 47)))
        + L(circle(32, 32, 13), SAND) + L(circle(32, 32, 9), CREAM) + L(circle(32, 32, 4), TERRA))
    out["files"] = svg(base, L(rect(18, 10, 28, 38, 3), SAGE, 1.2, ' transform="rotate(-14 32 50)"')
        + L(rect(18, 10, 28, 38, 3), SAND, 1.2, ' transform="rotate(-3 32 50)"') + L(rect(18, 10, 28, 38, 3), CREAM, 1.4, ' transform="rotate(9 32 50)"')
        + f'<g transform="rotate(9 32 50)"><path d="M23 18h14M23 23h18M23 28h18M23 33h10" stroke="{NAVY}" stroke-opacity=".55" stroke-width="2" stroke-linecap="round"/></g>')
    out["computer"] = svg(base, monitor(8, 9, 48, 32))
    out["external-drive"] = svg(base, S("M52 33h3a4 4 0 0 1 4 4v14", NAVY, 3) + L(rect(6, 20, 48, 24, 6), TERRA)
        + L(rect(10, 24, 40, 12, 4), SAND, 1) + L(rect(10, 38, 22, 2.6, 1.3), "#c4604a", 0.4)
        + f'<circle cx="44" cy="39.3" r="2" fill="{CREAM}"/>')
    out["discovered"] = svg(base, L(rect(9, 9, 28, 36, 3), CREAM)
        + f'<path d="M14 17h16M14 22h18M14 27h12" stroke="{NAVY}" stroke-opacity=".5" stroke-width="2" stroke-linecap="round"/>'
        + S("m46 46 8 8", NAVY, 5.5) + L(circle(39, 39, 12), TERRA) + L(circle(39, 39, 8), SKY, 0.6)
        + '<path d="M34.5 36a6 6 0 0 1 5-3.5" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round"/>'
        + L(star(48, 14, 7, 0.38, 4, -90), SAND))
    out["free-space"] = svg(base + ARC_GRAD, L(circle(32, 32, 25), NAVY) + L(circle(32, 32, 21), SAND) + L(circle(32, 32, 17.5), CREAM)
        + f'<path d="{TRACK}" fill="none" stroke="#e3d6b4" stroke-width="5" stroke-linecap="round"/>'
        + ARC.format(w=5) + NEEDLE.format(c=NAVY, w=2.4) + L(circle(32, 32, 3.2), TERRA, 0.6))
    out["network-share"] = svg(base, slabs(10, (12, 26, 40), 44, 12))
    out["nearby-computer"] = svg(base, monitor(5, 20, 38, 26) + waves(46, 18))
    out["nearby-server"] = svg(base, L(rect(10, 14, 28, 44, 4), NAVY)
        + "".join(L(rect(14, y, 20, 6, 1.5), c, 0.6) + f'<circle cx="31" cy="{y + 3}" r="1.2" fill="{CREAM}"/>' for y, c in ((19, SAND), (28, SAGE), (37, TEAL)))
        + f'<circle cx="24" cy="50" r="2.5" fill="{TERRA}"/>' + waves(46, 18))
    out["cloud-drive"] = svg(base, L(CL, SKY, 1.4, ' transform="translate(3 0)"') + L(CL, CREAM, 1.2, ' transform="translate(32 40) scale(.68) translate(-30 -40)"')
        + L("M32 47V34m-5 5 5-5 5 5", NAVY, 0).replace('fill="' + NAVY + '"', f'fill="none" stroke="{NAVY}" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"').replace('fill="#2b1d10"', 'fill="none" stroke="#2b1d10" stroke-width="3"'))
    out["desktop"] = svg(base, monitor(4, 8, 40, 28)
        + L(rect(47, 13, 12, 40, 3), NAVY) + L(rect(50, 18, 6, 2.6, 1.3), SAND, 0.6) + L(rect(50, 24, 6, 2, 1), "#5b5f7a", 0.5)
        + f'<circle cx="53" cy="46" r="2" fill="{TERRA}"/>')
    return out


# ------------------------------------------------------------ PIXEL ART
PX = {
    "k": "#1d1b33", "w": "#ffffff", "s": "#c7cfdd", "g": "#8b97ad", "G": "#56627a",
    "Y": "#ffd23f", "L": "#fff1a8", "O": "#f29e1f", "D": "#d98a12",
    "b": "#8fd3ff", "B": "#3a86ff", "N": "#1f2f5c",
    "t": "#7ff0d0", "T": "#1fb89a", "U": "#127a6a",
    "r": "#ff9c8a", "R": "#ee4b4b", "Q": "#b52e3c",
    "p": "#c4a8ff", "P": "#8a5cf0", "e": "#45d483", "E": "#1f9d5a", "c": "#cfe3ff",
}
SPRITES = {
    "folder": ["................", "................", ".kkkkk..........", "kOOOOOk.........", "kOOOOOOkkkkkkkk.",
               "kOOOOOOOOOOOOOOk", "kOkkkkkkkkkkkkkk", "kkLLLLLLLLLLLLLk", "kYYYYYYYYYYYYYYk", "kYYYYYYYYYYYYYYk",
               "kYYYYYYYYYYYYYYk", "kYYYYYYYYYYYYYYk", "kYYYYYYYYYYYYYYk", "kDDDDDDDDDDDDDDk", ".kkkkkkkkkkkkkk.", "................"],
    "storage": ["................", "....kkkkkkkk....", "..kkttttttttkk..", ".kttttttttttttk.", ".kTttttttttttTk.",
                ".kTTkkkkkkkkTTk.", ".kTTTTTTTTTTTTk.", ".kTTTTTTTTTYTTk.", ".kUTTTTTTTTTTUk.", ".kTUUkkkkkkUUTk.",
                ".kTTTTTTTTTTTTk.", ".kTTTTTTTTTYTTk.", ".kUTTTTTTTTTTUk.", "..kkUUUUUUUUkk..", "....kkkkkkkk....", "................"],
    "computer": ["................", "kkkkkkkkkkkkkkkk", "kssssssssssssssk", "ksbbbbbbbbbbbbsk", "ksbbbbbbbbbYYbsk",
                 "ksbbbbbbbbbYYbsk", "ksbbbbebbbbbbbsk", "ksbbbeeebbbbbbsk", "ksbbeeeeebbebbsk", "ksbeeeeeeeeeeesk",
                 "ksEEEEEEEEEEEEsk", "kssssssssssssssk", "kkkkkkkkkkkkkkkk", "......kggk......", "....kkggggkk....", "....kkkkkkkk...."],
    "external-drive": ["................", "...kkkkkkkkkk...", "..krrrrrrrrrrk..", "..krRRRRRRRRRk..", "..krRRRRRRRRRk..",
                       "..krRRRRRRRRRk..", "..krRRRRRRRRRk..", "..krRRRRRRRRRk..", "..krRRRRRRRRRk..", "..krRRRRRRRRRk..",
                       "..krRkkkkkRRRk..", "..krRRRRRRRYRk..", "..kQRRRRRRRRRk..", "..kQQQQQQQQQQk..", "...kkkkkkkkkk...", "................"],
    "files": ["................", ".kkkkkkkkk......", ".kpppppppk......", ".kppkkkkkkkk....", ".kppkwwwwwwkk...",
              ".kppkwwwwwwkwk..", ".kppkwsssswkkkk.", ".kppkwwwwwwwwwk.", ".kppkwssssssswk.", ".kppkwwwwwwwwwk.",
              ".kppkwssssssswk.", ".kPPkwwwwwwwwwk.", ".kkkkwsssswwwwk.", "....kwwwwwwwwwk.", "....kkkkkkkkkkk.", "................"],
    "discovered": ["...........Y....", "..........YLY...", "...kkkkk...Y....", "..kbbbbbk.......", ".kbwwbbbbk......",
                   ".kbwbbbbbk......", ".kbbbbbbbk......", ".kbbbbbbbk......", ".kbbbbbbBk......", "..kbbbBBk.......",
                   "...kkkkkPk......", "........kPPk....", ".........kPPk...", "..........kPPk..", "...........kkk..", "................"],
    "network-share": ["................", ".kkkkkkkkkkkkkk.", ".kssssssssssssk.", ".ksGsGsGsssesbk.", ".kggggggggggggk.",
                      ".kkkkkkkkkkkkkk.", ".kssssssssssssk.", ".ksGsGsGsssesbk.", ".kggggggggggggk.", ".kkkkkkkkkkkkkk.",
                      ".kssssssssssssk.", ".ksGsGsGsssesbk.", ".kggggggggggggk.", ".kkkkkkkkkkkkkk.", "..kk........kk..", "................"],
    "nearby-computer": ["................", "..........eee...", ".............e..", "..........ee..e.", "............e.e.",
                        "..........e.e.e.", "kkkkkkkkkkk.....", "ksssssssssk.....", "ksbbbbbbbsk.....", "ksbbbbbebsk.....",
                        "ksbbebeeesk.....", "kseeeeeeesk.....", "ksssssssssk.....", "kkkkkkkkkkk.....", "....kggk........", "...kkkkkk......."],
    "nearby-server": ["................", "..........eee...", ".............e..", "..........ee..e.", "............e.e.",
                      "kkkkkkkkkke.e.e.", "kssssssssk......", "ksGsGssesk......", "kggggggggk......", "kkkkkkkkkk......",
                      "kssssssssk......", "ksGsGssesk......", "kggggggggk......", "kkkkkkkkkk......", ".kk....kk.......", "................"],
    "desktop": ["................", "kkkkkkkkkk......", "kssssssssk.kkkkk", "ksbbbbbbsk.ksssk", "ksbbbbbYsk.kGGGk",
                "ksbbbbbbsk.ksssk", "ksbbebbbsk.ksssk", "ksbeeeebsk.ksssk", "kseeeeeesk.kGGGk", "ksEEEEEEsk.ksssk",
                "kssssssssk.ksssk", "kkkkkkkkkk.ksesk", "....kggk...ksssk", "..kggggggk.kgggk", "..kkkkkkkk.kkkkk", "................"],
    "cloud-drive": ["................", "................", "................", "......kkkk......", ".....kwwwwk.kk..",
                    "..kkkwwwwwwkwwk.", ".kwwwwwwBwwwwwk.", "kwwwwwwBBBwwwwwk", "kwwwwwBBBBBwwwck", "kcwwwwwwBwwwwcck",
                    "kccccccBccccccck", ".kkkkkkkkkkkkkk.", "................", "................", "................", "................"],
}


def pixelart():
    """Hand-placed 16 x 16 pixel sprites with a dark outline."""
    def sprite(rows):
        assert len(rows) == 16 and all(len(r) == 16 for r in rows), rows
        by = {}
        for y, row in enumerate(rows):
            x = 0
            while x < 16:
                ch = row[x]
                if ch == ".":
                    x += 1
                    continue
                start = x
                while x < 16 and row[x] == ch:
                    x += 1
                by.setdefault(ch, []).append(f"M{start * 4} {y * 4}h{(x - start) * 4}v4h{-(x - start) * 4}z")
        return "".join(f'<path fill="{PX[c]}" d="{"".join(d)}"/>' for c, d in by.items())

    out = {k: svg("", f'<g shape-rendering="crispEdges">{sprite(v)}</g>') for k, v in SPRITES.items()}
    rows = []
    for y in range(16):
        row = ""
        for x in range(16):
            d = ((x + 0.5 - 8) ** 2 + (y + 0.5 - 8) ** 2) ** 0.5
            row += "k" if 6.7 < d <= 7.7 else "g" if 5.8 < d <= 6.7 else "s" if 5.4 < d <= 5.8 else "N" if d <= 5.4 else "."
        rows.append(row)
    out["free-space"] = svg(ARC_GRAD, f'<g shape-rendering="crispEdges">{sprite(rows)}</g>'
        + f'<path d="{TRACK}" fill="none" stroke="#2d3f6e" stroke-width="4"/>' + ARC.format(w=4).replace('stroke-linecap="round"', 'stroke-linecap="square"')
        + NEEDLE.format(c="#ffffff", w=2.5).replace('stroke-linecap="round"', 'stroke-linecap="square"') + '<rect x="30" y="30" width="4" height="4" fill="#ffd23f"/>')
    return out


# ------------------------------------------------------------- RISO PRINT
def riso():
    """Two-ink risograph print: fluorescent pink and blue overprinting (multiply), halftone shading, slight misregistration."""
    BL, PK, YE = "#0078bf", "#ff48b0", "#ffe800"
    base = ''.join(f'<pattern id="h{c}" width="2.6" height="2.6" patternUnits="userSpaceOnUse" patternTransform="rotate(30)"><circle cx="1.3" cy="1.3" r=".8" fill="{v}"/></pattern>'
                   for c, v in (("b", BL), ("p", PK)))

    def ink(d, color, extra=""):
        return f'<path d="{d}" fill="{color}" style="mix-blend-mode:multiply"{extra}/>'

    def line(d, color, w, extra=""):
        return f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round" style="mix-blend-mode:multiply"{extra}/>'

    def P(body, defs=""):
        return svg(base + defs, f'<g style="isolation:isolate">{body}</g>')

    MIS = ' transform="translate(1.2 .7)"'

    def monitor(x, y, w, h):
        return (ink(rect(x, y, w, h, 3), BL) + ink(f"M{x + 4} {y + h - 3}V{n(y + h * 0.6)}l{n(w * 0.22)} {n(-h * 0.3)} {n(w * 0.2)} {n(h * 0.24)} {n(w * 0.14)} {n(-h * 0.12)} {n(w * 0.36 - 8 + 4)} {n(h * 0.3)}V{y + h - 3}Z", PK, MIS)
                + f'<circle cx="{n(x + w * 0.76)}" cy="{n(y + h * 0.32)}" r="{n(h * 0.15)}" fill="{YE}" style="mix-blend-mode:multiply"/>'
                + ink(rect(x + w / 2 - 5, y + h, 10, 5, 0.5), BL) + ink(rect(x + w / 2 - 13, y + h + 5, 26, 4.5, 1), PK, MIS))

    def wifi(cx, cy):
        return (f'<circle cx="{cx}" cy="{cy}" r="2.6" fill="{PK}" style="mix-blend-mode:multiply"/>'
                + line(f"M{cx} {cy - 7}a7 7 0 0 1 7 7M{cx} {cy - 13}a13 13 0 0 1 13 13", PK, 3.6))

    def racks(x, ys, w, h):
        s = ""
        for y in ys:
            s += ink(rect(x, y, w, h, 2), BL) + ink(rect(x + 4, y + h / 2 - 1.4, w * 0.45, 2.8, 1.4), YE)
            s += f'<circle cx="{n(x + w - 9)}" cy="{n(y + h / 2)}" r="2" fill="{PK}" style="mix-blend-mode:multiply"/><circle cx="{n(x + w - 4.5)}" cy="{n(y + h / 2)}" r="2" fill="{PK}" style="mix-blend-mode:multiply"/>'
        return s

    out = {}
    out["folder"] = P(ink("M8 13h16l5 5h27v32H8Z", BL) + ink(rect(13, 17, 34, 22, 0.5), YE, ' transform="rotate(-4 30 28)"')
        + ink("M8 26h48v24H8Z", PK, MIS) + ink("M8 40h48v10H8Z", "url(#hb)"))
    out["storage"] = P(ink(circle(32, 32, 23), BL) + line(circle(32, 32, 15), PK, 7, MIS) + ink("M32 9a23 23 0 0 1 0 46Z", "url(#hp)")
        + ink(circle(32, 32, 5), YE) + '<circle cx="32" cy="32" r="1.6" fill="#fff"/>')
    out["files"] = P(ink(rect(10, 14, 26, 34, 0.5), YE, ' transform="rotate(-8 23 31)"') + ink(rect(10, 14, 26, 34, 0.5), "url(#hb)", ' transform="rotate(-8 23 31)"')
        + ink(rect(22, 10, 26, 36, 0.5), PK, ' transform="rotate(5 35 28)"')
        + line("M28 20h14M28 26h14M28 32h9", BL, 2.6, ' transform="rotate(5 35 28)"'))
    out["computer"] = P(monitor(7, 9, 50, 34))
    out["external-drive"] = P(ink(rect(14, 8, 36, 48, 3), PK) + ink(rect(14, 8, 36, 30, 3), "url(#hb)") + ink("M14 40h36v13a3 3 0 0 1-3 3H17a3 3 0 0 1-3-3Z", BL, MIS)
        + ink(rect(20, 46, 14, 3.2, 1), YE) + f'<circle cx="42" cy="47.6" r="2.8" fill="{YE}"/>')
    out["discovered"] = P(ink(rect(9, 8, 30, 40, 0.5), YE) + line("M15 17h16M15 23h18M15 29h11", BL, 2.6)
        + line("m47 47 9 9", BL, 6) + ink(circle(39, 39, 11), "url(#hp)") + line(circle(39, 39, 10), PK, 6, MIS))
    out["free-space"] = svg(base + ARC_GRAD, f'<g style="isolation:isolate">{ink(circle(32, 32, 25), PK)}{ink(circle(32, 32, 25), "url(#hb)")}{ink(circle(32, 32, 18.5), BL, MIS)}</g>'
        + f'<path d="{TRACK}" fill="none" stroke="#ffffff" stroke-opacity=".3" stroke-width="5" stroke-linecap="round"/>'
        + ARC.format(w=5) + NEEDLE.format(c="#ffffff", w=2.6) + f'<circle cx="32" cy="32" r="3.2" fill="{YE}"/>')
    out["network-share"] = P(racks(10, (11, 26, 41), 44, 12))
    out["nearby-computer"] = P(monitor(5, 22, 36, 24) + wifi(42, 22))
    out["nearby-server"] = P(racks(6, (24, 36, 48), 32, 9) + wifi(42, 20))
    out["cloud-drive"] = P(ink(CL, BL) + ink(CL, PK, ' transform="translate(37 30) scale(.62) translate(-30 -36)"')
        + line("M33 50V34m-6 6 6-6 6 6", YE, 4.4) + line("M33 50V34m-6 6 6-6 6 6", BL, 1.4, MIS))
    out["desktop"] = P(monitor(4, 9, 38, 28) + ink(rect(46, 13, 13, 40, 2), PK) + ink(rect(46, 13, 13, 18, 2), "url(#hb)")
        + ink(rect(49.5, 36, 6, 2.6, 1.3), YE) + f'<circle cx="52.5" cy="46" r="2.2" fill="{BL}" style="mix-blend-mode:multiply"/>')
    return out


# -------------------------------------------------------------- BLUEPRINT
def blueprint():
    """Technical drawings: white linework on a gridded blueprint tile, with hidden lines and dimensions."""
    base = (lg("bp", 4, 4, 60, 60, (0, "#2a63c4"), (1, "#163c88"))
            + '<pattern id="grid" width="6" height="6" patternUnits="userSpaceOnUse" x="4" y="4"><path d="M6 0H0V6" fill="none" stroke="#fff" stroke-opacity=".14" stroke-width=".6"/></pattern>')
    tile = (f'<path d="{rect(4, 4, 56, 56, 12)}" fill="url(#bp)"/><path d="{rect(4, 4, 56, 56, 12)}" fill="url(#grid)"/>'
            f'<path d="{rect(5, 5, 54, 54, 11)}" fill="none" stroke="#fff" stroke-opacity=".28" stroke-width=".8"/>')

    def Ln(d, w=1.5, extra=""):
        return f'<path d="{d}" fill="none" stroke="#fff" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"{extra}/>'

    def H(d):
        return f'<path d="{d}" fill="none" stroke="#fff" stroke-opacity=".6" stroke-width=".9" stroke-dasharray="2 1.6"/>'

    def axis(d):
        return f'<path d="{d}" fill="none" stroke="#fff" stroke-opacity=".45" stroke-width=".7" stroke-dasharray="5 1.5 1 1.5"/>'

    def dim_h(x1, x2, y):
        return f'<path d="M{x1} {y}H{x2}M{x1} {y - 1.8}v3.6M{x2} {y - 1.8}v3.6" stroke="#bfe0ff" stroke-opacity=".8" stroke-width=".7" fill="none"/>'

    def dim_v(y1, y2, x):
        return f'<path d="M{x} {y1}V{y2}M{x - 1.8} {y1}h3.6M{x - 1.8} {y2}h3.6" stroke="#bfe0ff" stroke-opacity=".8" stroke-width=".7" fill="none"/>'

    def B(body, defs=""):
        return svg(base + defs, tile + body)

    def wifi(cx, cy):
        return f'<circle cx="{cx}" cy="{cy}" r="1.5" fill="#fff"/>' + Ln(f"M{cx} {cy - 6}a6 6 0 0 1 6 6M{cx} {cy - 11}a11 11 0 0 1 11 11", 1.4)

    def monitor(x, y, w, h):
        return (Ln(rect(x, y, w, h, 2.5)) + Ln(rect(x + 3, y + 3, w - 6, h - 6, 1), 0.8)
                + Ln(f"M{n(x + w / 2 - 3)} {y + h}l-1.5 6h{n(9)}l-1.5-6M{n(x + w / 2 - 8)} {y + h + 6}h16")
                + axis(f"M{n(x + w / 2)} {y + 4}v{h - 8}M{x + 4} {n(y + h / 2)}h{w - 8}"))

    out = {}
    out["folder"] = B(Ln("M13 21a2 2 0 0 1 2-2h8l3 3h21a2 2 0 0 1 2 2v20a2 2 0 0 1-2 2H15a2 2 0 0 1-2-2Z") + Ln("M13 28h36")
        + H("M17 24h28v12H17Z") + dim_h(13, 49, 51) + dim_v(19, 46, 54))
    out["storage"] = B(axis("M32 11v42") + Ln('M18 20v22a14 4.5 0 0 0 28 0V20') + '<ellipse cx="32" cy="20" rx="14" ry="4.5" fill="none" stroke="#fff" stroke-width="1.5"/>'
        + Ln("M18 31a14 4.5 0 0 0 28 0", 1) + H("M18 42a14 4.5 0 0 1 28 0") + H("M18 31a14 4.5 0 0 1 28 0") + dim_v(20, 46.5, 52) + dim_h(18, 46, 52.5))
    out["files"] = B(H("M14 14h22v30H14Z") + Ln(page(21, 18, 25, 32, 7, 2)) + Ln("M39 18v5a2 2 0 0 0 2 2h5", 1)
        + Ln("M26 31h15M26 36h15M26 41h9", 1) + dim_h(21, 46, 54) + dim_v(18, 50, 51.5).replace("51.5", "50.5", 0))
    out["computer"] = B(monitor(12, 12, 40, 27) + dim_h(12, 52, 9))
    out["external-drive"] = B(Ln(rect(19, 11, 26, 40, 5)) + H(circle(32, 26, 8)) + axis("M32 15v22M24 26h16")
        + Ln("M25 43h9") + '<circle cx="39" cy="43" r="1.6" fill="none" stroke="#fff" stroke-width="1.1"/>' + dim_v(11, 51, 51) + dim_h(19, 45, 55))
    out["discovered"] = B(Ln(rect(11, 11, 24, 32, 2)) + Ln("M15 19h14M15 24h16M15 29h10", 1)
        + '<circle cx="38" cy="38" r="9.5" fill="#2a63c4" fill-opacity=".85"/>' + Ln(circle(38, 38, 9.5)) + axis("M38 26v24M26 38h24")
        + Ln("m44.7 44.7 7.3 7.3", 2.6) + f'<path d="M38 38l6.7-6.7" stroke="#bfe0ff" stroke-width=".7"/>')
    ticks = ""
    import math
    for i in range(10):
        a = math.radians(135 + i * 30)
        if i * 30 > 270:
            break
        ticks += f"M{n(round(32 + 19 * math.cos(a), 2))} {n(round(32 + 19 * math.sin(a), 2))}L{n(round(32 + 22 * math.cos(a), 2))} {n(round(32 + 22 * math.sin(a), 2))}"
    out["free-space"] = B(Ln(circle(32, 32, 24)) + Ln(ticks, 1) + axis("M32 6v52M6 32h52").replace("M32 6v52M6 32h52", "M32 9v46M9 32h46")
        + f'<path d="{TRACK}" fill="none" stroke="#fff" stroke-opacity=".2" stroke-width="4.4" stroke-linecap="round"/>'
        + ARC.format(w=4.4) + NEEDLE.format(c="#ffffff", w=1.8) + '<circle cx="32" cy="32" r="2.4" fill="#fff"/>', ARC_GRAD)

    def rack(x, ys, w, h):
        s = ""
        for y in ys:
            s += Ln(rect(x, y, w, h, 2), 1.3) + f'<path d="M{x + 4} {n(y + h / 2)}h{n(w * 0.4)}" stroke="#fff" stroke-opacity=".7" stroke-width=".9" stroke-dasharray="1.6 1.6"/>'
            s += f'<circle cx="{n(x + w - 8)}" cy="{n(y + h / 2)}" r="1.3" fill="none" stroke="#fff"/><circle cx="{n(x + w - 4)}" cy="{n(y + h / 2)}" r="1.3" fill="#fff"/>'
        return s

    out["network-share"] = B(rack(14, (14, 26, 38), 36, 9) + dim_v(14, 47, 54) + dim_h(14, 50, 52))
    out["nearby-computer"] = B(monitor(9, 25, 30, 20) + wifi(42, 26))
    out["nearby-server"] = B(rack(10, (26, 36, 46), 27, 7.5) + wifi(42, 24))
    cl = ' transform="translate(32 34) scale(.84) translate(-32 -34)"'
    out["cloud-drive"] = B(f'<g{cl}>' + H(circle(16, 36, 10)) + H(circle(29.86, 27.97, 14)) + H(circle(46, 33.5, 13.1)) + Ln(CL, 1.8) + "</g>"
        + Ln("M32 47V36m-4 4 4-4 4 4", 1.6))
    out["desktop"] = B('<g transform="translate(32 32) scale(.82) translate(-32 -31)">' + monitor(5, 9, 38, 28)
        + Ln(rect(46, 12, 13, 41, 2.5)) + Ln("M49.5 18h6", 1.1) + Ln("M49.5 22h6", 1.1) + H(rect(48, 27, 9, 12, 1.5))
        + '<circle cx="52.5" cy="46" r="1.6" fill="none" stroke="#fff" stroke-width="1.1"/></g>')
    return out


# ---------------------------------------------------------------- STICKER
def sticker():
    """Die-cut vinyl stickers: flat bold colors, ink outlines, a thick white border and a slight tilt."""
    Y, P, B, V, O, G, W, K, S = "#ffbe0b", "#ff006e", "#3a86ff", "#8338ec", "#fb5607", "#06d6a0", "#ffffff", "#1d2140", "#8ecdf8"
    base = blur("ss", 1.8)

    def make(parts, rot=-6, after=""):
        sil, ink = "", ""
        for p in parts:
            if p[0] == "f":
                d, c, t = p[1], p[2], (f' transform="{p[3]}"' if len(p) > 3 else "")
                sil += f'<path d="{d}"{t} fill="#fff" stroke="#fff" stroke-width="7" stroke-linejoin="round"/>'
                ink += f'<path d="{d}"{t} fill="{c}" stroke="{K}" stroke-width="1.8" stroke-linejoin="round"/>'
            else:
                _, d, c, w = p
                sil += f'<path d="{d}" fill="none" stroke="#fff" stroke-width="{w + 7}" stroke-linecap="round" stroke-linejoin="round"/>'
                ink += (f'<path d="{d}" fill="none" stroke="{K}" stroke-width="{n(w + 3)}" stroke-linecap="round" stroke-linejoin="round"/>'
                        f'<path d="{d}" fill="none" stroke="{c}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"/>')
        shadow = sil.replace('#fff', '#0b1226')
        return (f'<g transform="rotate({rot} 32 32)"><g opacity=".35" filter="url(#ss)" transform="translate(1.2 2.4)">{shadow}</g>{sil}{ink}'
                f'{after}</g>')

    def crt(x, y, w, h):
        return [("f", rect(x, y, w, h, 7), O), ("f", rect(x + 5, y + 5, w - 10, h - 10, 4.5), B),
                ("f", star(x + w * 0.4, y + h * 0.48, h * 0.2, 0.3, 4), W),
                ("f", f"M{n(x + w / 2 - 8)} {y + h}h16l3 6H{n(x + w / 2 - 11)}Z", "#d9480f")]

    def badge(cx, cy):
        return [("f", circle(cx, cy, 9), G)], (f'<circle cx="{cx - 3}" cy="{cy + 3}" r="1.7" fill="#fff"/>'
                f'<path d="M{cx - 3} {cy - 1.5}a4.5 4.5 0 0 1 4.5 4.5M{cx - 3} {cy - 5.5}a8.5 8.5 0 0 1 8.5 8.5" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round"/>')

    def rack(x, ys, w, h):
        parts, extra = [], ""
        for y in ys:
            parts.append(("f", rect(x, y, w, h, 3), B))
            extra += f'<path d="M{x + 5} {n(y + h / 2)}h{n(w * 0.38)}" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-dasharray="2.5 2.5"/>'
            extra += f'<circle cx="{n(x + w - 9)}" cy="{n(y + h / 2)}" r="2" fill="{G}" stroke="{K}" stroke-width="1"/><circle cx="{n(x + w - 4.5)}" cy="{n(y + h / 2)}" r="2" fill="{Y}" stroke="{K}" stroke-width="1"/>'
        return parts, extra

    out = {}
    out["folder"] = svg(base, make([("f", "M10 20a3 3 0 0 1 3-3h10l4 4h21a3 3 0 0 1 3 3v22a3 3 0 0 1-3 3H13a3 3 0 0 1-3-3Z", V),
                                    ("f", rect(15, 22, 32, 14, 2), Y), ("f", "M10 30h41v16a3 3 0 0 1-3 3H13a3 3 0 0 1-3-3Z", "#a66bff")]))
    out["storage"] = svg(base, make([("f", "M14 20v24a18 6 0 0 0 36 0V20Z", G), ("f", "M14 20a18 6 0 1 0 36 0a18 6 0 1 0-36 0Z", "#7ff0cf"),
                                     ("s", "M14 32a18 6 0 0 0 36 0", K, 0.01)],
                                    after='<ellipse cx="32" cy="20" rx="7" ry="2.2" fill="#fff" opacity=".7"/><circle cx="44" cy="40" r="1.8" fill="#fff"/><circle cx="44" cy="50" r="1.8" fill="#fff"/>'))
    out["files"] = svg(base, make([("f", rect(14, 14, 24, 32, 3), B), ("f", rect(24, 11, 24, 32, 3), W), ("f", star(45, 45, 8.5), Y)],
                                  after=f'<path d="M29 19h14M29 24h14M29 29h9" stroke="{B}" stroke-width="2.4" stroke-linecap="round"/>'))
    out["computer"] = svg(base, make(crt(10, 11, 44, 32)))
    out["external-drive"] = svg(base, make([("f", rect(17, 9, 30, 46, 7), Y), ("f", "M35 16l-10 16h7l-3 15 11-18h-7l3-13Z", O)], rot=5))
    out["discovered"] = svg(base, make([("f", star(25, 26, 15, 0.48), Y), ("s", "m45 45 8 8", P, 5), ("f", circle(39, 39, 10), S), ("s", circle(39, 39, 10), P, 3.4)],
                                       after='<path d="M34 36a6 6 0 0 1 5-4" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round"/>'))
    out["free-space"] = svg(base + ARC_GRAD, make([("f", circle(32, 32, 24), W), ("f", circle(32, 32, 18), K)], rot=0,
        after=f'<path d="{TRACK}" fill="none" stroke="#3b4266" stroke-width="5" stroke-linecap="round"/>' + ARC.format(w=5)
        + NEEDLE.format(c="#ffffff", w=2.6) + f'<circle cx="32" cy="32" r="3.2" fill="{Y}"/>'))
    parts, extra = rack(11, (12, 26, 40), 42, 11)
    out["network-share"] = svg(base, make(parts, after=extra))
    bp, bx = badge(46, 18)
    out["nearby-computer"] = svg(base, make(crt(7, 22, 34, 24) + bp, after=bx))
    parts, extra = rack(8, (24, 36, 48), 32, 9)
    out["nearby-server"] = svg(base, make(parts + bp, after=extra + bx))
    out["cloud-drive"] = svg(base, make([("f", CL, S, "translate(21 -4) scale(.7)"), ("f", CL, W)], rot=-4,
        after=f'<path d="M33 46V33m-5 5 5-5 5 5" fill="none" stroke="{B}" stroke-width="3.4" stroke-linecap="round" stroke-linejoin="round"/>'))
    out["desktop"] = svg(base, make(crt(4, 11, 38, 28) + [("f", rect(46, 14, 11, 38, 4), V)],
        after=f'<path d="M49.5 21h4M49.5 26h4" stroke="#ffffff" stroke-width="2" stroke-linecap="round"/><circle cx="51.5" cy="45" r="2.2" fill="{Y}" stroke="{K}" stroke-width="1"/>'))
    return out


# ============================================== SCREENSHOT FAMILY (dark bodies + glowing accent)
def sparkle(cx, cy, r, fill="#ffffff", op=1):
    return (f'<path d="M{n(cx)} {n(cy - r)}Q{n(cx)} {n(cy)} {n(cx + r)} {n(cy)}Q{n(cx)} {n(cy)} {n(cx)} {n(cy + r)}'
            f'Q{n(cx)} {n(cy)} {n(cx - r)} {n(cy)}Q{n(cx)} {n(cy)} {n(cx)} {n(cy - r)}Z" fill="{fill}" opacity="{op}"/>')


def rpoly(pts, r):
    """A polygon with every corner rounded by up to r."""
    import math
    out = ""
    k = len(pts)
    for i, (x, y) in enumerate(pts):
        px, py = pts[i - 1]
        nx, ny = pts[(i + 1) % k]
        d1, d2 = math.hypot(px - x, py - y), math.hypot(nx - x, ny - y)
        t1, t2 = min(r, d1 / 2) / d1, min(r, d2 / 2) / d2
        a = (round(x + (px - x) * t1, 2), round(y + (py - y) * t1, 2))
        b = (round(x + (nx - x) * t2, 2), round(y + (ny - y) * t2, 2))
        out += ("M" if i == 0 else "L") + f"{n(a[0])} {n(a[1])}Q{n(x)} {n(y)} {n(b[0])} {n(b[1])}"
    return out + "Z"


def family(P):
    """Bodies in one material (navy glass or silver), a hairline rim, a glowing accent
    gradient and small LED dots: the language of the Overview mockup."""
    defs = (lg("body", 0, 6, 0, 58, (0, P["b1"]), (1, P["b2"])) + lg("acc", 8, 8, 56, 56, (0, P["a1"]), (1, P["a2"]))
            + lg("accd", 8, 8, 56, 56, (0, P["a2"]), (1, P["a3"])) + lg("acc2", 8, 8, 56, 56, (0, P["c1"]), (1, P["c2"]))
            + lg("inner", 0, 8, 0, 56, (0, P["in1"]), (1, P["in2"]))
            + lg("rim", 0, 6, 0, 58, (0, "#ffffff", P["rim_t"]), (1, "#ffffff", P["rim_b"]))
            + lg("sheen", 0, 6, 0, 36, (0, "#ffffff", P["sheen"]), (1, "#ffffff", 0))
            + blur("g", 3.4) + blur("sh", 2.2) + ARC_GRAD)
    W = 'stroke="#ffffff" stroke-linecap="round" stroke-linejoin="round" fill="none"'

    def shadow(d, dy=2.6, op=0.5):
        return f'<path d="{d}" fill="#04060f" opacity="{op}" filter="url(#sh)" transform="translate(0 {dy})"/>'

    def glow(d, op=0.8, color=None):
        return f'<path d="{d}" fill="{color or P["glow"]}" opacity="{op}" filter="url(#g)"/>'

    def body(d, fill="body", w=0.9, sheen=True, sh=True):
        edge = (f'<path d="{d}" fill="none" stroke="{P["edge"][0]}" stroke-opacity="{P["edge"][1]}" stroke-width="{n(w + 1.1)}"/>' if P["edge"] else "")
        return ((shadow(d) if sh else "") + edge + f'<path d="{d}" fill="url(#{fill})"/>'
                + (f'<path d="{d}" fill="url(#sheen)"/>' if sheen else "") + f'<path d="{d}" fill="none" stroke="url(#rim)" stroke-width="{w}"/>')

    def led(cx, cy, r, color):
        return f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r * 2.2)}" fill="{color}" opacity=".45" filter="url(#sh)"/><circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}" fill="{color}"/>'

    def laptop(sc=1.0, tx=0.0, ty=0.0):
        # Seen from above and slightly closed: the screen leans toward you (wider at the top) with
        # almost no bezel, over a deck with a recessed keyboard and a trackpad.
        lid = rpoly([(6.5, 9.5), (57.5, 9.5), (54, 31.2), (10, 31.2)], 3.4)
        scr = rpoly([(8.6, 11.4), (55.4, 11.4), (52.4, 29.2), (11.6, 29.2)], 2.2)
        deck = rpoly([(10, 30), (54, 30), (59, 53.5), (5, 53.5)], 4)
        keys = rpoly([(13.4, 33.6), (50.6, 33.6), (53, 42.2), (11, 42.2)], 2)
        pad = rect(23, 44.4, 18, 6, 1.8)
        dashes = "".join(f'<path d="M{n(x0)} {n(y)}H{n(64 - x0)}" stroke="{P["line"]}" stroke-width="1.2" stroke-linecap="round" stroke-dasharray="2.4 1.5" opacity=".45"/>'
                         for x0, y in ((16.2, 35.7), (15.4, 38.4)))
        g = (body(deck) + f'<path d="{keys}" fill="url(#inner)" opacity=".92"/>' + dashes
             + f'<path d="{pad}" fill="{P["line"]}" opacity=".28"/><path d="{pad}" fill="none" stroke="{P["line"]}" stroke-width=".8" opacity=".7"/>'
             + body(lid, "inner", sh=False) + glow(scr, 0.6) + f'<path d="{scr}" fill="url(#acc)"/>'
             + f'<path d="M8.6 11.4h24L22 29.2H11.6Z" fill="#ffffff" opacity=".16"/>')
        return f'<g transform="translate({n(tx)} {n(ty)}) scale({n(sc)})">{g}</g>'

    def racks(x, ys, w, h):
        s = ""
        for i, y in enumerate(ys):
            s += body(rect(x, y, w, h, 3.5), w=0.8)
            c = P["dots"][i % len(P["dots"])]
            s += led(x + 6, y + h / 2, 1.5, c) + f'<circle cx="{n(x + 11.5)}" cy="{n(y + h / 2)}" r="1.4" fill="{P["line"]}" opacity=".5"/><circle cx="{n(x + 16.5)}" cy="{n(y + h / 2)}" r="1.4" fill="{P["line"]}" opacity=".5"/>'
            s += f'<path d="M{n(x + w - 16)} {n(y + h / 2)}h10" stroke="{P["line"]}" stroke-opacity=".6" stroke-width="1.6" stroke-linecap="round"/>'
        return s

    def badge(cx, cy, r, inner):
        return (shadow(circle(cx, cy, r)) + glow(circle(cx, cy, r), 0.5) + f'<path d="{circle(cx, cy, r)}" fill="url(#acc)"/>'
                + f'<path d="{circle(cx, cy, r)}" fill="none" stroke="url(#rim)" stroke-width=".9"/>' + inner)

    def wifi(cx, cy):
        return (f'<circle cx="{n(cx - 3.5)}" cy="{n(cy + 3.5)}" r="1.7" fill="#fff"/>'
                f'<path d="M{n(cx - 3.5)} {n(cy - 1.5)}a5 5 0 0 1 5 5M{n(cx - 3.5)} {n(cy - 5.5)}a9 9 0 0 1 9 9" {W} stroke-width="2"/>')

    def storage():
        if P["storage"] == "hex":
            return (body(HEX) + glow(HEX_IN, 0.85) + f'<path d="{HEX_IN}" fill="url(#acc)"/>'
                    + f'<path d="{HEX_IN}" fill="none" stroke="#ffffff" stroke-opacity=".55" stroke-width=".9"/>'
                    + '<path d="m32 21 8 4.6v9.2L32 39.4l-8-4.6v-9.2Z" fill="#ffffff" fill-opacity=".92"/>')
        if P["storage"] == "stack":
            s = ""
            for k, y in reversed(list(enumerate((15, 28, 41)))):
                side = f"M10 {y}v11a22 6.5 0 0 0 44 0V{y}"
                top = f'<ellipse cx="32" cy="{y}" rx="22" ry="6.5"'
                fill = "acc" if k != 1 else "acc2"
                s += (shadow(side + "Z", 2, 0.4) + glow(side + "Z", 0.35) + f'<path d="{side}Z" fill="url(#{fill})"/>'
                      + f'{top} fill="url(#{fill})"/>{top} fill="#ffffff" opacity=".3"/>'
                      + f'<path d="M10 {y}a22 6.5 0 0 0 44 0" fill="none" stroke="#06091a" stroke-opacity=".5" stroke-width="1.4"/>'
                      + f'{top} fill="none" stroke="url(#rim)" stroke-width=".9"/>'
                      + f'<circle cx="46" cy="{y + 9}" r="1.5" fill="#ffffff" opacity=".95"/><circle cx="41" cy="{y + 9.6}" r="1.5" fill="#ffffff" opacity=".6"/>')
            return s
        # puck: a silver drum with a glowing ring on top
        side = "M11 25v19a21 7.5 0 0 0 42 0V25"
        return (shadow(side + "Z") + (f'<path d="{side}Z" fill="none" stroke="{P["edge"][0]}" stroke-opacity="{P["edge"][1]}" stroke-width="2"/>' if P["edge"] else "")
                + f'<path d="{side}Z" fill="url(#body)"/><path d="{side}" fill="none" stroke="url(#rim)" stroke-width=".9"/>'
                + '<ellipse cx="32" cy="25" rx="21" ry="7.5" fill="#ffffff" fill-opacity=".55"/>'
                + '<ellipse cx="32" cy="25" rx="21" ry="7.5" fill="none" stroke="url(#rim)" stroke-width=".9"/>'
                + '<ellipse cx="32" cy="25.4" rx="13" ry="4.4" fill="url(#inner)"/>'
                + '<ellipse cx="32" cy="25.4" rx="6.5" ry="2.2" fill="url(#acc)"/>' + '<ellipse cx="32" cy="25" rx="6.5" ry="2.2" fill="#9fd0ff" opacity=".7" filter="url(#sh)"/>'
                + f'<path d="M24 42h13" stroke="{P["line"]}" stroke-width="2.2" stroke-linecap="round"/>' + led(44, 42, 1.6, P["dots"][0]))

    out = {}
    out["storage"] = svg(defs, storage())
    out["folder"] = svg(defs, body(FOLDER_BACK, "accd", sheen=False)
        + glow(FOLDER_FRONT, 0.35) + f'<path d="{FOLDER_FRONT}" fill="url(#acc)"/><path d="{FOLDER_FRONT}" fill="url(#sheen)"/>'
        + f'<path d="{FOLDER_FRONT}" fill="none" stroke="url(#rim)" stroke-width=".9"/>')
    out["files"] = svg(defs,
        f'<g transform="rotate(-9 27 32)">{body(page(12, 12, 30, 40, 9, 4), "accd", w=0.8)}</g>'
        + body(page(19, 9, 30, 42, 10, 4)) + f'<path d="{fold(19, 9, 30, 10)}" fill="{P["line"]}" opacity=".55"/>'
        + f'<path d="M26 26h14M26 32h18M26 38h11" stroke="url(#acc)" stroke-width="2.4" stroke-linecap="round"/>'
        + sparkle(50, 15, 6.5) + sparkle(56, 26, 3, P["spark"]))
    out["computer"] = svg(defs, laptop(1, 0, 0.5))
    out["external-drive"] = svg(defs, body(rect(14, 8, 36, 48, 9)) + body(rect(19.5, 13.5, 25, 22, 5), "inner", w=0.7, sh=False)
        + glow(circle(32, 24.5, 4.5), 0.8) + f'<path d="{circle(32, 24.5, 4)}" fill="url(#acc)"/>'
        + f'<path d="{rect(20, 41, 20, 4, 2)}" fill="{P["ink"]}" opacity=".55"/>' + led(44.5, 47.5, 1.7, P["dots"][0]))
    out["discovered"] = svg(defs, body(page(8, 8, 32, 42, 10, 4)) + f'<path d="{fold(8, 8, 32, 10)}" fill="{P["line"]}" opacity=".55"/>'
        + '<path d="M15 25h14M15 31h12M15 37h7" stroke="url(#acc)" stroke-width="2.4" stroke-linecap="round"/>'
        + f'<path d="m47.5 47.5 7 7" stroke="{P["handle"]}" stroke-width="5" stroke-linecap="round"/>'
        + shadow(circle(39, 39, 11)) + glow(circle(39, 39, 11), 0.7) + f'<path d="{circle(39, 39, 11)}" fill="url(#inner)"/>'
        + f'<path d="{circle(39, 39, 9.5)}" fill="none" stroke="url(#acc)" stroke-width="4"/>' + f'<path d="{LENS}" {W} stroke-width="2" stroke-opacity=".9"/>'
        + sparkle(50, 14, 5, P["spark"]))
    out["free-space"] = svg(defs, body(circle(32, 32, 26)) + f'<path d="{circle(32, 32, 19.5)}" fill="url(#inner)"/>'
        + f'<path d="{circle(32, 32, 19.5)}" fill="none" stroke="#ffffff" stroke-opacity=".15"/>'
        + f'<path d="{TRACK}" fill="none" stroke="{P["track"]}" stroke-width="5" stroke-linecap="round"/>' + ARC.format(w=5)
        + NEEDLE.format(c="#ffffff", w=2.4) + f'<circle cx="32" cy="32" r="3.4" fill="url(#acc)"/><circle cx="32" cy="32" r="3.4" fill="none" stroke="#fff" stroke-opacity=".7" stroke-width=".8"/>')
    out["network-share"] = svg(defs, racks(10, (11, 26, 41), 44, 12))
    out["nearby-computer"] = svg(defs, laptop(0.76, 0.6, 13) + badge(46, 19, 10.5, wifi(46, 19)))
    out["nearby-server"] = svg(defs, racks(6, (22, 34, 46), 36, 10) + badge(46, 19, 10.5, wifi(46, 19)))
    out["cloud-drive"] = svg(defs, body(CLOUD_BACK, "inner", sheen=False, sh=False) + glow(CLOUD_FRONT, 0.4) + shadow(CLOUD_FRONT)
        + f'<path d="{CLOUD_FRONT}" fill="url(#acc)"/><path d="{CLOUD_FRONT}" fill="url(#sheen)"/><path d="{CLOUD_FRONT}" fill="none" stroke="url(#rim)" stroke-width=".9"/>'
        + body(circle(47, 46, 10.5), "body") + f'<path d="{SYNC_ARCS}" {W} stroke-width="2"/><path d="{SYNC_HEADS}" {W} stroke-width="2"/>')
    scr = rect(7.6, 11.6, 32.8, 21.8, 2.4)
    out["desktop"] = svg(defs, body(rect(21.5, 36, 6, 8, 1.5)) + body(rect(12.5, 43, 23, 5.5, 2.7))
        + body(rect(5, 9, 38, 28, 4.5), "inner") + glow(scr, 0.55) + f'<path d="{scr}" fill="url(#acc)"/>'
        + f'<path d="M7.6 11.6h20L19.4 33.4H7.6Z" fill="#ffffff" opacity=".16"/>'
        + body(rect(46, 12, 13, 41, 4)) + f'<path d="{rect(49.5, 17.5, 6, 2.4, 1.2)}" fill="{P["ink"]}" opacity=".5"/>'
        + f'<path d="M49.5 23.5h6M49.5 26.5h6" stroke="{P["line"]}" stroke-width="1.2" stroke-linecap="round" opacity=".55"/>' + led(52.5, 46, 1.7, P["dots"][0]))
    return out


_COMMON = dict(rim_t=0.55, rim_b=0.08, sheen=0.22, edge=None, a3="#1b3fb0", glow="#3d7bff", spark="#9fd0ff", storage="hex")


def midnight():
    return family(dict(_COMMON, b1="#2a3560", b2="#121931", a1="#74b8ff", a2="#2f69f2", c1="#9bd0ff", c2="#4d86ff",
                       in1="#0b1026", in2="#1b2442", line="#8d9bd0", ink="#05081a", track="#2a3458", handle="#6a8cff",
                       dots=["#4da3ff", "#4da3ff", "#4da3ff"]))


def aurora():
    return family(dict(_COMMON, b1="#2c2960", b2="#120f2e", a1="#5ee6ff", a2="#8a5cff", a3="#4a2bc4", c1="#ff8bd8", c2="#8a5cff",
                       glow="#7a5cff", spark="#ffd1f3", storage="stack", in1="#0d0b26", in2="#1e1946", line="#a89be8", ink="#05041a",
                       track="#2c2660", handle="#a78bff", dots=["#4de3ff", "#a78bff", "#ff6fd8"]))


def titanium():
    return family(dict(_COMMON, b1="#f5f7fb", b2="#a1aabb", rim_t=0.95, rim_b=0.3, sheen=0.5, edge=("#1b2238", 0.38), a1="#6fb6ff", a2="#2d6bf0",
                       c1="#9bd0ff", c2="#4d86ff", in1="#262d44", in2="#454e69", line="#5a6580", ink="#1d2336", track="#5a6482",
                       handle="#6b7590", storage="puck", dots=["#2bd49a", "#4da3ff", "#ffb04d"]))


SUITES = {"glass": glass, "clay": clay, "line": line, "neon": neon, "iso": iso, "tilt": tilt,
          "paper": paper, "pixelart": pixelart, "riso": riso, "blueprint": blueprint, "sticker": sticker,
          "midnight": midnight, "aurora": aurora, "titanium": titanium}

# Suites shipped in the app: generator name -> asset folder (SymbolStyle).
APP_SUITES = {"glass": "glass", "clay": "clay", "line": "line", "iso": "isometric", "tilt": "tilt-3d",
              "paper": "paper-cut", "pixelart": "pixel-art", "sticker": "sticker",
              "midnight": "midnight", "titanium": "titanium"}

if __name__ == "__main__":
    out = sys.argv[1]
    for name, make in SUITES.items():
        art = make()
        missing = set(NAMES) - set(art)
        assert not missing, f"{name} misses {missing}"
        os.makedirs(os.path.join(out, name), exist_ok=True)
        for key in NAMES:
            with open(os.path.join(out, name, f"{key}.svg"), "w") as f:
                f.write(art[key])
        fs = art["free-space"]
        for hook in ('M23.5 40.5a12 12 0 0 1 17-17', 'stroke="url(#rile-free-arc)"', 'd="M32 32 38.5 25.5"', "#5fd39a"):
            assert hook in fs, f"{name} free-space lacks hook {hook}"
    if "--app" in sys.argv:
        import shutil
        app = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../crates/rile-app/assets/overview")
        for suite, folder in APP_SUITES.items():
            os.makedirs(os.path.join(app, folder), exist_ok=True)
            for key in NAMES:
                shutil.copy(os.path.join(out, suite, f"{key}.svg"), os.path.join(app, folder, f"{key}.svg"))
    # One bundle for the review page (current set included for comparison).
    import json
    current = os.path.join(os.path.dirname(__file__), "../../crates/rile-app/assets/overview")
    bundle = {"current": {k: open(os.path.join(current, f"{k}.svg")).read() for k in NAMES}}
    for name in SUITES:
        bundle[name] = {k: open(os.path.join(out, name, f"{k}.svg")).read() for k in NAMES}
    with open(os.path.join(out, "symbols.js"), "w") as f:
        f.write("window.SYMBOLS = " + json.dumps(bundle) + ";\n")
    print(f"{len(SUITES)} suites x {len(NAMES)} symbols -> {out}")
