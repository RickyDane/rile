/* Current library screens. Every operation is confined to sample data. */
const workspacePreview = {
  root:null, spaceTab:'overview', backupRuns:1, backupName:'Project archive',
  backupSource:'rile', backupDestination:'Studio SSD', photoSelection:null,
};
const wsButton = (label, action, glyph, variant='') => `<button class="ws-btn ${variant}" data-workspace-action="${action}">${glyph?icon(glyph):''}${label}</button>`;
const wsNote = text => `<p class="ws-note">${icon('info')}<span>${text}</span></p>`;
const wsHeading = (title, description, actions='') => `<header class="workspace-heading"><div><h1>${title}<span>.</span></h1>${description?`<p>${description}</p>`:''}</div><div class="ws-actions">${actions}</div></header>`;
function descendants(id) {
  return childFiles(id).flatMap(f => isFolder(f) ? [f,...descendants(f.id)] : [f]);
}
function previewBytes(f) {
  return isFolder(f) ? descendants(f.id).reduce((n,item)=>n+(item.bytes||0),0) : f.bytes||0;
}
function workspaceDialog(title, content) {
  const dialog = $('#workspace-dialog');
  dialog.innerHTML = `<h2 id="workspace-dialog-title">${title}</h2>${content}`;
  dialog.showModal();
}
function renderWorkspacePage() {
  const page=locationId();
  $('#file-area').dataset.view='workspace';
  const content = page==='space' ? spacePreview() : page==='backups' ? backupsPreview() : page==='photos' ? photosPreview() : page==='network' ? networkPreview() : settingsPreview();
  $('#file-area').innerHTML=`<div class="workspace-page">${content}</div>`;
  $('#item-count').textContent='Design preview · Sample data only';
}
function spacePreview() {
  const root=find(workspacePreview.root);
  const heading=wsHeading('Space Analyzer',root?`${escapeHtml(pathString(root.id))} · Sample analysis`:'Understand what is taking up space. Choose a folder to begin.',
    wsButton(root?'Change folder':'Choose folder','choose-root','folder',root?'':'primary')+(root?wsButton('Rescan','rescan','clock') : ''));
  if (!root) return `${heading}<section class="ws-card ws-empty">${icon('pie')}<h2>A little perspective. A little more space.</h2><p>Explore a folder as a bubble map, find its largest files, and review cleanup candidates. Nothing is scanned until you choose a folder.</p>${wsButton('Choose folder','choose-root','folder','primary')}</section>${wsNote('Native Space Analyzer preview, not a plugin. Only the in-memory sample workspace is analyzed; your files are never accessed.')}`;
  const contents=descendants(root.id), total=previewBytes(root), children=childFiles(root.id).slice().sort((a,b)=>previewBytes(b)-previewBytes(a));
  const selected=workspacePreview.spaceTab;
  const tabs=`<div class="ws-tabs" role="tablist" aria-label="Space Analyzer views">${['overview','files','cleanup'].map(name=>`<button role="tab" id="space-tab-${name}" aria-controls="space-panel" data-space-tab="${name}" aria-selected="${selected===name}" tabindex="${selected===name?0:-1}">${{overview:'Overview',files:'Largest files',cleanup:'Cleanup'}[name]}</button>`).join('')}</div>`;
  const rows=items=>items.map(f=>`<button class="ws-row" data-${isFolder(f)?'analyze-folder':'start-file'}="${f.id}">${icon(fileIcon(f))}<span>${escapeHtml(f.name)}<small>${isFolder(f)?`${descendants(f.id).length} items · Explore folder`:escapeHtml(locationName(f.parent))}</small></span><strong>${formatSize(previewBytes(f))}</strong>${icon('right')}</button>`).join('');
  let panel;
  if(selected==='overview') {
    const colors=['#8bbfff','#a393dc','#75c8b6','#dfab75','#dd92ae'];
    panel=`<div class="ws-split"><section class="ws-card"><div class="ws-kicker">Analyzed size</div><div class="ws-total">${formatSize(total)}</div><p>${contents.filter(f=>!isFolder(f)).length} files · ${contents.filter(isFolder).length} folders</p><div class="bubble-map" aria-label="Folder size map">${children.slice(0,7).map((f,i)=>`<button data-${isFolder(f)?'analyze-folder':'start-file'}="${f.id}" style="--bubble-size:${Math.round(82+110*Math.sqrt(previewBytes(f)/Math.max(total,1)))}px;--bubble-color:${colors[i%colors.length]}" title="${escapeHtml(f.name)} · ${formatSize(previewBytes(f))}"><strong>${escapeHtml(f.name)}</strong><small>${formatSize(previewBytes(f))}</small></button>`).join('')}</div></section><section class="ws-card"><h2>Folder contents</h2><p>Select a folder to drill down. Sizes are calculated from sample file metadata.</p>${root.parent?`<button class="ws-btn ghost" data-analyze-folder="${root.parent}">${icon('left')} Parent folder</button>`:''}${rows(children)}</section></div>`;
  } else if(selected==='files') {
    panel=`<section class="ws-card"><h2>Largest files</h2><p>All files below ${escapeHtml(root.name)}, sorted by size.</p>${rows(contents.filter(f=>!isFolder(f)).sort((a,b)=>b.bytes-a.bytes))||'<p>No files in this sample folder.</p>'}</section>`;
  } else {
    const candidates=contents.filter(f=>f.kind==='archive');
    panel=`<section class="ws-card"><h2>Review before you remove</h2><p>Archives may already be extracted or backed up. Review these sample candidates individually; they are not automatically safe to delete.</p>${candidates.map(f=>`<div class="ws-row">${icon('box')}<span>${escapeHtml(f.name)}<small>${formatSize(f.bytes)} · Archive · Review required</small></span><button class="ws-btn danger" data-review-trash="${f.id}">Review</button></div>`).join('')||'<p>No archive cleanup candidates in this sample folder. Choose Downloads or Studio SSD to explore more sample files.</p>'}</section>`;
  }
  return `${heading}${tabs}<div role="tabpanel" id="space-panel" aria-labelledby="space-tab-${selected}">${panel}</div>${wsNote('Sample analysis only. Bubble area represents relative size; small items have a minimum display size. No native scan or disk access takes place.')}`;
}
function formatSize(bytes) {
  if(bytes>=1e9) return `${(bytes/1e9).toFixed(1)} GB`;
  if(bytes>=1e6) return `${(bytes/1e6).toFixed(1)} MB`;
  if(bytes>=1e3) return `${(bytes/1e3).toFixed(1)} KB`;
  return `${bytes} B`;
}
function backupsPreview() {
  const source=find(workspacePreview.backupSource), items=descendants(source.id).filter(f=>!isFolder(f));
  const total=items.reduce((n,f)=>n+f.bytes,0);
  return `${wsHeading('Backups','',wsButton('New backup','new-backup','plus'))}
    <section class="ws-card"><div class="backup-top"><div class="backup-title">${icon('backup')}<div><h2>${escapeHtml(workspacePreview.backupName)} <span class="ws-badge">Up to date · Demo</span></h2><p>${escapeHtml(source.name)} → ${escapeHtml(workspacePreview.backupDestination)} / Rile Backup</p></div></div><div class="ws-actions">${wsButton('Back up now','backup-now','right','primary')}${wsButton('Restore…','restore','backup')}${wsButton('Edit','edit-backup','settings')}</div></div>
    <p id="backup-summary">Last sample backup ${workspacePreview.backupRuns>1?'just now':'5 min ago'} — ${items.length} files copied (${formatSize(total)})</p>
    <div class="backup-meta"><span>${icon('clock')} Manual</span><span>Keep versions 30 days</span><span>${items.length} sample files</span></div>
    <div class="backup-runs"><h3>Recent runs</h3>${Array.from({length:Math.min(workspacePreview.backupRuns,4)},(_,i)=>`<div class="ws-row">${icon('clock')}<span>${i===0&&workspacePreview.backupRuns>1?'Just now':'Oct 28, 13:07'}<small>Manual · Simulated run</small></span><strong>${items.length} files · ${formatSize(total)}</strong><span class="ws-badge">Completed</span></div>`).join('')}</div></section>
    ${wsNote('Backups in the native app are plain folders, with replaced and deleted files retained in .rile-versions. This preview simulates run history and restore confirmation; it never copies or restores real files.')}`;
}
function photosPreview() {
  const query=tab().query.toLowerCase().trim();
  const photos=files.filter(f=>['image','screen'].includes(f.kind)&&f.name.toLowerCase().includes(query));
  return `${wsHeading('Photos','A month-by-month view of your images. These are illustrative previews, not your photo library.')}
    <h2 class="photo-month">October 2026 <span class="ws-kicker">${photos.length} images</span></h2><div class="photo-grid">${photos.map(f=>`<button class="photo-tile" data-photo-preview="${f.id}" aria-label="Preview ${escapeHtml(f.name)}">${landscape()}<span>${escapeHtml(f.name)}<small>${f.dimensions||'Sample image'} · ${fileSize(f)}</small></span></button>`).join('')}</div>${photos.length?'':`<section class="ws-card ws-empty">${icon('search')}<h2>No matching photos</h2><p>Try another filename or clear your search.</p></section>`}${wsNote('Photo thumbnails use original placeholder artwork. Open an image for an illustrative preview or reveal it in the file explorer.')}`;
}
function networkPreview() {
  return `${wsHeading('Network','Connect to a server without leaving your workspace.',wsButton('Connect to server','connect','plus','primary'))}
    <section class="ws-card ws-empty">${icon('server')}<h2>Your files, wherever they live.</h2><p>The native app supports FTP, FTPS, SFTP, and SMB connections, with saved credentials and nearby-server discovery.</p>${wsButton('Connect to server','connect','server','primary')}</section>${wsNote('Connection forms are illustrative. No credentials are saved and no network request is made by this prototype.')}`;
}
function settingsPreview() {
  const accent=document.body.dataset.accent||'Blue', density=document.body.dataset.density||'comfortable';
  return `${wsHeading('Appearance','A familiar workspace, made yours. Preview changes apply only to this browser session.')}
    <section class="ws-card"><h2>Interface</h2><div class="settings-row"><div><strong>Color theme</strong><p>Graphite surfaces, consistent across app and library pages.</p></div><span class="ws-badge">Dark</span></div>
    <div class="settings-row"><div><strong>Accent color</strong><p>Primary actions and keyboard focus.</p></div><div class="accent-choices" role="group" aria-label="Accent color">${[['Blue','#86baf5'],['Violet','#b7a4ef'],['Mint','#83cfb4'],['Peach','#e6b28d']].map(([name,color])=>`<button style="--swatch:${color}" data-accent="${name}" data-color="${color}" aria-label="${name}" aria-pressed="${accent===name}" title="${name}"></button>`).join('')}</div></div>
    <div class="settings-row"><div><label for="preview-density">Content density</label><p>Adjust sidebar and file-list row spacing.</p></div><select id="preview-density"><option value="comfortable" ${density==='comfortable'?'selected':''}>Comfortable</option><option value="compact" ${density==='compact'?'selected':''}>Compact</option></select></div>
    <div class="settings-row"><div><strong>Platform preview</strong><p>Shared Rile controls with platform-specific window chrome.</p></div><a class="ws-btn" href="?page=settings${window.rileWindows?'':'&platform=windows'}">${window.rileWindows?'Switch to macOS':'Switch to Windows'}</a></div></section>
    ${wsNote('A focused subset of the current app’s appearance settings. Native theme, Dock icons, startup preferences, and plugin settings remain in the application.')}`;
}
function backupForm(editing) {
  workspaceDialog(editing?'Edit sample backup':'New sample backup',`<p>Configure an illustrative backup. No files will be copied.</p><form class="ws-form" id="preview-backup-form"><label>Name<input name="name" required maxlength="60" value="${editing?escapeHtml(workspacePreview.backupName):''}" placeholder="Project archive"></label><label>Source folder<select name="source">${['rile','projects','documents','pictures'].map(id=>`<option value="${id}" ${editing&&workspacePreview.backupSource===id?'selected':''}>${escapeHtml(locationName(id))}</option>`).join('')}</select></label><label>Destination<select name="destination">${['Studio SSD','Local backup folder'].map(name=>`<option ${editing&&workspacePreview.backupDestination===name?'selected':''}>${name}</option>`).join('')}</select></label><div class="ws-actions">${wsButton('Cancel','close-dialog','','ghost')}<button class="ws-btn primary" type="submit">Save sample backup</button></div></form>`);
}
document.addEventListener('click',event=>{
  const button=event.target.closest('button'); if(!button) return;
  const action=button.dataset.workspaceAction;
  if(action==='close-dialog') $('#workspace-dialog').close();
  if(action==='choose-root') workspaceDialog('Choose a folder to analyze',`<p>Select a folder from the sample workspace. Nothing on your device is scanned.</p><div class="workspace-root-options">${['rile','projects','downloads','ssd'].map(id=>`<button class="ws-btn" data-analyze-folder="${id}">${icon('folder')}${escapeHtml(locationName(id))}</button>`).join('')}</div><div class="ws-actions" style="margin-top:22px">${wsButton('Cancel','close-dialog','','ghost')}</div>`);
  if(button.dataset.analyzeFolder) { workspacePreview.root=button.dataset.analyzeFolder; workspacePreview.spaceTab='overview'; $('#workspace-dialog').close(); renderFiles(); }
  if(action==='rescan') { renderFiles(); toast('Sample analysis refreshed. No disk scan was performed.'); }
  if(button.dataset.spaceTab) { workspacePreview.spaceTab=button.dataset.spaceTab; renderFiles(); $(`[data-space-tab="${workspacePreview.spaceTab}"]`).focus(); }
  if(action==='backup-now') { workspacePreview.backupRuns++; renderFiles(); toast('Simulated backup completed. No files were copied.'); }
  if(action==='new-backup'||action==='edit-backup') backupForm(action==='edit-backup');
  if(action==='restore') workspaceDialog('Restore from a sample backup',`<p>Restore ${escapeHtml(workspacePreview.backupName)} from its latest sample run? The native app lets you choose original locations or another folder.</p><p><strong>Preview only:</strong> no real files will be read, overwritten, or restored.</p><div class="ws-actions">${wsButton('Cancel','close-dialog','','ghost')}${wsButton('Simulate restore','confirm-restore','backup','primary')}</div>`);
  if(action==='confirm-restore') { $('#workspace-dialog').close(); toast('Sample restore completed. Your files are untouched.'); }
  if(button.dataset.photoPreview) {
    const f=find(button.dataset.photoPreview); workspacePreview.photoSelection=f.id;
    workspaceDialog(escapeHtml(f.name),`${landscape()}<p>${f.dimensions||'Sample image'} · ${fileSize(f)} · Illustrative artwork</p><div class="ws-actions">${wsButton('Close','close-dialog','','ghost')}${wsButton('Show in folder','reveal-photo','folder')}</div>`);
  }
  if(action==='reveal-photo') { const f=find(workspacePreview.photoSelection); $('#workspace-dialog').close(); navigate(f.parent); selectFile(f.id); }
  if(button.dataset.reviewTrash) {
    const f=find(button.dataset.reviewTrash);
    workspaceDialog('Remove sample archive?',`<p>Remove <strong>${escapeHtml(f.name)}</strong> (${fileSize(f)}) from the in-memory sample workspace? This lasts until reload. No file on your device will be changed.</p><div class="ws-actions">${wsButton('Cancel','close-dialog','','ghost')}<button class="ws-btn danger" data-confirm-trash="${f.id}">Remove sample file</button></div>`);
  }
  if(button.dataset.confirmTrash) { const index=files.findIndex(f=>f.id===button.dataset.confirmTrash); if(index>=0)files.splice(index,1); $('#workspace-dialog').close(); renderFiles(); toast('Sample file removed. Your files are untouched.'); }
  if(action==='connect') workspaceDialog('Connect to server',`<p>Preview form only. Do not enter real credentials.</p><form id="preview-connect-form" class="ws-form"><label>Protocol<select name="protocol"><option>SFTP</option><option>FTP</option><option>FTPS</option><option>SMB</option></select></label><label>Server<input name="server" placeholder="server.example.com" required></label><label>Username<input name="username" placeholder="Your username" autocomplete="off"></label><div class="ws-actions">${wsButton('Cancel','close-dialog','','ghost')}<button type="submit" class="ws-btn primary">Preview connection</button></div></form>`);
  if(button.dataset.accent) { document.body.dataset.accent=button.dataset.accent; document.documentElement.style.setProperty('--preview-accent',button.dataset.color); renderFiles(); $(`[data-accent="${button.dataset.accent}"]`).focus(); }
});
document.addEventListener('submit',event=>{
  if(event.target.id==='preview-backup-form') {
    event.preventDefault(); const data=new FormData(event.target);
    const name=String(data.get('name')).trim(); if(!name) { event.target.elements.name.setCustomValidity('Enter a backup name.'); event.target.reportValidity(); return; }
    workspacePreview.backupName=name; workspacePreview.backupSource=String(data.get('source')); workspacePreview.backupDestination=String(data.get('destination')); workspacePreview.backupRuns=1;
    $('#workspace-dialog').close(); renderFiles(); toast('Sample backup saved for this session.');
  }
  if(event.target.id==='preview-connect-form') { event.preventDefault(); $('#workspace-dialog').close(); toast('Connection preview complete. No network request or credential storage.'); }
});
document.addEventListener('input',event=>{ if(event.target.name==='name') event.target.setCustomValidity(''); });
document.addEventListener('change',event=>{ if(event.target.id==='preview-density') document.body.dataset.density=event.target.value; });
document.addEventListener('keydown',event=>{
  if(!event.target.matches('[data-space-tab]')||!['ArrowLeft','ArrowRight','Home','End'].includes(event.key))return;
  event.preventDefault(); const tabs=['overview','files','cleanup'];
  const index=tabs.indexOf(workspacePreview.spaceTab);
  workspacePreview.spaceTab=tabs[event.key==='Home'?0:event.key==='End'?2:(index+(event.key==='ArrowRight'?1:2))%3];
  renderFiles(); $(`[data-space-tab="${workspacePreview.spaceTab}"]`).focus();
});
if(window.rileWindows) { $('#platform-link').href='?'; $('#platform-link').textContent='macOS ↗'; }
