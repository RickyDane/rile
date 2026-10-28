#!/usr/bin/env node
// Writes the app icon artwork: assets/app-icons/<style>/<accent>.svg for every
// style × accent, plus src/app_icon_table.rs. Rasterise with gen_app_icon.sh.
// The designs come from mockups/app-icons/round-5 … round-7.
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
const handle = () => `<rect x="402" y="612" width="220" height="56" rx="28" fill="${A(26, .6, .55)}"/>
  <rect x="418" y="622" width="188" height="16" rx="8" fill="#fff" fill-opacity=".25"/>`;
const drawerFront = deco => `
  <g filter="url(#lf)"><rect x="214" y="500" width="596" height="290" rx="54" fill="url(#d)"/></g>
  <path d="M268 506h488" stroke="#fff" stroke-opacity=".45" stroke-width="6" stroke-linecap="round"/>${deco}`;

const BOLT = "M18-100-62 12h52l-20 88 82-118h-54z";
const bolt = (cx, cy, k, fill, extra = "") =>
  `<path transform="translate(${cx} ${cy}) scale(${k})" d="${BOLT}" fill="${fill}" stroke="${fill}" stroke-width="10" stroke-linejoin="round" ${extra}/>`;

const FLAME_OUT = "M-4-122C10-86 30-68 34-38C42-56 54-64 64-76C64-40 82-18 82 24C82 76 46 112 0 112C-46 112-82 78-82 30C-82-10-62-32-52-64C-44-42-36-32-26-22C-30-62-24-98-4-122Z";
const FLAME_IN = "M-2-22C12 4 42 26 42 62C42 90 24 106 0 106C-24 106-42 90-42 64C-42 42-28 28-20 8C-14 22-8 30-2 34C-8 14-8-4-2-22Z";
const flame = (cx, cy, k, outer, inner, extra = "") =>
  `<g transform="translate(${cx} ${cy}) scale(${k})" ${extra}><path d="${FLAME_OUT}" fill="${outer}"/><path d="${FLAME_IN}" fill="${inner}"/></g>`;
const emberDefs = `<linearGradient id="eo" x2=".2" y2="1"><stop stop-color="#ffc247"/><stop offset="1" stop-color="#f0491a"/></linearGradient>
  <linearGradient id="ei" x2="0" y2="1"><stop stop-color="#fff2a8"/><stop offset="1" stop-color="#ffc93a"/></linearGradient>
  <filter id="eg" x="-80%" y="-80%" width="260%" height="260%"><feDropShadow stdDeviation="24" flood-color="#ff7a1f" flood-opacity=".7"/></filter>`;
const ember = (cx, cy, k) => flame(cx, cy, k, "url(#eo)", "url(#ei)", `filter="url(#eg)"`);
// Hot-rod flame decal licking back from the right edge, clipped to the panel.
const HOTROD = "M840 512C710 514 600 534 462 560C560 576 616 588 664 600C540 612 420 640 296 662C430 672 540 680 624 692C520 704 404 732 276 764L840 810Z";
const hotrod = (clipShape, dy = 0) => `<defs><clipPath id="hc">${clipShape}</clipPath></defs>
  <g clip-path="url(#hc)" transform="translate(0 ${dy})"><path d="${HOTROD}" fill="url(#eo)"/>
  <path d="${HOTROD}" fill="url(#ei)" transform="translate(840 700) scale(.62 .5) translate(-840 -640)"/></g>`;

// Order and slugs mirror rile_core::settings::AppIcon.
const STYLES = [
  ["FolioBolt", "folio-bolt", () => folioDefs() + folioBody(bolt(512, 620, 1.12, "#fff", `filter="url(#lf)"`))],
  ["DrawerBolt", "drawer-bolt", () => drawerDefs() + drawerFolders() + drawerFront(`
    ${bolt(512, 646, .98, A(26, .6, .55))}
    <path transform="translate(512 640) scale(.98)" d="${BOLT}" fill="#fff" fill-opacity=".92" stroke="#fff" stroke-opacity=".92" stroke-width="10" stroke-linejoin="round"/>`)],
  ["FolioFlame", "folio-flame", () => folioDefs() + folioBody(flame(512, 606, 1.1, "#fff", A(76, .7), `filter="url(#lf)"`))],
  ["FolioEmber", "folio-ember", () => folioDefs(emberDefs) + folioBody(ember(512, 606, 1.1))],
  ["FolioHotRod", "folio-hot-rod", () => folioDefs(emberDefs) + folioBody(hotrod(`<path d="${FOLIO_FRONT}"/>`))],
  ["DrawerFlame", "drawer-flame", () => drawerDefs() + drawerFolders() + drawerFront(`
    ${flame(512, 650, .98, A(26, .6, .5), A(26, .6, .5))}
    ${flame(512, 644, .98, "#fff", A(76, .7))}`)],
  ["DrawerEmber", "drawer-ember", () => drawerDefs(emberDefs) + drawerFolders() + drawerFront(ember(512, 644, .98))],
  ["DrawerHotRod", "drawer-hot-rod", () => drawerDefs(emberDefs) + drawerFolders() +
    drawerFront(hotrod(`<rect x="214" y="500" width="596" height="290" rx="54"/>`, 14) + handle())],
];

const arms = [];
for (const [variant, slug, draw] of STYLES) {
  mkdirSync(join(root, "assets/app-icons", slug), { recursive: true });
  for (const [accent, h, s] of ACCENTS) {
    [H, S] = [h, s];
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">${draw()}\n</svg>\n`;
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
