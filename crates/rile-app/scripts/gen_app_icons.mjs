#!/usr/bin/env node
// Writes the app icon artwork: assets/app-icons/<style>/<accent>.svg for every
// style × accent, plus src/app_icon_table.rs. Rasterise with gen_app_icon.sh.
// The designs come from mockups/app-icons/round-5 … round-7 and round-11 (27).
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

// Mirrors rile_core::settings::Accent::hue_sat.
const ACCENTS = [
  ["Blue", 212, 0.92], ["Purple", 268, 0.75], ["Pink", 330, 0.80], ["Red", 0, 0.80],
  ["Orange", 28, 0.95], ["Green", 142, 0.65], ["Teal", 178, 0.70], ["Graphite", 220, 0.08],
];

let H = 0, S = 0;
function hsl(h, s, l, a) {
  s = Math.min(1, s); l /= 100;
  const k = n => (n + h / 30) % 12;
  const f = n => l - s * Math.min(l, 1 - l) * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1));
  const [r, g, b] = [f(0), f(8), f(4)].map(v => Math.round(v * 255));
  return a === 1 ? `rgb(${r},${g},${b})` : `rgba(${r},${g},${b},${a})`;
}
// Accent tone at lightness l, saturation scaled by sMul, alpha a.
const A = (l, sMul = 1, a = 1) => hsl(H, S * sMul, l, a);
const stop = (o, c) => {
  const m = c.match(/^rgba\((.*),([\d.]+)\)$/);
  return m ? `<stop offset="${o}" stop-color="rgb(${m[1]})" stop-opacity="${m[2]}"/>` : `<stop offset="${o}" stop-color="${c}"/>`;
};

// macOS-style squircle tile: shadow, body fill, top gloss, hairline rim. Closes <defs>.
const tile = fill => `
  <filter id="sh" x="-20%" y="-20%" width="140%" height="150%"><feDropShadow dy="18" stdDeviation="18" flood-color="#070d18" flood-opacity=".22"/></filter>
  <linearGradient id="gl" x2="0" y2="1"><stop stop-color="#fff" stop-opacity=".22"/><stop offset=".5" stop-color="#fff" stop-opacity="0"/></linearGradient>
  </defs>
  <rect x="100" y="100" width="824" height="824" rx="184" fill="${fill}" filter="url(#sh)"/>
  <rect x="100" y="100" width="824" height="824" rx="184" fill="url(#gl)"/>
  <rect x="102" y="102" width="820" height="820" rx="182" fill="none" stroke="#fff" stroke-opacity=".2" stroke-width="3"/>`;
const lift = `<filter id="lf" x="-30%" y="-30%" width="160%" height="170%"><feDropShadow dy="14" stdDeviation="14" flood-color="#081321" flood-opacity=".28"/></filter>`;

// A shared glass-like finish, drawn LAST so the reflection covers the artwork
// as well as the tile. Keep it inside the existing macOS icon safe area.
const macFinish = `<defs>
  <clipPath id="mac-surface"><rect x="100" y="100" width="824" height="824" rx="184"/></clipPath>
  <linearGradient id="mac-reflection" x1=".15" y1="0" x2=".65" y2="1">
    <stop stop-color="#fff" stop-opacity=".24"/><stop offset=".55" stop-color="#e9f6ff" stop-opacity=".07"/><stop offset="1" stop-color="#fff" stop-opacity="0"/>
  </linearGradient>
  <radialGradient id="mac-glare" gradientUnits="userSpaceOnUse" cx="330" cy="100" r="600" gradientTransform="translate(0 45) scale(1 .55)">
    <stop stop-color="#fff" stop-opacity=".16"/><stop offset="1" stop-color="#fff" stop-opacity="0"/>
  </radialGradient>
  <linearGradient id="mac-bevel" x1=".3" y1="0" x2=".7" y2="1">
    <stop stop-color="#fff" stop-opacity=".72"/><stop offset=".27" stop-color="#e4efff" stop-opacity=".25"/><stop offset=".6" stop-color="#fff" stop-opacity=".03"/><stop offset="1" stop-color="#04112c" stop-opacity=".32"/>
  </linearGradient>
  <linearGradient id="mac-base" x2="0" y2="1">
    <stop offset=".65" stop-color="#06122b" stop-opacity="0"/><stop offset="1" stop-color="#06122b" stop-opacity=".09"/>
  </linearGradient>
  </defs>
  <g clip-path="url(#mac-surface)">
    <path d="M100 100H924V315C692 284 414 458 100 379Z" fill="url(#mac-reflection)"/>
    <rect x="100" y="100" width="824" height="824" fill="url(#mac-glare)"/>
    <rect x="100" y="100" width="824" height="824" fill="url(#mac-base)"/>
    <rect x="103" y="103" width="818" height="818" rx="181" fill="none" stroke="url(#mac-bevel)" stroke-width="6"/>
    <path d="M108 345V284C108 187 187 108 284 108H713" fill="none" stroke="#fff" stroke-opacity=".22" stroke-width="2" stroke-linecap="round"/>
  </g>`;

