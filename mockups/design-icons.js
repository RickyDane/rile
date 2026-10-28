// Reuse the explorer's icon vocabulary throughout the design study.
function designIcon(name) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('class', 'design-icon');
  svg.innerHTML = icons[name] || '';
  return svg;
}
document.querySelectorAll('.nav').forEach((nav, i) => {
  const names = ['home', 'folder', 'clock', 'drive', 'grid'];
  nav.textContent = nav.textContent.replace(/^[⌂▱◷◉◇]\s*/, '');
  nav.prepend(designIcon(names[i]));
});
document.querySelectorAll('.btn').forEach(button => {
  const label = button.textContent.trim();
  const name = {'Choose folder':'folder', 'New folder':'folder-plus', 'Remove':'close', 'Analyze':'search', 'Copy path':'file', 'Reveal':'right', 'Open folder':'folder'}[label];
  if (name) button.prepend(designIcon(name));
  if (button.classList.contains('icon')) { button.textContent = ''; button.append(designIcon('more')); }
});
document.querySelectorAll('.tab').forEach(tab => {
  const name = {Overview:'grid', Files:'file', Cleanup:'box'}[tab.textContent.trim()];
  if (name) tab.prepend(designIcon(name));
});
const card = document.createElement('section');
card.className = 'card wide';
card.innerHTML = '<h2>Icon vocabulary</h2><p>Consistent 16px line icons for navigation, actions, views and plugins.</p><div class="icon-gallery"></div>';
const gallery = card.querySelector('.icon-gallery');
[['home','Home'],['folder','Folder'],['folder-plus','New folder'],['search','Search'],['clock','Recent'],['heart','Favorite'],['file','File'],['drive','Storage'],['grid','Grid'],['list','List'],['columns','Columns'],['sort','Sort'],['download','Download'],['more','More']].forEach(([name, label]) => {
  const item = document.createElement('div');
  item.className = 'icon-sample';
  item.append(designIcon(name), document.createTextNode(label));
  gallery.append(item);
});
document.querySelector('.grid').append(card);
