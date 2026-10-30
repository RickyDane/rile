const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
const root = document.documentElement;
root.classList.add('js');
const motion = matchMedia('(prefers-reduced-motion: reduce)');
const finePointer = matchMedia('(hover: hover) and (pointer: fine)');

const nav = $('[data-nav]');
const menuToggle = $('[data-menu-toggle]');
function setMenu(open) {
  nav.classList.toggle('is-menu-open', open);
  menuToggle.setAttribute('aria-expanded', String(open));
  menuToggle.setAttribute('aria-label', open ? 'Close navigation' : 'Open navigation');
}
menuToggle.addEventListener('click', () => setMenu(menuToggle.getAttribute('aria-expanded') !== 'true'));
$$('.nav-links a').forEach(link => link.addEventListener('click', () => setMenu(false)));
document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && menuToggle.getAttribute('aria-expanded') === 'true') {
    setMenu(false); menuToggle.focus();
  }
});
document.addEventListener('click', e => { if (!nav.contains(e.target)) setMenu(false); });
nav.addEventListener('focusout', e => { if (!nav.contains(e.relatedTarget)) setMenu(false); });
matchMedia('(max-width: 1080px)').addEventListener('change', () => setMenu(false));

// Keep content visible even when animation APIs are unavailable.
$$('[data-intro]').forEach(el => el.style.setProperty('--i', el.dataset.intro));
root.classList.add('is-ready');
const reveal = 'IntersectionObserver' in window && new IntersectionObserver(entries => {
  entries.forEach(entry => {
    if (!entry.isIntersecting) return;
    entry.target.classList.add('is-in');
    reveal.unobserve(entry.target);
  });
}, { threshold: 0.08 });
$$('[data-reveal]').forEach(el => reveal ? reveal.observe(el) : el.classList.add('is-in'));

// One lightweight canvas, capped for high-density displays. No WebGL required.
const canvas = $('#stars');
const ctx = canvas.getContext('2d');
let stars = [], frame = 0, width = 0, height = 0;
function resizeSky() {
  width = innerWidth; height = innerHeight;
  const dpr = Math.min(devicePixelRatio || 1, 2);
  canvas.width = width * dpr; canvas.height = height * dpr;
  ctx?.setTransform(dpr, 0, 0, dpr, 0, 0);
  stars = Array.from({ length: Math.min(210, Math.floor(width * height / 6000)) }, () => ({
    x: Math.random() * width, y: Math.random() * height,
    r: Math.random() * 1.05 + 0.2, phase: Math.random() * Math.PI * 2,
  }));
  drawSky(0);
}
function drawSky(time) {
  if (!ctx) return;
  ctx.clearRect(0, 0, width, height);
  stars.forEach(star => {
    ctx.fillStyle = `rgba(205,215,255,${0.25 + (Math.sin(time / 2400 + star.phase) + 1) * 0.25})`;
    ctx.beginPath(); ctx.arc(star.x, star.y, star.r, 0, Math.PI * 2); ctx.fill();
  });
}
function animate(time) {
  drawSky(time);
  frame = requestAnimationFrame(animate);
}
function updateMotion() {
  cancelAnimationFrame(frame);
  if (!motion.matches && !document.hidden) frame = requestAnimationFrame(animate);
  else drawSky(0);
}
resizeSky(); updateMotion();
addEventListener('resize', resizeSky, { passive: true });
document.addEventListener('visibilitychange', updateMotion);
motion.addEventListener('change', updateMotion);

let scrollPending = false;
function updateScroll() {
  scrollPending = false;
  $('[data-nav]').classList.toggle('is-scrolled', scrollY > 24);
  if (motion.matches) return;
  const stage = $('[data-hero-stage]');
  const progress = Math.min(1, Math.max(0, scrollY / 800));
  const hero = $('.window--hero');
  if (stage.getBoundingClientRect().bottom > 0) {
    hero.style.setProperty('--tilt', `${24 * (1 - progress)}deg`);
    hero.style.setProperty('--scale', 0.9 + progress * 0.1);
  }
}
addEventListener('scroll', () => {
  if (!scrollPending) { scrollPending = true; requestAnimationFrame(updateScroll); }
}, { passive: true });
updateScroll();
if ('IntersectionObserver' in window) {
  const sections = new IntersectionObserver(entries => entries.forEach(entry => {
    if (entry.isIntersecting) $$('.nav-links a').forEach(a => {
      const active = a.hash === `#${entry.target.id}`;
      a.classList.toggle('is-active', active);
      if (active) a.setAttribute('aria-current', 'location');
      else a.removeAttribute('aria-current');
    });
  }), { rootMargin: '-20% 0px -55% 0px' });
  $$('main section[id]').forEach(section => sections.observe(section));
}
$$('[data-spotlight]').forEach(card => card.addEventListener('pointermove', e => {
  if (!finePointer.matches || motion.matches) return;
  const bounds = card.getBoundingClientRect();
  card.style.setProperty('--mx', `${e.clientX - bounds.left}px`);
  card.style.setProperty('--my', `${e.clientY - bounds.top}px`);
}));

