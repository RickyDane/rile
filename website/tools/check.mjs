// npm install --prefix /tmp/rile-website-check --no-audit --no-fund linkedom
// node website/tools/check.mjs
import { createRequire } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve, dirname } from 'node:path';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const require = createRequire('/tmp/rile-website-check/package.json');
const { parseHTML } = require('linkedom');
const base = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const html = readFileSync(resolve(base, 'index.html'), 'utf8');
const source = readFileSync(resolve(base, 'js/main.js'), 'utf8');

for (const match of html.matchAll(/(?:src|href|content)="((?:assets|css|js)\/[^"#]+)"/g)) {
  assert.ok(existsSync(resolve(base, match[1])), `Missing asset: ${match[1]}`);
}
for (const platform of ['Macintosh', 'Windows NT 10.0', 'Linux', 'iPhone', 'Linux; Android']) {
 for (const reducedMotion of [false, true]) {
  const { document, Event } = parseHTML(html);
  const $ = selector => document.querySelector(selector);
  const $$ = selector => [...document.querySelectorAll(selector)];
  $('#stars').getContext = () => null;
  let focused = null;
  Object.defineProperty(document, 'activeElement', { get: () => focused });
  const prototype = document.defaultView.HTMLElement.prototype;
  prototype.focus = function () { focused = this; };
  prototype.select = function () { this.dataset.selected = 'true'; };
  prototype.getBoundingClientRect = () => ({ top: 0, bottom: 800, left: 0, right: 1280 });
  const box = $('[data-lightbox]');
  box.showModal = () => box.setAttribute('open', '');
  box.close = () => { box.removeAttribute('open'); box.dispatchEvent(new Event('close')); };
  const timers = [];
  const context = {
    document, navigator: { userAgent: platform, clipboard: { writeText: async () => {} } },
    matchMedia: query => ({ matches: query.includes('reduced-motion') ? reducedMotion : true, addEventListener() {} }),
    innerWidth: 1280, innerHeight: 800, devicePixelRatio: 1, scrollY: 0,
    addEventListener() {}, requestAnimationFrame: () => 1, cancelAnimationFrame() {},
    setTimeout: callback => { timers.push(callback); return timers.length; }, clearTimeout() {},
  };
  context.window = context;
  vm.runInNewContext(source, context);
  assert.ok(document.documentElement.classList.contains('is-ready'));
  assert.ok(document.documentElement.hasAttribute('data-enhanced'));
  assert.equal($('.hero .announce'), null, 'Hero announcement pill returned');
  const bubbles = $$('[data-bubbles] circle').map(circle => ({
    x: +circle.getAttribute('cx'), y: +circle.getAttribute('cy'), r: +circle.getAttribute('r'),
  }));
  assert.equal(bubbles.length, 7);
  bubbles.forEach((circle, index) => {
    assert.ok(circle.x - circle.r >= 3 && circle.x + circle.r <= 297);
    assert.ok(circle.y - circle.r >= 4 && circle.y + circle.r <= 176);
    bubbles.slice(index + 1).forEach(other => assert.ok(
      Math.hypot(circle.x - other.x, circle.y - other.y) - circle.r - other.r >= 10,
      'Bubble spacing must exceed maximum relative float movement',
    ));
  });
  assert.ok($$('[data-reveal]').every(el => el.classList.contains('is-in')));
  $('[data-accent="teal"]').click();
  $('[data-icon="folio-bolt"]').click();
  assert.equal($('[data-icon="folio-bolt"]').getAttribute('aria-checked'), 'true');
  assert.ok($$('[data-app-icon]').every(img => img.getAttribute('src') === 'assets/icons/folio-bolt-teal.png'));
  for (const accent of $$('[data-accent]')) {
    accent.click();
    for (const icon of $$('[data-icon]')) {
      icon.click();
      assert.ok(existsSync(resolve(base, $('[data-app-icon]').getAttribute('src'))));
    }
  }
  const key = (target, name) => {
    const event = new Event('keydown', { bubbles: true, cancelable: true });
    event.key = name; target.dispatchEvent(event);
  };
  $('[data-accent="graphite"]').focus();
  key(document.activeElement, 'ArrowRight');
  assert.equal($('[data-accent="blue"]').getAttribute('aria-checked'), 'true');
  $('[data-shot="dual"][role="tab"]').click();
  assert.equal($('[role="tab"][aria-selected="true"]').dataset.shot, 'dual');
  assert.equal($$('.tour-stage img[tabindex="0"]').length, 1);
  $('[role="tab"][aria-selected="true"]').focus();
  key(document.activeElement, 'End');
  assert.equal($('[role="tab"][aria-selected="true"]').dataset.shot, 'home');
  key(document.activeElement, 'Home');
  assert.equal($('[role="tab"][aria-selected="true"]').dataset.shot, 'columns');
  const shot = $('.tour-stage img.is-active');
  shot.click(); assert.ok(box.hasAttribute('open'));
  $('[data-close]').click(); assert.ok(!box.hasAttribute('open'));
  assert.equal(document.activeElement, shot);
  $('[data-menu-toggle]').click();
  assert.equal($('[data-menu-toggle]').getAttribute('aria-expanded'), 'true');
  key($('[data-menu-toggle]'), 'Escape');
  assert.equal($('[data-menu-toggle]').getAttribute('aria-expanded'), 'false');
  $('[data-menu-toggle]').click(); $('.nav-links a').click();
  assert.equal($('[data-menu-toggle]').getAttribute('aria-expanded'), 'false');
  $('[data-copy]').click();
  await new Promise(setImmediate);
  assert.match($('.toast').textContent, /copied/);
  context.navigator.clipboard.writeText = async () => { throw new Error('Permission denied'); };
  $('[data-copy]').click(); await new Promise(setImmediate);
  assert.equal($('.command-fallback').value, $('[data-copy]').dataset.copy);
  assert.equal($('.command-fallback').dataset.selected, 'true');
  const mobile = /iPhone|Android/.test(platform);
  assert.equal($$('[data-platform].is-recommended').length, mobile ? 0 : 1);
  if (platform.includes('Windows')) assert.match($('[data-primary-download]').href, /x64-setup\.exe$/);
  for (const link of $$('a[href^="#"]')) assert.ok($(link.getAttribute('href')), `Broken anchor: ${link.href}`);
  console.log(`${platform} / ${reducedMotion ? 'reduced' : 'full'} motion: assets, menu, keyboard, tabs, all icons, zoom, clipboard success/denial, platform passed`);
 }
}
