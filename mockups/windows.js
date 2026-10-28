/* Opt-in Windows review; the default macOS preview is unchanged. */
window.rileWindows = new URLSearchParams(location.search).get('platform') === 'windows';
if (window.rileWindows) {
  document.body.dataset.platform = 'windows';
  document.title = 'Rile for Windows — Design preview';
  document.querySelector('.traffic-lights').remove();
  document.querySelector('[data-location="mac"] span:not([data-icon])').textContent = 'This PC';
  document.querySelector('[data-location="disk"] span:not([data-icon])').textContent = 'Local Disk (C:)';
  document.querySelector('.drive-summary > span:last-child').firstChild.textContent = 'Local Disk (C:)';
  document.querySelector('.search kbd').textContent = 'Ctrl F';
  document.querySelector('.prototype-note').textContent = 'Windows concept · Sample files';

  const titlebar = document.createElement('header');
  titlebar.className = 'windows-titlebar';
  titlebar.innerHTML = `<span class="windows-app-mark" aria-hidden="true">r</span>
    <span>Rile <span class="windows-preview-label">for Windows · Design preview</span></span>
    <nav aria-label="Preview screens"><a href="?platform=windows">Home</a><a href="?platform=windows&preview=files">Files</a></nav>
    <div class="windows-caption-controls" aria-label="Illustrative window controls">
      <button type="button" aria-label="Minimize (preview)" title="Minimize (illustrative)"><svg viewBox="0 0 12 12" aria-hidden="true"><path d="M1 6h10"/></svg></button>
      <button type="button" aria-label="Maximize (preview)" title="Maximize (illustrative)"><svg viewBox="0 0 12 12" aria-hidden="true"><rect x="1.5" y="1.5" width="9" height="9"/></svg></button>
      <button type="button" aria-label="Close (preview)" title="Close (illustrative)"><svg viewBox="0 0 12 12" aria-hidden="true"><path d="m2 2 8 8m0-8-8 8"/></svg></button>
    </div>`;
  document.body.prepend(titlebar);
  titlebar.querySelectorAll('button').forEach(button => button.addEventListener('click', () => {
    toast('Window controls are illustrative. This is a browser preview.');
  }));
}
