/* Start-page analytics: disk figures/history are illustrative; file counts
   and type distributions are calculated from the in-memory sample workspace. */
const sampleVolumes = [
  {id:'disk',name:'Macintosh HD',description:'Internal · APFS',used:284,total:512,icon:'laptop',color:'#5daaff'},
  {id:'ssd',name:'Studio SSD',description:'External · USB-C',used:732,total:2000,icon:'drive',color:'#a190e8'},
];
const formatGB = value => value>=1000 ? `${(value/1000).toFixed(2)} TB` : `${value} GB`;
function startStats() {
  const discovered=files.filter(f=>!isFolder(f));
  const groups=[
    {name:'Images',color:'#64adf4',kinds:['image','screen','svg']},
    {name:'Code',color:'#9d8de0',kinds:['rust','react','css','toml','json']},
    {name:'Documents',color:'#65b8a6',kinds:['markdown','pdf','text']},
    {name:'Archives',color:'#d9a46d',kinds:['archive']},
  ].map(group=>({...group,count:discovered.filter(f=>group.kinds.includes(f.kind)).length}));
  return {discovered,groups,folders:files.filter(f=>isFolder(f)&&f.parent).length,used:sampleVolumes.reduce((n,v)=>n+v.used,0),total:sampleVolumes.reduce((n,v)=>n+v.total,0)};
}
function discoveryGraph(count,polished=false) {
  const values=[.12,.18,.18,.37,.48,.51,.63,.66,.8,.87,.87,1].map(value=>Math.round(value*count));
  const coordinates=values.map((value,i)=>({x:36+i*43,y:142-value/Math.max(count,1)*112}));
  const points=coordinates.map(point=>`${point.x},${point.y}`).join(' ');
  const curve=coordinates.map((point,i)=>i===0?`M ${point.x} ${point.y}`:`C ${(coordinates[i-1].x+point.x)/2} ${coordinates[i-1].y}, ${(coordinates[i-1].x+point.x)/2} ${point.y}, ${point.x} ${point.y}`).join(' ');
  const last=coordinates[coordinates.length-1];
  const plot=polished?`<path class="chart-area" d="${curve} L 509 142 L 36 142 Z" fill="url(#discovery-fill)"/><path class="chart-line" d="${curve}"/><circle class="chart-halo" cx="${last.x}" cy="${last.y}" r="8"/><circle class="chart-end" cx="${last.x}" cy="${last.y}" r="3.5"/>`:`<polygon points="36,142 ${points} 509,142" fill="url(#discovery-fill)"/><polyline points="${points}"/><circle cx="${last.x}" cy="${last.y}" r="3.5"/>`;
  return `<svg class="discovery-chart" viewBox="0 0 540 185" role="img" aria-label="Illustrative seven-day discovery history, increasing to ${count} sample files"><defs><linearGradient id="discovery-fill" x1="0" y1="0" x2="0" y2="1"><stop stop-color="#5aa7f5" stop-opacity="${polished?'.30':'.22'}"/><stop offset="1" stop-color="#5aa7f5" stop-opacity="0"/></linearGradient></defs>${[30,86,142].map((y,i)=>`<line x1="36" x2="509" y1="${y}" y2="${y}"/><text x="24" y="${y+3}" text-anchor="end">${Math.round(count*(1-i/2))}</text>`).join('')}${plot}${['Oct 21','Oct 22','Oct 23','Oct 24','Oct 25','Oct 26','Oct 27'].map((label,i)=>`<text x="${36+i*78.8}" y="171" text-anchor="${i===0?'start':i===6?'end':'middle'}">${label}</text>`).join('')}</svg>`;
}
function renderStartPage() {
  const stats=startStats(), count=stats.discovered.length;
  const metric=(name,value,unit,detail,art)=>`<article class="start-metric"><span class="metric-icon">${overviewArt(art,`overview-${art}`)}</span><div><p>${name}</p><strong>${value}<small>${unit}</small></strong><span>${detail}</span></div></article>`;
  const query=tab().query.trim();
  const recent=(query?currentFiles().filter(f=>!isFolder(f)):stats.discovered.slice().sort((a,b)=>b.modified.localeCompare(a.modified)).slice(0,5));
  $('#file-area').dataset.view='start';
  $('#file-area').innerHTML=`<div class="start-page overview-page${query?' searching':''}">
    <header class="start-heading"><div><p class="start-eyebrow">YOUR WORKSPACE, AT A GLANCE</p><h1>Home<span>.</span></h1><p>A little perspective on everything you keep.</p></div><span class="sample-badge"><i></i> Sample workspace</span></header>
    <section class="start-metrics" aria-label="Workspace statistics">
      ${metric('Storage used',formatGB(stats.used).split(' ')[0],formatGB(stats.used).split(' ')[1],`of ${formatGB(stats.total)} across 2 disks`,'drive')}
      ${metric('Files discovered',count.toLocaleString(),'','in the sample workspace','files')}
      ${metric('Folders',stats.folders,'','across your sample locations','folder')}
      ${metric('Connected disks',sampleVolumes.length,'','1 internal · 1 external','devices')}
    </section>
    <div class="start-charts">
      <section class="start-panel discovery-panel"><header><div><h2>File discovery</h2><p>Sample history · last 7 days</p></div><span class="chart-period">7 days</span></header><div class="discovery-summary"><strong>${count}</strong><span>files discovered <i>in this demo</i></span></div>${discoveryGraph(count,true)}</section>
      <section class="start-panel types-panel"><header><div><h2>A closer look</h2><p>Your sample files, by kind</p></div>${icon('grid')}</header><div class="file-type-chart">${overviewDonut(stats.groups,count)}<ul>${stats.groups.map(group=>`<li><i style="background:${group.color}"></i><span>${group.name}</span><strong>${group.count}</strong></li>`).join('')}</ul></div></section>
    </div>
    <section class="start-storage" aria-labelledby="storage-heading"><div class="start-section-heading"><h2 id="storage-heading">Your storage</h2><span>Illustrative disk usage</span></div><div class="storage-cards">${sampleVolumes.map(volume=>`<button class="storage-card" data-navigate="${volume.id}" aria-label="Browse ${volume.name}"><div class="storage-card-top"><span class="volume-icon" style="--volume-color:${volume.color}">${overviewArt(volume.id==='disk'?'devices':'ssd',`overview-volume-${volume.id}`)}</span><span><strong>${volume.name}</strong><small>${volume.description}</small></span><span class="volume-status"><i></i>Connected</span>${icon('right')}</div><div class="volume-usage"><span><strong>${volume.used} GB</strong> used</span><span>${volume.total-volume.used} GB free</span></div><div class="volume-meter" role="meter" aria-label="${volume.name} storage used in GB" aria-valuemin="0" aria-valuemax="${volume.total}" aria-valuenow="${volume.used}" style="--volume-color:${volume.color}"><i style="width:${volume.used/volume.total*100}%"></i></div><div class="volume-caption"><span>${Math.round(volume.used/volume.total*100)}% used</span><span>${formatGB(volume.total)} capacity</span></div></button>`).join('')}</div></section>
    <div class="start-bottom"><section class="start-panel start-recent"><header><div><h2>${query?'Search results':'Recently modified'}</h2><p>${query?`Matching “${escapeHtml(query)}” across sample files`:'Pick up where you left off'}</p></div>${query?'':`<button data-navigate="recents">View all ${icon('right')}</button>`}</header><div class="recent-files">${recent.length?recent.map(f=>`<button data-start-file="${f.id}" class="start-recent-row" aria-label="Show ${escapeHtml(f.name)} in folder"><span class="recent-file-icon">${icon(fileIcon(f))}</span><span class="recent-file-name"><strong>${escapeHtml(f.name)}</strong><small>${escapeHtml(locationName(f.parent))}</small></span><span class="recent-file-size">${fileSize(f)}</span><span class="recent-file-date">${modified(f)}</span>${icon('right')}</button>`).join(''):'<p class="start-empty">No sample files match your search.</p>'}</div></section><section class="start-panel start-shortcuts"><header><div><h2>Jump back in</h2><p>Your everyday places</p></div>${icon('folder')}</header>${['projects','downloads','pictures','home'].map(id=>`<button data-navigate="${id}" aria-label="Open ${escapeHtml(locationName(id))}">${icon(find(id)?.placeIcon||'folder')}<span>${escapeHtml(locationName(id))}</span>${icon('right')}</button>`).join('')}<p>Browse directly. No indexing required.</p></section></div>
    <footer class="start-footnote">${icon('info')} This is a design preview. File counts come from sample data; disk usage and discovery history are illustrative.</footer>
  </div>`;
  $('#item-count').textContent=`${count} sample files · ${sampleVolumes.length} disks`;
}