// Folio: an open folder with documents peeking out, on a light tile.
const folioDefs = (extra = "") => `<defs>
  <linearGradient id="t" x2=".2" y2="1"><stop stop-color="#fdfdfe"/><stop offset="1" stop-color="#e4e7ee"/></linearGradient>
  <linearGradient id="b" x2="0" y2="1">${stop(0, A(42))}${stop(1, A(34))}</linearGradient>
  <linearGradient id="f" x2=".15" y2="1">${stop(0, A(66))}${stop(1, A(50))}</linearGradient>
  ${lift}${extra}${tile("url(#t)")}`;
const FOLIO_FRONT = "M232 488q-4-36 32-36h496q36 0 32 36l-28 254q-3 30-34 30H294q-31 0-34-30z";
const folioBody = (deco = "") => `
  <path d="M262 300h150q22 0 36 17l26 31h290q34 0 34 34v356H228V334q0-34 34-34z" fill="url(#b)"/>
  <g filter="url(#lf)">
    <rect x="300" y="352" width="400" height="300" rx="18" fill="#fff" transform="rotate(-6 500 500)"/>
    <rect x="330" y="372" width="400" height="300" rx="18" fill="#f4f6fa" transform="rotate(4 530 520)"/>
  </g>
  <g fill="${A(55, 0.5, .35)}" transform="rotate(4 530 520)"><rect x="370" y="410" width="220" height="22" rx="11"/><rect x="370" y="452" width="300" height="22" rx="11"/></g>
  <path d="${FOLIO_FRONT}" fill="url(#f)" filter="url(#lf)"/>
  <path d="M262 468h500" stroke="#fff" stroke-opacity=".45" stroke-width="6" stroke-linecap="round"/>${deco}`;

// Drawer: an open file drawer with hanging folders, on a deep tile.
const drawerDefs = (extra = "") => `<defs>
  <linearGradient id="t" x2=".25" y2="1">${stop(0, A(30, .5))}${stop(1, A(16, .55))}</linearGradient>
  <linearGradient id="d" x2="0" y2="1">${stop(0, A(64))}${stop(1, A(48))}</linearGradient>
  ${lift}${extra}${tile("url(#t)")}`;
const drawerFolders = () => `
  <path d="M262 380h110q14 0 14 14v20h220V560H262z" fill="#dfe4ec"/>
  <path d="M300 330h110q14 0 14 14v20h292V560H300z" fill="${A(80, .7)}"/>
  <path d="M560 288h124q14 0 14 14v26h44V560H330V328h216q14 0 14-14z" fill="#fbfcfe"/>
  <g fill="${A(55, .5, .3)}"><rect x="378" y="380" width="200" height="20" rx="10"/><rect x="378" y="420" width="300" height="20" rx="10"/></g>`;
const drawerFront = deco => `
  <g filter="url(#lf)"><rect x="214" y="500" width="596" height="290" rx="54" fill="url(#d)"/></g>
  <path d="M268 506h488" stroke="#fff" stroke-opacity=".45" stroke-width="6" stroke-linecap="round"/>${deco}`;

const BOLT = "M18-100-62 12h52l-20 88 82-118h-54z";
const bolt = (cx, cy, k, fill, extra = "") =>
  `<path transform="translate(${cx} ${cy}) scale(${k})" d="${BOLT}" fill="${fill}" stroke="${fill}" stroke-width="10" stroke-linejoin="round" ${extra}/>`;