// Tabs do not auto-advance: readers and keyboard users control the tour.
function selectTab(tab) {
  const tabs = $$('[role="tab"]', $('[data-tour]'));
  tabs.forEach(button => {
    const active = button === tab;
    button.setAttribute('aria-selected', String(active));
    button.tabIndex = active ? 0 : -1;
  });
  $$('.tour-stage img').forEach(img => {
    const active = img.dataset.shot === tab.dataset.shot;
    img.classList.toggle('is-active', active);
    img.setAttribute('aria-hidden', String(!active));
    img.tabIndex = active ? 0 : -1;
  });
  $('#tour-panel').setAttribute('aria-labelledby', tab.id);
}
function arrowGroup(container, selector, activate) {
  const buttons = $$(selector, container);
  container.addEventListener('keydown', e => {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return;
    const index = buttons.indexOf(document.activeElement);
    if (index < 0) return;
    e.preventDefault();
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? buttons.length - 1 :
      (index + (['ArrowLeft', 'ArrowUp'].includes(e.key) ? -1 : 1) + buttons.length) % buttons.length;
    activate(buttons[next]); buttons[next].focus({ preventScroll: true });
    // Keep horizontal mobile tab strips in view without moving the page vertically.
    const bounds = buttons[next].getBoundingClientRect();
    const strip = buttons[next].parentElement;
    const stripBounds = strip.getBoundingClientRect();
    if (bounds.left < stripBounds.left) strip.scrollLeft -= stripBounds.left - bounds.left;
    else if (bounds.right > stripBounds.right) strip.scrollLeft += bounds.right - stripBounds.right;
  });
}
const tour = $('[data-tour]');
tour.classList.add('is-paused');
$$('[role="tab"]', tour).forEach(tab => tab.addEventListener('click', () => selectTab(tab)));
arrowGroup(tour, '[role="tab"]', selectTab);
selectTab($('[role="tab"][aria-selected="true"]', tour));

let accent = 'purple', icon = 'nebula';
const accents = { blue: [212, 92], purple: [268, 75], pink: [330, 80], red: [0, 80],
  orange: [28, 95], green: [142, 65], teal: [178, 70], graphite: [220, 8] };
function customize(button) {
  if (button.dataset.accent) accent = button.dataset.accent;
  if (button.dataset.icon) icon = button.dataset.icon;
  root.style.setProperty('--acc-h', accents[accent][0]);
  root.style.setProperty('--acc-s', accents[accent][1]);
  $$('[data-app-icon]').forEach(img => img.src = `assets/icons/${icon}-${accent}.png`);
  $$('[data-accents] button, [data-icons] button').forEach(el => {
    const selected = el.dataset.accent ? el.dataset.accent === accent : el.dataset.icon === icon;
    el.setAttribute('aria-checked', String(selected)); el.tabIndex = selected ? 0 : -1;
  });
}
$$('[data-accents], [data-icons]').forEach(group => {
  $$('button', group).forEach(button => button.addEventListener('click', () => customize(button)));
  arrowGroup(group, 'button', customize);
});
customize($('[data-accent="purple"]'));

const marquee = $('[data-marquee]');
[...marquee.children].forEach(item => {
  const copy = item.cloneNode(true);
  copy.setAttribute('aria-hidden', 'true'); marquee.append(copy);
});
$$('.sa-results li, .ask-steps li').forEach((row, index) => {
  row.style.transitionDelay = `${(index % 3) * 150}ms`;
  row.classList.add('show');
});

