// Execute this function with Winfer's browser_run_code_unsafe filename option.
// Uses the existing Winfer tab; does not launch a browser.
async (page) => {
  const check = (condition, message) => { if (!condition) throw new Error(message); };
  const errors = [];
  const onError = error => errors.push(error.message);
  page.on('pageerror', onError);
  const results = [];
  const previousViewport = page.viewportSize();
  try {
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.reload();
    await page.waitForFunction(() => document.documentElement.hasAttribute('data-enhanced'));
    for (const width of [320, 360, 390, 768, 1024, 1280, 1920]) {
      await page.setViewportSize({ width, height: 900 });
      // Sample moving decoration as well as the settled layout.
      for (let sample = 0; sample < 4; sample++) {
        await page.waitForTimeout(150);
        const layout = await page.evaluate(() => ({
          width: innerWidth, scroll: document.documentElement.scrollWidth,
          navHeight: document.querySelector('.nav-inner').getBoundingClientRect().height,
          cardRadius: parseFloat(getComputedStyle(document.querySelector('.platform')).borderRadius),
          cardPosition: getComputedStyle(document.querySelector('.platform')).position,
          illustration: document.querySelector('.va-items i').getBoundingClientRect().width,
          ringsFixed: [...document.querySelectorAll('.orbit-ring')].every(ring => getComputedStyle(ring).rotate === 'none' && ring.getAnimations().length === 0),
          gridFilled: (() => {
            const grid = document.querySelector('.bento');
            const width = grid.clientWidth;
            const rows = new Map();
            [...grid.children].forEach(card => {
              const rect = { width: card.offsetWidth };
              const key = card.offsetTop;
              const row = rows.get(key) || [];
              row.push(rect); rows.set(key, row);
            });
            return [...rows.values()].every(row => Math.abs(row.reduce((sum, rect) => sum + rect.width, 0) + (row.length - 1) * 18 - width) < 2);
          })(),
        }));
        check(layout.scroll <= layout.width, `Horizontal overflow at ${width}px: ${layout.scroll}`);
        check(layout.navHeight === 60, `Navigation wrapped at ${width}px`);
        check(layout.cardRadius > 0 && layout.cardPosition === 'relative', 'Download card lost its styling');
        check(layout.illustration > 0, 'View illustration is blank');
        check(layout.ringsFixed, 'Hero orbit tracks are rotating');
        check(layout.gridFilled, `Feature grid has an empty slot at ${width}px`);
      }
      results.push(`${width}px: layout passed`);
    }
    await page.setViewportSize({ width: 1440, height: 1000 });
    const tracks = await page.evaluate(() => [...document.querySelectorAll('.orbit-ring')].map(ring => {
      const dot = ring.querySelector('span');
      const animation = dot.getAnimations()[0];
      const duration = animation.effect.getTiming().duration;
      animation.pause();
      const samples = [0, .25, .5, .75].map(fraction => {
        animation.currentTime = fraction * duration;
        const r = ring.getBoundingClientRect(), d = dot.getBoundingClientRect();
        const x = d.left + d.width / 2, y = d.top + d.height / 2;
        return { x, y, onTrack: Math.abs(((x - r.left - r.width / 2) / (r.width / 2)) ** 2 +
          ((y - r.top - r.height / 2) / (r.height / 2)) ** 2 - 1) < .02 };
      });
      animation.play();
      return samples.every(sample => sample.onTrack) && Math.hypot(samples[0].x - samples[1].x, samples[0].y - samples[1].y) > 20;
    }));
    check(tracks.every(Boolean), 'Orbit dots do not follow the fixed elliptical tracks');
    check(await page.locator('.hero .announce').count() === 0, 'Hero announcement pill returned');
    await page.locator('.card--analyzer').scrollIntoViewIfNeeded();
    await page.waitForTimeout(1800);
    const bubbleGap = await page.evaluate(() => {
      const circles = [...document.querySelectorAll('[data-bubbles] circle')];
      const animations = circles.flatMap(circle => circle.getAnimations()).filter(animation => animation.animationName === 'bubble-float');
      animations.forEach(animation => animation.pause());
      let minimum = Infinity;
      for (let time = 0; time <= 12000; time += 250) {
        animations.forEach(animation => { animation.currentTime = time; });
        const bounds = circles.map(circle => circle.getBoundingClientRect());
        bounds.forEach((a, index) => bounds.slice(index + 1).forEach(b => {
          minimum = Math.min(minimum, Math.hypot(a.x + a.width / 2 - b.x - b.width / 2,
            a.y + a.height / 2 - b.y - b.height / 2) - (a.width + b.width) / 2);
        }));
      }
      animations.forEach(animation => animation.play());
      return minimum;
    });
    check(bubbleGap > 1, `Space Analyzer bubbles overlap during animation: ${bubbleGap}px`);
    results.push('Hero pill removed; animated Space Analyzer bubbles remain separated: passed');
    check(await page.evaluate(() => getComputedStyle(document.querySelector('.launch-sky')).maskImage !== 'none'), 'Download glow lost its soft fade');
    results.push('Fixed elliptical orbit tracks, moving dots, filled feature rows and soft download fade: passed');
    for (const width of [320, 768, 1440]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.evaluate(() => {
        const rect = document.querySelector('[data-orbits]').getBoundingClientRect();
        scrollTo({ top: scrollY + rect.top - 120, behavior: 'instant' });
      });
      await page.waitForTimeout(1000);
      const devices = await page.evaluate(() => [...document.querySelectorAll('.orbit')].map(orbit => {
        const sat = orbit.querySelector('.sat');
        const motion = orbit.getAnimations()[0], counter = sat.getAnimations()[0];
        motion.pause(); counter.pause();
        const duration = motion.effect.getTiming().duration;
        const style = getComputedStyle(orbit);
        const tip = parseFloat(style.getPropertyValue('--trail-angle'));
        const gradient = getComputedStyle(orbit, '::before').backgroundImage;
        const trailsBehind = (style.animationDirection === 'reverse'
          ? / 0%,.+ 30%\)$/ : / 70%,.+ 100%\)$/).test(gradient);
        let error = 0, upright = true;
        for (let phase = 0; phase < 16; phase++) {
          motion.currentTime = counter.currentTime = phase * duration / 16;
          const track = orbit.getBoundingClientRect(), device = sat.getBoundingClientRect();
          const angle = (tip + parseFloat(getComputedStyle(orbit).rotate)) * Math.PI / 180;
          const radius = (orbit.offsetWidth - 2) / 2;
          const x = track.x + track.width / 2 + radius * Math.sin(angle);
          const y = track.y + track.height / 2 - radius * Math.cos(angle);
          error = Math.max(error, Math.hypot(device.x + device.width / 2 - x, device.y + device.height / 2 - y));
          upright &&= Math.abs(device.width - sat.offsetWidth) < 1 && Math.abs(device.height - sat.offsetHeight) < 1;
        }
        motion.play(); counter.play();
        return { name: sat.textContent.trim(), error, upright, trailsBehind };
      }));
      devices.forEach(device => check(device.error < 2 && device.upright && device.trailsBehind,
        `Device/trail mismatch at ${width}px: ${JSON.stringify(device)}`));
    }
    results.push('All three device trails: attached, correctly directed and upright at 16 phases × 3 widths');
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.reload();
    for (let y = 0; y < await page.evaluate(() => document.documentElement.scrollHeight); y += 700) {
      await page.evaluate(top => scrollTo({ top, behavior: 'instant' }), y);
    }
    await page.waitForTimeout(300);
    await page.waitForFunction(() => [...document.images].filter(img => img.getAttribute('src')).every(img => img.complete && img.naturalWidth));
    check(await page.evaluate(() => document.getAnimations().every(animation => animation.playState !== 'running')), 'Reduced motion still animates');
    await page.addScriptTag({ path: '/tmp/rile-website-check/node_modules/axe-core/axe.min.js' });
    const violations = await page.evaluate(async () => (await axe.run(document, {
      runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21aa'] },
    })).violations.map(item => ({ id: item.id, targets: item.nodes.map(node => node.target) })));
    check(!violations.length, JSON.stringify(violations));
    results.push('Reduced motion, image loading and axe WCAG A/AA: passed');
    const cdp = await page.context().newCDPSession(page);
    try {
      await cdp.send('Emulation.setScriptExecutionDisabled', { value: true });
      await page.setViewportSize({ width: 320, height: 900 });
      await page.reload();
      const fallback = await page.evaluate(() => ({
        noJS: !document.documentElement.classList.contains('js'),
        scroll: document.documentElement.scrollWidth,
        nav: getComputedStyle(document.querySelector('.nav-links')).display,
        images: [...document.querySelectorAll('.tour-stage img')].every(img => getComputedStyle(img).opacity === '1'),
      }));
      check(fallback.noJS && fallback.scroll === 320 && fallback.nav === 'flex' && fallback.images, 'No-JavaScript fallback failed');
      results.push('JavaScript disabled at 320px: navigation and all tour images passed');
    } finally {
      await cdp.send('Emulation.setScriptExecutionDisabled', { value: false });
      await cdp.detach();
    }
    check(!errors.length, `Page errors: ${errors.join('; ')}`);
    return results;
  } finally {
    page.off('pageerror', onError);
    await page.emulateMedia({ reducedMotion: null });
    if (previousViewport) await page.setViewportSize(previousViewport);
    await page.reload();
    await page.evaluate(() => scrollTo({ top: 0, behavior: 'instant' }));
  }
}
