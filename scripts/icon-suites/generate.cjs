// Generates candidate icon suites from the Lucide icons Rile uses.
// Usage: NODE_PATH=<dir with roughjs> node generate.cjs <lucide icons dir> <names.txt> <out dir>
// Pixel is produced separately by pixel.py (it needs a rasterizer).
const fs = require('fs');
const path = require('path');
const rough = require('roughjs');
const svgpath = require('svgpath');

const [srcDir, namesFile, outDir] = process.argv.slice(2);
const names = fs.readFileSync(namesFile, 'utf8').split('\n').map((s) => s.trim()).filter(Boolean);

// --- parsing -------------------------------------------------------------
function attrs(tag) {
  const out = {};
  for (const m of tag.matchAll(/([a-z-]+)="([^"]*)"/g)) out[m[1]] = m[2];
  return out;
}
function elements(svg) {
  const body = svg.replace(/^[\s\S]*?<svg[\s\S]*?>/, '').replace(/<\/svg>[\s\S]*$/, '');
  return [...body.matchAll(/<(path|circle|rect|line|ellipse|polyline|polygon)\b([\s\S]*?)\/>/g)].map((m) => ({
    tag: m[1],
    a: attrs(m[2]),
  }));
}
const n = (v, d = 0) => (v === undefined ? d : parseFloat(v));
// Path data for any element (used by the sketch style).
function toPath({ tag, a }) {
  switch (tag) {
    case 'path':
      return a.d;
    case 'line':
      return `M${a.x1} ${a.y1}L${a.x2} ${a.y2}`;
    case 'polyline':
    case 'polygon':
      return 'M' + a.points.trim().split(/[\s,]+/).reduce((s, v, i) => s + (i && i % 2 === 0 ? 'L' : ' ') + v, '') + (tag === 'polygon' ? 'Z' : '');
    case 'circle':
    case 'ellipse': {
      const cx = n(a.cx), cy = n(a.cy), rx = n(a.rx ?? a.r), ry = n(a.ry ?? a.r);
      return `M${cx - rx} ${cy}a${rx} ${ry} 0 1 0 ${2 * rx} 0a${rx} ${ry} 0 1 0 ${-2 * rx} 0Z`;
    }
    case 'rect': {
      const x = n(a.x), y = n(a.y), w = n(a.width), h = n(a.height);
      const r = Math.min(n(a.rx ?? a.ry), w / 2, h / 2);
      if (!r) return `M${x} ${y}h${w}v${h}h${-w}Z`;
      return `M${x + r} ${y}h${w - 2 * r}a${r} ${r} 0 0 1 ${r} ${r}v${h - 2 * r}a${r} ${r} 0 0 1 ${-r} ${r}h${-(w - 2 * r)}a${r} ${r} 0 0 1 ${-r} ${-r}v${-(h - 2 * r)}a${r} ${r} 0 0 1 ${r} ${-r}Z`;
    }
  }
}
function serialize({ tag, a }, extra = {}) {
  const all = { ...a, ...extra };
  return `<${tag} ${Object.entries(all).map(([k, v]) => `${k}="${v}"`).join(' ')}/>`;
}
const isDot = (el) => el.tag === 'path' && /^M[\d.\s-]+[hv]\.01$/i.test(el.a.d.trim());
const svg = (inner, attrs = '') =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"${attrs}>${inner}</svg>\n`;
const STROKE = 'fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"';

// --- styles ---------------------------------------------------------------
const styles = {
  // Heavy, friendly strokes, like a bold SF Symbol weight.
  bold(els) {
    return svg(`<g ${STROKE} stroke-width="2.6">${els.map((e) => serialize(e)).join('')}</g>`);
  },

  // Hand-drawn: each stroke re-traced with a slight wobble (seeded, stable).
  sketch(els, name) {
    const gen = rough.generator();
    let seed = [...name].reduce((s, c) => (s * 31 + c.charCodeAt(0)) >>> 0, 7) % 2 ** 31 || 1;
    const parts = els.map((e) => {
      if (isDot(e)) return serialize(e, { 'stroke-width': '2.4' });
      const d = gen.path(svgpath(toPath(e)).abs().unarc().round(3).toString(), {
        seed: seed++,
        roughness: 1.1,
        bowing: 1.4,
        maxRandomnessOffset: 1.1,
        strokeWidth: 1.6,
        preserveVertices: true,
        disableMultiStroke: false,
        curveFitting: 0.98,
      });
      return gen.toPaths(d).map((p) => `<path d="${p.d}"/>`).join('');
    });
    return svg(`<g ${STROKE} stroke-width="1.5">${parts.join('')}</g>`);
  },

  // Hollow "tube" lines: a thick stroke with a thin channel cut out of it.
  neon(els, name) {
    const id = `t-${name}`;
    const body = els.map((e) => serialize(e)).join('');
    return svg(
      `<mask id="${id}" maskUnits="userSpaceOnUse" x="-2" y="-2" width="28" height="28">` +
        `<rect x="-2" y="-2" width="28" height="28" fill="black"/>` +
        `<g ${STROKE.replace('currentColor', 'white')} stroke-width="3.1">${body}</g>` +
        `<g ${STROKE.replace('currentColor', 'black')} stroke-width="1.1">${els.filter((e) => !isDot(e)).map((e) => serialize(e)).join('')}</g>` +
        `</mask><rect x="-2" y="-2" width="28" height="28" fill="currentColor" mask="url(#${id})"/>`,
    );
  },

  // The glyph knocked out of a solid rounded tile.
  badge(els, name) {
    const id = `b-${name}`;
    const body = els.map((e) => serialize(e)).join('');
    return svg(
      `<mask id="${id}" maskUnits="userSpaceOnUse" x="0" y="0" width="24" height="24">` +
        `<rect x="1" y="1" width="22" height="22" rx="6" fill="white"/>` +
        `<g transform="translate(12 12) scale(0.64) translate(-12 -12)" ${STROKE.replace('currentColor', 'black')} stroke-width="2.6">${body}</g>` +
        `</mask><rect x="1" y="1" width="22" height="22" rx="6" fill="currentColor" mask="url(#${id})"/>`,
    );
  },
};

for (const style of Object.keys(styles)) fs.mkdirSync(path.join(outDir, style), { recursive: true });
for (const name of names) {
  const els = elements(fs.readFileSync(path.join(srcDir, `${name}.svg`), 'utf8'));
  for (const [style, make] of Object.entries(styles)) {
    fs.writeFileSync(path.join(outDir, style, `${name}.svg`), make(els, name));
  }
}
console.log(`${names.length} icons x ${Object.keys(styles).length} styles`);