// Nebula Dossier artwork is shared with the approved concept 27.
// Its violet palette is preserved for Blue and shifted for the other accents.
const nebulaDossier = () => {
  const tint = hex => {
    const rgb = hex.match(/\w\w/g).map(v => parseInt(v, 16) / 255);
    const hi = Math.max(...rgb), lo = Math.min(...rgb), d = hi - lo;
    const l = (hi + lo) / 2;
    let h = hi === rgb[0] ? (rgb[1] - rgb[2]) / d : hi === rgb[1] ? (rgb[2] - rgb[0]) / d + 2 : (rgb[0] - rgb[1]) / d + 4;
    h = (h * 60 + 360 + H - 212) % 360;
    return hsl(h, d / (1 - Math.abs(2 * l - 1)) * S / .92, l * 100, 1);
  };
  return `<defs>
    <clipPath id="nc"><rect x="8" y="8" width="240" height="240" rx="53"/></clipPath>
    <radialGradient id="nn" cx=".75" cy=".3" r=".85">${stop(0, tint("b78be5"))}${stop(.35, tint("505697"))}${stop(1, tint("13233f"))}</radialGradient>
    <linearGradient id="nv" x2=".7" y2="1">${stop(0, tint("e8c9ff"))}${stop(.45, tint("a48cdb"))}${stop(1, tint("4c4b91"))}</linearGradient>
    <linearGradient id="ns" x2=".8" y2="1"><stop stop-color="#f4f9ff"/><stop offset=".45" stop-color="#becde3"/><stop offset="1" stop-color="#617998"/></linearGradient>
    <radialGradient id="ng"><stop stop-color="#87d8ff" stop-opacity=".5"/><stop offset="1" stop-color="#87d8ff" stop-opacity="0"/></radialGradient>
    <radialGradient id="nt" cx=".25" cy="0" r="1.1"><stop stop-color="#fff" stop-opacity=".15"/><stop offset=".5" stop-color="#fff" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity=".22"/></radialGradient>
    ${tile("url(#nn)")}
    <g transform="translate(72.533333 72.533333) scale(3.433333)"><g clip-path="url(#nc)">
      <rect width="256" height="256" fill="url(#nn)"/>
      <path d="M-10 53V11h83q12 0 19 11l15 22h159v222H-10Z" fill="url(#nv)"/>
      <path d="M31 35h165l26 26v154H31Z" fill="#0a1833" opacity=".4" transform="rotate(9 128 128) translate(0 7)"/>
      <path d="M31 35h165l26 26v154H31Z" fill="url(#ns)" transform="rotate(9 128 128)"/>
      <path d="M22 55h163l26 26v161H22Z" fill="url(#nn)" transform="rotate(-6 128 128)"/>
      <g transform="rotate(-6 128 128)"><path d="M185 55v26h26Z" fill="#b5bbeb"/><circle cx="141" cy="113" r="39" fill="url(#ng)"/><path d="m54 110 36-20 30 38 42-27" fill="none" stroke="#e1d6ff" stroke-opacity=".6" stroke-width="2"/><g fill="#f4eaff"><circle cx="54" cy="110" r="3"/><circle cx="90" cy="90" r="4"/><circle cx="120" cy="128" r="3"/><circle cx="162" cy="101" r="4"/></g></g>
      <path d="M-10 150q0-12 12-12h71q10 0 17 8l15 20h161v100H-10Z" fill="#182343" opacity=".4" transform="translate(0 7)"/>
      <path d="M-10 150q0-12 12-12h71q10 0 17 8l15 20h161v100H-10Z" fill="url(#nv)"/>
      <path d="M8 140h65q9 0 16 8l15 20h145" fill="none" stroke="#f1dcff" stroke-opacity=".8" stroke-width="2"/>
      <path d="M106 201h44" stroke="#4d457f" stroke-opacity=".5" stroke-width="7" stroke-linecap="round"/>
      <rect x="8" y="8" width="240" height="240" rx="53" fill="url(#nt)"/><rect x="9" y="9" width="238" height="238" rx="52" fill="none" stroke="#d7eaff" stroke-opacity=".23"/>
    </g></g>`;
};

// Order and slugs mirror rile_core::settings::AppIcon.
const STYLES = [
  ["FolioBolt", "folio-bolt", () => folioDefs() + folioBody(bolt(512, 620, 1.12, "#fff", `filter="url(#lf)"`))],
  ["DrawerBolt", "drawer-bolt", () => drawerDefs() + drawerFolders() + drawerFront(`
    ${bolt(512, 646, .98, A(26, .6, .55))}
    <path transform="translate(512 640) scale(.98)" d="${BOLT}" fill="#fff" fill-opacity=".92" stroke="#fff" stroke-opacity=".92" stroke-width="10" stroke-linejoin="round"/>`)],
  ["NebulaDossier", "nebula-dossier", nebulaDossier],
];

const arms = [];
for (const [variant, slug, draw] of STYLES) {
  mkdirSync(join(root, "assets/app-icons", slug), { recursive: true });
  for (const [accent, h, s] of ACCENTS) {
    [H, S] = [h, s];
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">${draw()}${macFinish}\n</svg>\n`;
    const file = `${slug}/${accent.toLowerCase()}`;
    writeFileSync(join(root, `assets/app-icons/${file}.svg`), svg);
    arms.push(`        (AppIcon::${variant}, Accent::${accent}) => include_bytes!("../assets/app-icons/${file}.png"),`);
  }
}
writeFileSync(join(root, "src/app_icon_table.rs"), `// Generated by scripts/gen_app_icons.mjs; do not edit.

/// The 512px PNG of an app icon style in an accent.
fn png_bytes(style: AppIcon, accent: Accent) -> &'static [u8] {
    match (style, accent) {
${arms.join("\n")}
    }
}
`);
console.log(`Wrote ${arms.length} icon SVGs and src/app_icon_table.rs`);
