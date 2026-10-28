const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');

// Exercise the real sample model and renderers without packages or disk access
// from the preview. Browser layout and interactions are checked separately.
function preview(windows = false) {
  const elements = new Map();
  const element = selector => {
    if (!elements.has(selector)) elements.set(selector, {
      dataset:{}, style:{setProperty() {}}, classList:{toggle() {},add() {},remove() {}},
      addEventListener() {}, setAttribute() {}, removeAttribute() {},
      showModal() {}, close() {}, innerHTML:'', textContent:'',
    });
    return elements.get(selector);
  };
  const context = vm.createContext({
    window:{rileWindows:windows}, location:{search:'',hash:''}, URLSearchParams,
    matchMedia:()=>({matches:false}), setTimeout, clearTimeout,
    document:{querySelector:element,querySelectorAll:()=>[],addEventListener() {},body:element('body'),documentElement:element('html')},
  });
  for (const file of ['start.js','overview-art.js','icons.js','explorer.js','workspace-pages.js']) {
    vm.runInContext(fs.readFileSync(path.join(__dirname,file),'utf8'),context,{filename:file});
  }
  return {run:code=>vm.runInContext(code,context),element};
}

test('platform variants use correct internal and external filesystem paths', () => {
  const mac=preview(), windows=preview(true);
  assert.equal(mac.run("pathString('rile')"),'/Users/rickyperlick/Projects/rile');
  assert.equal(windows.run("pathString('rile')"),'C:\\Users\\rickyperlick\\Projects\\rile');
  assert.equal(windows.run("pathString('ssd')"),'D:\\');
  assert.equal(windows.run('sampleVolumes[0].description'),'Internal · NTFS');
});
test('current library pages have names and shared icon vocabulary', () => {
  const p=preview();
  for (const id of ['start','photos','space','backups','network','settings']) {
    assert.equal(p.run(`Boolean(icons[libraryIcons['${id}']])`),true,id);
    assert.equal(p.run(`pathNodes('${id}').length`),1,id);
  }
});
test('Home renders six metrics with current navigation and sample disclaimers', () => {
  const p=preview(); p.run('renderStartPage()');
  const html=p.element('#file-area').innerHTML;
  assert.equal((html.match(/class="start-metric"/g)||[]).length,6);
  assert.match(html,/Recent activity/);
  assert.match(html,/data-navigate="backups"/);
  assert.match(html,/illustrative/);
});
test('Space Analyzer requires explicit folder selection and calculates recursive sizes', () => {
  const p=preview();
  assert.match(p.run('spacePreview()'),/Nothing is scanned until you choose a folder/);
  assert.doesNotMatch(p.run('spacePreview()'),/class="bubble-map"/);
  p.run("workspacePreview.root='rile'");
  assert.equal(p.run("previewBytes(find('rile'))"),p.run("descendants('rile').filter(f=>!isFolder(f)).reduce((n,f)=>n+f.bytes,0)"));
  assert.match(p.run('spacePreview()'),/class="bubble-map"/);
  p.run("workspacePreview.spaceTab='files'");
  assert.match(p.run('spacePreview()'),/Largest files/);
});
test('cleanup candidates are review-only until confirmed', () => {
  const p=preview(); p.run("workspacePreview.root='ssd'; workspacePreview.spaceTab='cleanup'");
  assert.match(p.run('spacePreview()'),/data-review-trash="backup-zip"/);
  assert.equal(p.run("Boolean(find('backup-zip'))"),true);
  assert.match(p.run('spacePreview()'),/not automatically safe to delete/);
});
test('backup history and names are session data with HTML escaping', () => {
  const p=preview(); p.run("workspacePreview.backupName='<unsafe>'; workspacePreview.backupRuns=3");
  const html=p.run('backupsPreview()');
  assert.match(html,/&lt;unsafe&gt;/);
  assert.equal((html.match(/Simulated run/g)||[]).length,3);
  assert.match(html,/never copies or restores real files/);
});
test('Photos filters filenames and handles empty results', () => {
  const p=preview();
  assert.match(p.run('photosPreview()'),/data-photo-preview/);
  p.run("tab().query='no-such-photo'");
  assert.match(p.run('photosPreview()'),/No matching photos/);
});
test('all current pages render in both platform variants', () => {
  for(const windows of [false,true]) {
    const p=preview(windows);
    for(const id of ['photos','space','backups','network','settings']) {
      p.run(`tab().history=['${id}']; tab().cursor=0; renderWorkspacePage()`);
      const html=p.element('#file-area').innerHTML;
      assert.match(html,/workspace-heading/);
      assert.doesNotMatch(html,/undefined|NaN/);
    }
  }
});
