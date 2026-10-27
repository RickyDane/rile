/* In-memory interaction prototype. Never reads or writes the user's filesystem. */
const icons = {
  folder:'<path d="M3 7V5a1 1 0 0 1 1-1h5l2 3h9a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7Z"/>',
  'folder-plus':'<path d="M3 7V5a1 1 0 0 1 1-1h5l2 3h9a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7Z"/><path d="M12 10v7m-3-3h6"/>',
  home:'<path d="m3 10 9-7 9 7v10H3V10Z"/><path d="M9 20v-7h6v7"/>',
  clock:'<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  heart:'<path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1.1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21l8.8-8.6a5.5 5.5 0 0 0 0-7.8Z"/>',
  file:'<path d="M14 3H5v18h14V8l-5-5Z"/><path d="M14 3v5h5"/>',
  download:'<path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5"/>',
  image:'<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8" cy="8" r="1.5"/><path d="m3 17 5-5 4 4 4-6 5 7"/>',
  drive:'<rect x="3" y="5" width="18" height="14" rx="3"/><path d="M3 13h18M16 16h2"/>',
  desktop:'<rect x="3" y="4" width="18" height="14" rx="2"/><path d="M8 21h8m-4-3v3M3 14h18"/>',
  laptop:'<rect x="5" y="4" width="14" height="12" rx="1"/><path d="M5 16 2 20h20l-3-4M10 18h4"/>',
  left:'<path d="m14 6-6 6 6 6"/>',right:'<path d="m10 6 6 6-6 6"/>',
  chevron:'<path d="m6 9 6 6 6-6"/>',
  box:'<rect x="3" y="3" width="18" height="5" rx="1"/><path d="M5 8v13h14V8M10 12h4"/>',
  sidebar:'<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16M5.5 8h1M5.5 11h1"/>',
  inspector:'<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M15 4v16"/>',
  plus:'<path d="M12 5v14M5 12h14"/>',close:'<path d="m6 6 12 12M6 18 18 6"/>',
  grid:'<rect x="4" y="4" width="6" height="6" rx="1"/><rect x="14" y="4" width="6" height="6" rx="1"/><rect x="4" y="14" width="6" height="6" rx="1"/><rect x="14" y="14" width="6" height="6" rx="1"/>',
  list:'<path d="M8 5h13M8 12h13M8 19h13M3 5h.1M3 12h.1M3 19h.1"/>',
  columns:'<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16M15 4v16"/>',
  search:'<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/>',
  sort:'<path d="M8 4v16m-4-4 4 4 4-4M14 5h7M14 10h5M14 15h3"/>',
  more:'<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
  info:'<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7h.01"/>',
  calendar:'<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M7 3v4m10-4v4M3 11h18M7 15h2m4 0h2m-8 3h2"/>',
  tag:'<path d="M3 3h9l9 9-9 9-9-9V3Z"/><circle cx="7.5" cy="7.5" r="1"/>',
  expand:'<path d="M14 3h7v7m0-7-8 8M10 21H3v-7m0 7 8-8"/>',
  code:'<path d="m8 7-5 5 5 5m8-10 5 5-5 5M14 4l-4 16"/>',
  atom:'<ellipse cx="12" cy="12" rx="10" ry="4"/><ellipse cx="12" cy="12" rx="10" ry="4" transform="rotate(60 12 12)"/><ellipse cx="12" cy="12" rx="10" ry="4" transform="rotate(120 12 12)"/><circle cx="12" cy="12" r="1"/>',
};
const $ = selector => document.querySelector(selector);
const escapeHtml = value => String(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const icon = (name, className = '') => `<svg class="${className}" viewBox="0 0 24 24" aria-hidden="true">${icons[name] || icons.file}</svg>`;
const kinds = {folder:'Folder',rust:'Rust source',toml:'TOML document',markdown:'Markdown',image:'JPEG image',screen:'PNG image',react:'TypeScript JSX',css:'CSS stylesheet',pdf:'PDF document',json:'JSON document',text:'Text document',archive:'ZIP archive',svg:'SVG image'};
const tagColors = {Work:'blue',Personal:'violet',Important:'orange'};
const files = [];
function folder(id, parent, name, extra = {}) { files.push({id,parent,name,kind:'folder',modified:'2026-10-27T09:42:00',tags:[],...extra}); }
function file(id, parent, name, kind, bytes, extra = {}) { files.push({id,parent,name,kind,bytes,modified:'2026-10-26T14:32:00',tags:[],...extra}); }
folder('disk', null, 'Macintosh HD', {placeIcon:'drive'});
folder('users', 'disk', 'Users');
folder('home', 'users', 'rickyperlick', {placeIcon:'home'});
folder('desktop', 'home', 'Desktop', {placeIcon:'desktop'});
folder('documents', 'home', 'Documents', {placeIcon:'file'});
folder('downloads', 'home', 'Downloads', {placeIcon:'download'});
folder('pictures', 'home', 'Pictures', {placeIcon:'image'});
folder('projects', 'home', 'Projects');
folder('rile', 'projects', 'rile', {tags:['Work'],favorite:true});
folder('website', 'projects', 'website', {tags:['Work']});
folder('experiments', 'projects', 'experiments');
folder('assets', 'rile', 'assets', {tags:['Work'],placeIcon:'image'});
folder('docs', 'rile', 'docs');
folder('plugins', 'rile', 'plugins');
folder('src', 'rile', 'src', {tags:['Work'],favorite:true});
folder('tests', 'rile', 'tests');
file('cargo', 'rile', 'Cargo.toml', 'toml', 1126, {tags:['Work']});
file('changelog', 'rile', 'CHANGELOG.md', 'markdown', 4800);
file('app', 'rile', 'App.tsx', 'react', 2867);
file('main', 'rile', 'main.rs', 'rust', 4300, {tags:['Work']});
file('readme', 'rile', 'README.md', 'markdown', 6700);
file('roadmap', 'rile', 'roadmap.pdf', 'pdf', 1840000, {tags:['Important']});
file('schema', 'rile', 'schema.json', 'json', 856);
file('screenshot', 'rile', 'screenshot.png', 'screen', 2430000, {dimensions:'2560 × 1600',tags:['Work']});
file('styles', 'rile', 'styles.css', 'css', 3400);
file('alpine', 'rile', 'alpine-lake.jpg', 'image', 4250000, {dimensions:'3840 × 2560',tags:['Personal'],favorite:true,modified:'2026-10-27T08:24:00'});
file('gitignore', 'rile', '.gitignore', 'text', 124);
file('architecture', 'docs', 'architecture.md', 'markdown', 12300);
file('shortcuts', 'docs', 'keyboard-shortcuts.md', 'markdown', 3400);
file('theme', 'src', 'theme.rs', 'rust', 2800);
file('pane', 'src', 'pane.rs', 'rust', 8100);
file('workspace', 'src', 'workspace.rs', 'rust', 18400);
file('test-navigation', 'tests', 'navigation.rs', 'rust', 3900);
file('test-files', 'tests', 'file_operations.rs', 'rust', 5100);
file('plugin', 'plugins', 'hello.rs', 'rust', 730);
file('app-icon', 'assets', 'icon.svg', 'svg', 3700);
file('hero', 'assets', 'hero.jpg', 'image', 2380000, {dimensions:'2400 × 1600'});
file('website-index', 'website', 'index.html', 'text', 4100);
file('desktop-notes', 'desktop', 'Notes.md', 'markdown', 2100);
file('project-plan', 'documents', 'Project plan.pdf', 'pdf', 284000, {tags:['Work','Important']});
file('invoice', 'documents', 'Invoice — October.pdf', 'pdf', 94000, {tags:['Important']});
file('download-image', 'downloads', 'mountains.jpg', 'image', 3200000, {dimensions:'3000 × 2000'});
file('download-archive', 'downloads', 'icons.zip', 'archive', 12400000);
file('download-pdf', 'downloads', 'Getting started.pdf', 'pdf', 426000);
file('download-json', 'downloads', 'settings.json', 'json', 1200);
file('picture-lake', 'pictures', 'Lake at dawn.jpg', 'image', 5100000, {dimensions:'4096 × 2730',tags:['Personal']});
file('picture-screen', 'pictures', 'Screenshot 2026-10-26.png', 'screen', 1420000, {dimensions:'2560 × 1600'});
folder('ssd', null, 'Studio SSD', {placeIcon:'drive'});
folder('backups', 'ssd', 'Backups');
folder('media', 'ssd', 'Media');
file('backup-zip', 'backups', 'rile-backup.zip', 'archive', 48200000);
file('media-lake', 'media', 'Alps.jpg', 'image', 6800000, {dimensions:'5000 × 3333'});
const virtualLocations = {start:'Home',recents:'Recents',favorites:'Favorites',mac:'MacBook Pro','tag-work':'Work','tag-personal':'Personal','tag-important':'Important'};
let nextTab = 3;
const makeTab = (id, location, selected = null) => ({id,history:[location],cursor:0,selected,query:''});
const state = {tabs:[makeTab(1,'start'),makeTab(2,'rile','alpine')],active:new URLSearchParams(location.search).get('preview')==='files'?2:1,view:'grid',sort:'name',ascending:true,showHidden:false,inspector:!matchMedia('(max-width:720px)').matches};
const tab = () => state.tabs.find(t => t.id === state.active);
const locationId = () => tab().history[tab().cursor];
const find = id => files.find(f => f.id === id);
const locationName = id => virtualLocations[id] || find(id)?.name || 'Files';
const isFolder = f => f.kind === 'folder';
const childFiles = id => files.filter(f => f.parent === id);
const fileSize = f => isFolder(f) ? `${childFiles(f.id).length} ${childFiles(f.id).length===1?'item':'items'}` : f.bytes < 1000 ? `${f.bytes} B` : f.bytes < 1000000 ? `${+(f.bytes / 1000).toFixed(1)} KB` : `${+(f.bytes / 1000000).toFixed(1)} MB`;
const modified = f => new Date(f.modified).toLocaleDateString('en-US',{month:'short',day:'numeric',year:'numeric'});
const fileIcon = f => f.placeIcon || (isFolder(f) ? 'folder' : ['image','screen','svg'].includes(f.kind) ? 'image' : ['rust','react','css','json','toml'].includes(f.kind) ? 'code' : 'file');
function rawFiles(id = locationId()) {
  if (id === 'start') return files.filter(f => f.parent);
  if (id === 'recents') return files.filter(f => !isFolder(f)).sort((a,b) => b.modified.localeCompare(a.modified)).slice(0,12);
  if (id === 'favorites') return files.filter(f => f.favorite);
  if (id.startsWith('tag-')) return files.filter(f => f.tags.some(tag => tag.toLowerCase() === id.slice(4)));
  return childFiles(id === 'mac' ? 'home' : id);
}
function currentFiles() {
  return rawFiles().filter(f => (state.showHidden || !f.name.startsWith('.')) && f.name.toLowerCase().includes(tab().query.toLowerCase())).sort((a,b) => {
    const folderOrder = Number(isFolder(b)) - Number(isFolder(a));
    const order = state.sort === 'modified' ? a.modified.localeCompare(b.modified) : state.sort === 'type' ? kinds[a.kind].localeCompare(kinds[b.kind]) || a.name.localeCompare(b.name) : a.name.localeCompare(b.name);
    return folderOrder || (state.ascending ? order : -order);
  });
}
function pathNodes(id = locationId()) {
  if (id === 'start') return [{id:'start',name:'Home',placeIcon:'home'}];
  if (id in virtualLocations) return [find('home'),{id,name:locationName(id),placeIcon:id==='recents'?'clock':id==='favorites'?'heart':'tag'}];
  const nodes = []; let current = find(id);
  while (current) { nodes.unshift(current); current = find(current.parent); }
  return nodes;
}
function pathString(id = locationId()) {
  const nodes = pathNodes(id);
  if (nodes[0]?.id==='disk') return '/' + nodes.slice(1).map(f=>f.name).join('/');
  if (nodes[0]?.id==='ssd') return '/Volumes/' + nodes.map(f=>f.name).join('/');
  return '/' + nodes.map(f=>f.name).join('/');
}
function landscape() {
  // Self-contained illustration: no external image requests or tracking.
  return '<svg class="landscape" viewBox="0 0 600 400" preserveAspectRatio="xMidYMid slice" aria-hidden="true"><defs><linearGradient id="sky" x2="0" y2="1"><stop stop-color="#a4c0d1"/><stop offset="1" stop-color="#d8d6c5"/></linearGradient><linearGradient id="water" x2="0" y2="1"><stop stop-color="#5c969a"/><stop offset="1" stop-color="#1f555d"/></linearGradient><linearGradient id="rock" x2=".9" y2="1"><stop stop-color="#b5b2a2"/><stop offset="1" stop-color="#5a6b68"/></linearGradient></defs><path fill="url(#sky)" d="M0 0h600v400H0z"/><path fill="#a3b2ad" d="m0 200 94-105 45 37 77-96 84 111 48-47 91 94 75-75 86 74v207H0z"/><path fill="url(#rock)" d="m0 226 67-99 65 37 84-128 60 81 40 60 31-48 76 94 67-75 110 114v138H0z"/><path fill="#e0e0d2" d="m132 164 84-128 60 81-31-17-16-28-14 26-12-13-23 42-15-1z"/><path fill="#c9d2c7" d="m347 129 76 94-41-27-14-29-13 2-13-22-26 30z"/><path fill="#657d6c" d="m0 191 70 41 75-29 73 45 101-16 63 30 86-52 132 1v189H0z"/><path fill="url(#water)" d="M0 271q133-26 271-7t329-13v149H0z"/><path fill="#85b3aa" opacity=".4" d="m140 272 78 15 71-12 86 32-167 2 98 14-171-8 66-14z"/><path fill="#294c40" d="m0 208 34 41 37 13 46 19-60 40-57 4zm600-23-45 54-37 18-50 17 83 27 49 41z"/><path fill="#1f3e36" d="m0 290 35 23 24 27 53 17 52 43H0zm600 15-53 22-28 34-52 39h133z"/><path fill="#59766a" d="m0 338 65 13 46 30 55 19H0z"/><path stroke="#b8d3c4" stroke-width="1" opacity=".35" d="m142 345 97 1m42 17h69m-157 15 145-1m39-53h53"/></svg>';
}
function fileArt(f) {
  let art;
  if (isFolder(f)) art = `<div class="folder-object">${f.placeIcon ? icon(f.placeIcon) : ''}</div>`;
  else if (f.kind === 'image') art = `<div class="media-object">${landscape()}</div>`;
  else if (f.kind === 'screen') art = '<div class="media-object screen-object"><div class="screen-mini"><i></i><div><b></b><b></b><b></b><b></b><b></b><b></b></div></div></div>';
  else {
    const artByKind = {rust:['#df985c','R'],toml:['#d7a563','[T]'],markdown:['#96a5bd','M↓'],react:['#49b9e4',icon('atom')],css:['#4aabed','3'],pdf:['#da877c','<span class="extension-badge">PDF</span>'],json:['#d0b770','{ }'],svg:['#bd96e0',icon('image')],archive:['#c5a16d','ZIP'],text:['#8495ae','≡']};
    const [color,content] = artByKind[f.kind] || artByKind.text;
    art = `<div class="document-object" style="--file-color:${color}">${content.startsWith('<') ? content : `<span>${content}</span>`}</div>`;
  }
  return `<div class="file-object" aria-hidden="true">${art}</div>`;
}
const tagDots = f => f.tags.map(tag => `<i class="${tagColors[tag]}" title="${tag}"></i>`).join('');
function card(f) { return `<button class="file-card ${f.id===tab().selected?'selected':''} ${f.favorite?'favorite':''}" data-file="${f.id}" aria-label="Select ${escapeHtml(f.name)}" aria-pressed="${f.id===tab().selected}" title="${escapeHtml(f.name)}">${fileArt(f)}<span class="file-name">${escapeHtml(f.name)}</span><span class="file-meta">${fileSize(f)}</span><span class="file-tags">${tagDots(f)}</span></button>`; }
function columnRow(f, parent = false) { return `<button class="column-row ${parent ? f.id===locationId()?'selected':'' : f.id===tab().selected?'selected':''}" ${parent ? 'data-navigate' : 'data-file'}="${f.id}" ${parent?'':`aria-pressed="${f.id===tab().selected}"`} aria-label="${parent?'Open':'Select'} ${escapeHtml(f.name)}">${icon(fileIcon(f))}<span>${escapeHtml(f.name)}</span>${isFolder(f)?icon('right'):''}</button>`; }
function renderTabs() {
  $('#tabs').innerHTML = state.tabs.map(t => {
    const id = t.history[t.cursor], name = locationName(id);
    return `<div class="tab ${t.id===state.active?'active':''}" role="presentation"><button role="tab" id="tab-${t.id}" data-tab="${t.id}" aria-selected="${t.id===state.active}" tabindex="${t.id===state.active?0:-1}" aria-controls="file-area">${icon(id==='start'?'home':find(id)?.placeIcon || 'folder')}<span>${escapeHtml(name)}</span></button><button class="tab-close" data-close-tab="${t.id}" aria-label="Close ${escapeHtml(name)} tab" title="Close tab">${icon('close')}</button></div>`;
  }).join('');
  $('#file-area').setAttribute('aria-labelledby',`tab-${state.active}`);
}
function renderChrome() {
  const id = locationId(), nodes = pathNodes();
  const atStart = id === 'start';
  $('.main').classList.toggle('at-start',atStart);
  $('.browser-toolbar').hidden = atStart;
  $('#path-bar').hidden = atStart;
  $('.zoom-control').hidden = atStart;
  $('#inspector-toggle').disabled = atStart;
  $('#search').placeholder = atStart ? 'Search sample workspace' : 'Search this folder';
  $('#search').setAttribute('aria-label',atStart ? 'Search sample workspace' : 'Search this folder');
  $('#location-title').textContent = locationName(id);
  $('#location-icon').innerHTML = icon(find(id)?.placeIcon || (id==='recents'?'clock':id==='favorites'?'heart':id.startsWith('tag-')?'tag':'folder'));
  $('#toolbar-path').innerHTML = `<button data-navigate="mac">MacBook Pro</button>${icon('right')}${nodes.slice(-2).map((f,i,arr) => i===arr.length-1 ? `<strong>${escapeHtml(locationName(f.id))}</strong>` : `<button data-navigate="${f.id}">${escapeHtml(locationName(f.id))}</button>${icon('right')}`).join('')}`;
  $('#path-bar').innerHTML = nodes.map(f => `<button data-navigate="${f.id}" title="${escapeHtml(pathString(f.id))}">${icon(f.placeIcon || 'folder')} ${escapeHtml(f.name)}</button>`).join(icon('right'));
  $('#back').disabled = tab().cursor===0;
  $('#forward').disabled = tab().cursor===tab().history.length-1;
  $('#search').value = tab().query;
  $('#new-folder').disabled = id in virtualLocations;
  $('#new-folder').title = id in virtualLocations ? 'Open a folder to create a folder' : 'New folder';
  document.querySelectorAll('[data-location]').forEach(button => {
    const active = button.dataset.location === id || (button.dataset.location === 'projects' && nodes.some(f => f.id === 'projects'));
    button.classList.toggle('active',active);
    if (active) button.setAttribute('aria-current','location'); else button.removeAttribute('aria-current');
  });
  renderTabs();
}
function renderFiles() {
  if (locationId()==='start') { renderStartPage(); renderSelection(); return; }
  const list = currentFiles();
  if (!list.some(f => f.id===tab().selected)) tab().selected = null;
  let html;
  if (!list.length) html = `<div class="empty-state">${icon(tab().query?'search':'folder')}<h2>${tab().query?'No matching files':'Nothing here yet'}</h2><p>${tab().query?'Try a different filename or clear your search.':'This sample folder is empty.'}</p></div>`;
  else if (state.view==='grid') html = `<div class="file-grid">${list.map(card).join('')}</div>`;
  else if (state.view==='list') html = `<table class="file-list"><thead><tr><th>Name</th><th>Date modified</th><th>Size</th><th>Kind</th></tr></thead><tbody>${list.map(f => `<tr class="${f.id===tab().selected?'selected':''}"><td><button data-file="${f.id}" aria-label="Select ${escapeHtml(f.name)}" aria-pressed="${f.id===tab().selected}">${icon(fileIcon(f),isFolder(f)?'folder-row-icon':'')}<span>${escapeHtml(f.name)}</span></button></td><td>${modified(f)}</td><td>${isFolder(f)?'—':fileSize(f)}</td><td>${kinds[f.kind]}</td></tr>`).join('')}</tbody></table>`;
  else {
    const parent = find(locationId())?.parent;
    const siblings = parent ? childFiles(parent).filter(isFolder) : rawFiles().filter(isFolder);
    html = `<div class="column-browser"><section class="column-parent" aria-label="Parent folders"><p>${escapeHtml(parent?locationName(parent):'Folders')}</p>${siblings.map(f => columnRow(f,true)).join('')}</section><section class="column-files" aria-label="Current folder"><p>${escapeHtml(locationName(locationId()))}</p>${list.map(f => columnRow(f)).join('')}</section><section class="column-selection" id="column-selection" aria-label="Selection"></section></div>`;
  }
  $('#file-area').innerHTML = html;
  $('#file-area').dataset.view = state.view;
  const count = `${list.length} ${list.length===1?'item':'items'}`;
  $('#location-count').textContent = count;
  $('#item-count').textContent = tab().query ? `${count} found` : count;
  renderSelection();
}
function renderSelection() {
  const f = currentFiles().find(f => f.id===tab().selected);
  $('#selection-status').textContent = f ? `1 selected · ${fileSize(f)}` : 'No selection';
  $('#selection-status').hidden = !f;
  if ($('#column-selection')) $('#column-selection').innerHTML = f ? `${fileArt(f)}<strong>${escapeHtml(f.name)}</strong><p>${kinds[f.kind]} · ${fileSize(f)}</p>` : '<p>Select a file to preview</p>';
  renderInspector(f);
}
function renderInspector(f) {
  const panel = $('#inspector'), visible = state.inspector && locationId()!=='start'; panel.hidden = !visible;
  $('#inspector-toggle').classList.toggle('active',visible);
  $('#inspector-toggle').setAttribute('aria-pressed',visible);
  const header = `<header class="inspector-header"><span>Inspector</span><button id="close-inspector" class="icon-button" aria-label="Close inspector">${icon('close')}</button></header>`;
  if (!f) { panel.innerHTML = `${header}<div class="empty-state">${icon('info')}<h2>A closer look.</h2><p>Select a file to see its preview<br>and details here.</p></div>`; return; }
  const row = (label,value) => `<div><dt>${label}</dt><dd>${escapeHtml(value)}</dd></div>`;
  panel.innerHTML = `${header}<div class="preview-stage ${f.kind==='image'?'landscape-stage':''}">${f.kind==='image'?landscape():fileArt(f)}</div><div class="inspector-identity"><h2>${escapeHtml(f.name)}</h2><p>${kinds[f.kind]}${isFolder(f)?'':' · '+fileSize(f)}</p></div><div class="inspector-quick-actions"><button class="icon-button ${f.favorite?'active':''}" id="favorite-file" aria-label="${f.favorite?'Remove from':'Add to'} favorites" aria-pressed="${!!f.favorite}" title="Favorite">${icon('heart')}</button><button class="icon-button" id="reveal-file" aria-label="Show in folder" title="Show in folder">${icon('folder')}</button><button class="icon-button" id="preview-file" aria-label="${isFolder(f)?'Open folder':'Quick Look'}" title="${isFolder(f)?'Open folder':'Quick Look'}">${icon('expand')}</button></div><section class="inspector-section"><h3>${icon('info')} Details</h3><dl>${row(isFolder(f)?'Contains':'Size',fileSize(f))}${!isFolder(f)?row('Extension',f.name.split('.').pop()):''}${row('Kind',kinds[f.kind])}${row('Location',locationName(f.parent))}</dl></section><section class="inspector-section"><h3>${icon('calendar')} Dates</h3><dl>${row('Created','Oct 18, 2026')}${row('Modified',modified(f))}${row('Accessed','Oct 27, 2026')}</dl></section>${f.dimensions?`<section class="inspector-section"><h3>${icon('image')} Image</h3><dl>${row('Dimensions',f.dimensions)}${row('Color space','sRGB')}</dl></section>`:''}<section class="inspector-section"><h3>${icon('tag')} Tags</h3><div class="inspector-tags">${f.tags.length?f.tags.map(tag=>`<button class="inspector-tag" data-navigate="tag-${tag.toLowerCase()}"><i class="tag-dot ${tagColors[tag]}"></i>${tag}</button>`).join(''):'<span class="no-tags">No tags</span>'}</div></section><button class="inspector-open" id="open-selection">${icon(isFolder(f)?'folder':'expand')}${isFolder(f)?'Open folder':'Quick Look'}<kbd>${isFolder(f)?'↵':'Space'}</kbd></button>`;
}
function render() { renderChrome(); renderFiles(); }
function navigate(id) {
  if (!find(id) && !(id in virtualLocations)) return;
  if (id===locationId()) return;
  tab().history = tab().history.slice(0,tab().cursor+1);
  tab().history.push(id); tab().cursor++; tab().selected=null; tab().query='';
  closeMenus(); render(); $('#file-area').scrollTop=0;
}
function selectFile(id) {
  if (!currentFiles().some(f => f.id===id)) return;
  tab().selected = id;
  document.querySelectorAll('[data-file]').forEach(button => {
    const active = button.dataset.file===id;
    button.classList.toggle('selected',active); button.setAttribute('aria-pressed',active);
    button.closest('tr')?.classList.toggle('selected',active);
  });
  renderSelection();
}
function selectedFile() { return find(tab().selected); }
function openSelection() {
  const f = selectedFile(); if (!f) return;
  if (isFolder(f)) { navigate(f.id); return; }
  quickLook();
}
function quickLook() {
  const f = selectedFile(); if (!f) return;
  $('#quicklook-name').textContent = f.name;
  $('#quicklook-content').innerHTML = f.kind==='image' ? landscape() : ['markdown','rust','toml','json','css','react','text'].includes(f.kind) ? `<pre class="code-preview">${escapeHtml(sampleText(f))}</pre>` : fileArt(f);
  $('#quicklook-meta').textContent = `${kinds[f.kind]} · ${fileSize(f)} · Illustrative sample preview`;
  $('#quicklook').showModal();
}
function sampleText(f) {
  if (f.kind==='markdown') return `# ${f.name.replace(/\.md$/,'')}\n\nA fast, index-free file explorer.\n\n## Getting started\n\ncargo run -p rile-app\n\n## Features\n\n- Familiar file navigation\n- Icon, list and column views\n- Quick Look and file details\n- Your files stay on your device\n\nThis is illustrative sample content.`;
  if (f.kind==='rust') return '// Rile · illustrative source preview\n\nuse gpui_kit::prelude::*;\n\npub fn main() {\n    // A little less friction.\n    // A little more focus.\n    Application::new().run(|cx| {\n        open_workspace(cx);\n    });\n}\n';
  if (f.kind==='toml') return '[package]\nname = "rile"\nversion = "0.1.0"\nedition = "2024"\n\n# Illustrative sample configuration';
  if (f.kind==='json') return '{\n  "theme": "dark",\n  "view": "grid",\n  "show_hidden": false\n}\n\n// Illustrative sample content';
  return `${f.name}\n\nIllustrative sample preview.\nThe native app will display the real file content.`;
}
function closeMenus() { ['sort','more'].forEach(id => { $(`#${id}-menu`).hidden=true; $(`#${id}-toggle`).setAttribute('aria-expanded','false'); }); }
let toastTimer;
function toast(message) { $('#toast').textContent=message; $('#toast').classList.add('visible'); clearTimeout(toastTimer); toastTimer=setTimeout(()=>$('#toast').classList.remove('visible'),3000); }
function switchTab(id) { state.active=id; closeMenus(); render(); }
document.addEventListener('click',event => {
  const button = event.target.closest('button');
  if (!event.target.closest('.popover-anchor')) closeMenus();
  if (!button) return;
  if (button.dataset.location) navigate(button.dataset.location);
  if (button.dataset.navigate) navigate(button.dataset.navigate);
  if (button.dataset.startFile) { const f=find(button.dataset.startFile); if(f) { navigate(f.parent); selectFile(f.id); } }
  if (button.dataset.file) { selectFile(button.dataset.file); if (event.detail===2) openSelection(); }
  if (button.dataset.view) {
    state.view=button.dataset.view;
    document.querySelectorAll('[data-view]').forEach(el=>{ const active=el.dataset.view===state.view; el.classList.toggle('active',active); el.setAttribute('aria-pressed',active); });
    renderFiles();
  }
  if (button.dataset.tab) switchTab(Number(button.dataset.tab));
  if (button.dataset.closeTab) {
    const id=Number(button.dataset.closeTab), index=state.tabs.findIndex(t=>t.id===id);
    state.tabs.splice(index,1);
    if (!state.tabs.length) state.tabs.push(makeTab(nextTab++,'start'));
    if (state.active===id) state.active=state.tabs[Math.min(index,state.tabs.length-1)].id;
    render();
  }
  if (button.id==='new-tab') { const t=makeTab(nextTab++,'start'); state.tabs.push(t); switchTab(t.id); }
  if (button.id==='back' || button.id==='forward') { tab().cursor+=button.id==='back'?-1:1; tab().selected=null; tab().query=''; render(); }
  if (button.id==='sidebar-toggle') { const hidden=document.body.classList.toggle('sidebar-hidden'); button.setAttribute('aria-pressed',!hidden); }
  if (['inspector-toggle','close-inspector'].includes(button.id)) { state.inspector=!state.inspector; renderSelection(); }
  if (['preview-file','open-selection'].includes(button.id)) openSelection();
  if (button.id==='close-quicklook') $('#quicklook').close();
  if (button.id==='reveal-file') { const f=selectedFile(); if(f) { navigate(f.parent); selectFile(f.id); } }
  if (button.id==='favorite-file') { const f=selectedFile(); if(f) { f.favorite=!f.favorite; renderFiles(); } }
  if (['sort-toggle','more-toggle'].includes(button.id)) { const menu=$(`#${button.id.replace('-toggle','-menu')}`), open=menu.hidden; closeMenus(); menu.hidden=!open; button.setAttribute('aria-expanded',open); }
  if (button.dataset.sort) {
    state.sort=button.dataset.sort;
    document.querySelectorAll('[data-sort]').forEach(el=>{ const active=el.dataset.sort===state.sort; el.setAttribute('aria-pressed',active); el.querySelector('span').textContent=active?'✓':''; });
    closeMenus(); renderFiles();
  }
  if (button.id==='sort-direction') { state.ascending=!state.ascending; button.innerHTML=`${state.ascending?'Ascending':'Descending'} <span>${state.ascending?'↑':'↓'}</span>`; closeMenus(); renderFiles(); }
  if (button.id==='hidden-files') { state.showHidden=!state.showHidden; button.setAttribute('aria-pressed',state.showHidden); button.querySelector('span').textContent=state.showHidden?'✓':''; closeMenus(); renderFiles(); }
  if (button.id==='copy-path') {
    if (navigator.clipboard) navigator.clipboard.writeText(pathString()).then(()=>toast('Folder path copied.'),()=>toast('Clipboard unavailable in this browser.'));
    else toast('Clipboard unavailable in this browser.');
    closeMenus();
  }
});
$('#search').addEventListener('input',event=>{ tab().query=event.target.value; renderFiles(); });
$('#zoom').addEventListener('input',event=>document.documentElement.style.setProperty('--icon-size',event.target.value+'px'));
$('#new-folder').addEventListener('click',()=>{ $('#folder-name').value=''; $('#folder-name').setCustomValidity(''); $('#folder-parent').textContent=`Create a folder in ${locationName(locationId())}.`; $('#folder-dialog').showModal(); $('#folder-name').focus(); });
$('#cancel-folder').addEventListener('click',()=>$('#folder-dialog').close());
$('#folder-name').addEventListener('input',()=>$('#folder-name').setCustomValidity(''));
$('#folder-form').addEventListener('submit',event=>{
  event.preventDefault(); const input=$('#folder-name'), name=input.value.trim();
  let error='';
  if (!name || ['.','..'].includes(name) || /[\/:\x00]/.test(name)) error='Enter a folder name without / or : characters.';
  else if (rawFiles().some(f=>f.name.toLowerCase()===name.toLowerCase())) error='An item with this name already exists.';
  if(error) { input.setCustomValidity(error); input.reportValidity(); return; }
  const id='new-'+Date.now(); folder(id,locationId(),name,{modified:new Date().toISOString()});
  tab().query=''; tab().selected=id; $('#folder-dialog').close(); render(); toast(`“${name}” created in the sample workspace.`);
});
document.addEventListener('keydown',event=>{
  if (event.key==='Escape') closeMenus();
  const typing=['INPUT','TEXTAREA','SELECT'].includes(document.activeElement.tagName), modal=!!document.querySelector('dialog[open]');
  if ((event.metaKey||event.ctrlKey) && event.key.toLowerCase()==='f' && !modal) { event.preventDefault(); $('#search').focus(); $('#search').select(); return; }
  if (typing||modal) return;
  if (event.key==='/') { event.preventDefault(); $('#search').focus(); }
  const tabButton=event.target.closest('[role=tab]');
  if (tabButton && ['ArrowLeft','ArrowRight','Home','End'].includes(event.key)) {
    event.preventDefault(); const current=state.tabs.findIndex(t=>t.id===state.active), delta=event.key==='ArrowLeft'?-1:1;
    const next=event.key==='Home'?0:event.key==='End'?state.tabs.length-1:(current+delta+state.tabs.length)%state.tabs.length;
    switchTab(state.tabs[next].id); $(`[data-tab="${state.active}"]`).focus(); return;
  }
  if (locationId()==='start') return;
  const fileFocus=document.activeElement.closest('[data-file],#file-area') || document.activeElement===document.body;
  if (!fileFocus) return;
  if (event.key===' ' || event.key==='Enter') { event.preventDefault(); event.key===' '?quickLook():openSelection(); }
  if (['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key)) {
    event.preventDefault(); const list=currentFiles(), current=list.findIndex(f=>f.id===tab().selected);
    const grid=$('.file-grid'), columns=grid?getComputedStyle(grid).gridTemplateColumns.split(' ').length:1;
    const delta=(['ArrowLeft','ArrowUp'].includes(event.key)?-1:1)*(grid&&['ArrowUp','ArrowDown'].includes(event.key)?columns:1);
    const next=current<0?0:Math.max(0,Math.min(list.length-1,current+delta));
    if(list[next]) { selectFile(list[next].id); $(`[data-file="${list[next].id}"]`)?.focus(); }
  }
});
document.querySelectorAll('[data-icon]').forEach(el=>el.innerHTML=icon(el.dataset.icon));
document.querySelectorAll('[data-location]').forEach(el=>{ el.setAttribute('aria-label',locationName(el.dataset.location)); el.title=locationName(el.dataset.location); });
// Old concept URLs all resolve to this one explorer; the gallery no longer exists.
if(location.hash) history.replaceState(null,'',location.pathname+location.search);
render();