const lightbox = $('[data-lightbox]');
let previousFocus;
function closeLightbox() { lightbox.close(); }
$$('[data-zoom]').forEach(img => {
  img.tabIndex = 0;
  img.setAttribute('role', 'button');
  img.setAttribute('aria-label', `Enlarge screenshot: ${img.alt}`);
  img.setAttribute('aria-haspopup', 'dialog');
  const open = () => {
    previousFocus = img;
    const large = $('img', lightbox); large.src = img.src; large.alt = img.alt;
    lightbox.showModal(); document.body.style.overflow = 'hidden';
  };
  img.addEventListener('click', open);
  img.addEventListener('keydown', e => {
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); }
  });
});
// Initialize inactive tour images after assigning lightbox keyboard controls.
selectTab($('[role="tab"][aria-selected="true"]', tour));
$('[data-close]').addEventListener('click', closeLightbox);
lightbox.addEventListener('click', e => { if (e.target === lightbox) closeLightbox(); });
lightbox.addEventListener('close', () => {
  document.body.style.overflow = ''; previousFocus?.focus({ preventScroll: true });
});

const status = document.createElement('p');
status.className = 'toast'; status.setAttribute('role', 'status');
status.setAttribute('aria-live', 'polite'); document.body.append(status);
let toastTimer;
function toast(message) {
  status.textContent = message; status.classList.add('is-visible');
  clearTimeout(toastTimer); toastTimer = setTimeout(() => status.classList.remove('is-visible'), 3500);
}
$$('[data-copy]').forEach(button => button.addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText(button.dataset.copy);
    button.classList.add('is-copied'); toast('Install command copied to clipboard.');
    setTimeout(() => button.classList.remove('is-copied'), 2500);
  } catch {
    let field = button.parentElement.querySelector('.command-fallback');
    if (!field) {
      field = document.createElement('textarea');
      field.className = 'command-fallback'; field.readOnly = true; field.rows = 4;
      field.setAttribute('aria-label', 'Install command — select and copy');
      field.value = button.dataset.copy; button.parentElement.append(field);
    }
    field.focus(); field.select();
    toast('Select and copy the install command below.');
  }
}));

const windows = /Windows/.test(navigator.userAgent);
const mac = /Macintosh|Mac OS X/.test(navigator.userAgent) && !/iPhone|iPad/.test(navigator.userAgent);
$$('[data-mod]').forEach(kbd => kbd.textContent = windows ? 'Ctrl' : '⌘');
// A mobile browser is not a supported desktop or a Linux server.
const linux = /Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent);
const recommended = $(`[data-platform="${windows ? 'win' : mac ? 'mac' : linux ? 'linux' : ''}"]`);
if (recommended) {
  recommended.classList.add('is-recommended');
  const badge = document.createElement('span'); badge.className = 'badge';
  badge.textContent = windows || mac ? 'For your device' : 'Headless server'; recommended.append(badge);
}
if (windows) {
  $('[data-os-label]').textContent = 'Download for Windows';
  $('[data-primary-download]').href = $('[data-asset="x64-setup.exe"]').href;
} else if (mac) $('[data-os-label]').textContent = 'Download for macOS';

// Small illustrative bubble map, not live storage data.
const bubbleMap = $('[data-bubbles]');
// Leave at least 10 SVG units between circles: independent float animations
// can close the gap by up to sqrt(4² + 6²), so they must not start tangent.
[[150,92,56],[54,64,30],[250,58,30],[250,139,23],[64,142,22],[111,20,11],[190,161,11]].forEach(([cx,cy,r], i) => {
  const circle = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
  circle.classList.add('bubble'); circle.style.animationDelay = `${-i}s`;
  circle.style.setProperty('--d', `${i * 70}ms`);
  Object.entries({cx, cy, r, fill: `hsl(${220 + i * 21} 70% 65% / .${i ? '45' : '65'})`,
    stroke: 'rgb(255 255 255 / .22)', 'stroke-width': 1}).forEach(([key, value]) => circle.setAttribute(key, value));
  bubbleMap.append(circle);
});
if ('IntersectionObserver' in window) {
  const ask = $('[data-ask]');
  const observer = new IntersectionObserver(entries => {
    if (entries.some(entry => entry.isIntersecting)) {
      ask.classList.add('is-done'); observer.disconnect();
    }
  }, { threshold: .4 });
  observer.observe(ask);
} else $('[data-ask]').classList.add('is-done');
root.setAttribute('data-enhanced', '');
