/* Small original vector illustrations for the Overview. Decorative only:
   the adjacent text contains the accessible metric or device name. */
function overviewArt(kind,id) {
  const shell=(defs,content)=>`<svg class="overview-art" viewBox="0 0 64 64" aria-hidden="true" focusable="false"><defs>${defs}</defs>${content}</svg>`;
  if(kind==='drive') return shell(
    `<linearGradient id="${id}-frame" x1="8" y1="5" x2="55" y2="57" gradientUnits="userSpaceOnUse"><stop stop-color="#607790"/><stop offset=".5" stop-color="#35475d"/><stop offset="1" stop-color="#202b3b"/></linearGradient><radialGradient id="${id}-core"><stop stop-color="#d1edff"/><stop offset=".35" stop-color="#85c6fa"/><stop offset="1" stop-color="#2e80cc"/></radialGradient>`,
    `<path d="M30 5a4 4 0 0 1 4 0l19 11a4 4 0 0 1 2 3.5v22a4 4 0 0 1-2 3.5L34 56a4 4 0 0 1-4 0L11 45a4 4 0 0 1-2-3.5v-22a4 4 0 0 1 2-3.5Z" fill="url(#${id}-frame)" stroke="#8194a9" stroke-opacity=".35"/><path d="m32 13 15 8.5v17L32 47 17 38.5v-17Z" fill="#17283b" stroke="#91b4d4" stroke-opacity=".35"/><circle cx="32" cy="30" r="10" fill="#27537a" stroke="#7ab5e6" stroke-opacity=".7"/><circle cx="32" cy="30" r="6.2" fill="url(#${id}-core)"/><circle cx="29.8" cy="27.7" r="1.5" fill="#e1f4ff" opacity=".8"/>`);
  if(kind==='files') return shell(
    `<linearGradient id="${id}-paper" x1="16" y1="7" x2="45" y2="55" gradientUnits="userSpaceOnUse"><stop stop-color="#aec8e6"/><stop offset="1" stop-color="#617eae"/></linearGradient><linearGradient id="${id}-front" x1="23" y1="11" x2="48" y2="53" gradientUnits="userSpaceOnUse"><stop stop-color="#e1edff"/><stop offset="1" stop-color="#8daed1"/></linearGradient>`,
    `<path d="M14 17a4 4 0 0 1 4-4h20l8 8v27a4 4 0 0 1-4 4H18a4 4 0 0 1-4-4Z" fill="#42536e" stroke="#7a90b1" stroke-opacity=".5" transform="rotate(-9 30 33)"/><path d="M21 10a4 4 0 0 1 4-4h17l8 8v31a4 4 0 0 1-4 4H25a4 4 0 0 1-4-4Z" fill="url(#${id}-paper)"/><path d="M25 14a4 4 0 0 1 4-4h13l8 8v29a4 4 0 0 1-4 4H29a4 4 0 0 1-4-4Z" fill="url(#${id}-front)" stroke="#fff" stroke-opacity=".25"/><path d="M42 10v7a2 2 0 0 0 2 2h6" fill="#7199c3" opacity=".8"/><path d="M31 27h12M31 32h12M31 37h7" fill="none" stroke="#557ba8" stroke-width="2" stroke-linecap="round" opacity=".8"/><circle cx="45" cy="45" r="8" fill="#324e72" stroke="#b9dfff" stroke-width="1.6"/><circle cx="44" cy="44" r="3" fill="none" stroke="#d7edff" stroke-width="1.7"/><path d="m46.5 46.5 2.5 2.5" fill="none" stroke="#d7edff" stroke-width="1.7" stroke-linecap="round"/>`);
  if(kind==='folder') return shell(
    `<linearGradient id="${id}-back" x1="8" y1="11" x2="56" y2="50" gradientUnits="userSpaceOnUse"><stop stop-color="#64c9ff"/><stop offset="1" stop-color="#2074c6"/></linearGradient><linearGradient id="${id}-front" x1="9" y1="22" x2="53" y2="52" gradientUnits="userSpaceOnUse"><stop stop-color="#7ad1ff"/><stop offset=".55" stop-color="#359eea"/><stop offset="1" stop-color="#1876c8"/></linearGradient>`,
    `<path d="M7 18a5 5 0 0 1 5-5h12l5 5h22a5 5 0 0 1 5 5v24a5 5 0 0 1-5 5H12a5 5 0 0 1-5-5Z" fill="url(#${id}-back)" stroke="#98ddff" stroke-opacity=".4"/><path d="M7 26a5 5 0 0 1 5-5h39a5 5 0 0 1 5 5v20a6 6 0 0 1-6 6H13a6 6 0 0 1-6-6Z" fill="url(#${id}-front)"/><path d="M12 23h38" fill="none" stroke="#d8f4ff" stroke-opacity=".55" stroke-linecap="round"/><path d="M13 49h37" fill="none" stroke="#135a9c" stroke-opacity=".5" stroke-linecap="round"/>`);
  if(kind==='ssd') return shell(
    `<linearGradient id="${id}-case" x1="10" y1="11" x2="52" y2="54" gradientUnits="userSpaceOnUse"><stop stop-color="#c0c8d0"/><stop offset=".45" stop-color="#7b8998"/><stop offset="1" stop-color="#4b5968"/></linearGradient>`,
    `<rect x="12" y="10" width="40" height="45" rx="9" fill="url(#${id}-case)" stroke="#dce8f0" stroke-opacity=".35"/><rect x="16" y="14" width="32" height="37" rx="6" fill="#728394" fill-opacity=".45" stroke="#d1e1ed" stroke-opacity=".25"/><path d="M21 19h22" stroke="#d7e5ed" stroke-opacity=".35" stroke-linecap="round"/><rect x="27" y="43" width="10" height="2" rx="1" fill="#b4c9d5" opacity=".6"/><circle cx="43" cy="46" r="1.4" fill="#98d8ff"/>`);
  return shell(
    `<linearGradient id="${id}-screen" x1="13" y1="12" x2="51" y2="43" gradientUnits="userSpaceOnUse"><stop stop-color="#a4d8fb"/><stop offset=".48" stop-color="#4e9cdb"/><stop offset="1" stop-color="#285587"/></linearGradient><linearGradient id="${id}-base" x1="7" y1="43" x2="56" y2="52" gradientUnits="userSpaceOnUse"><stop stop-color="#b3c0cd"/><stop offset=".5" stop-color="#7f91a2"/><stop offset="1" stop-color="#566778"/></linearGradient>`,
    `<rect x="10" y="9" width="44" height="34" rx="5" fill="#273747" stroke="#9db4c7" stroke-opacity=".65"/><rect x="13" y="12" width="38" height="27" rx="2" fill="url(#${id}-screen)"/><path d="M13 35 28 19l11 11 5-5 7 8v6H13Z" fill="#b4dcf2" opacity=".24"/><path d="M10 43h44l5 6a2 2 0 0 1-2 3H7a2 2 0 0 1-2-3Z" fill="url(#${id}-base)" stroke="#d5e1e9" stroke-opacity=".35"/><path d="M24 46h16l-1 2H25Z" fill="#455c70" opacity=".8"/><path d="M8 50h48" fill="none" stroke="#e5f2fa" stroke-opacity=".35"/>`);
}

function overviewDonut(groups,count) {
  const circumference=2*Math.PI*58; let offset=0;
  const segments=groups.map(group=>{
    const length=group.count/Math.max(count,1)*circumference;
    const dash=Math.max(0,length-2.5),start=offset;offset+=length;
    return length ? `<circle class="donut-segment" cx="70" cy="70" r="58" stroke="${group.color}" stroke-dasharray="${dash.toFixed(2)} ${(circumference-dash).toFixed(2)}" stroke-dashoffset="${(-start).toFixed(2)}"/>` : '';
  }).join('');
  return `<div class="type-donut overview-donut" role="img" aria-label="${groups.map(g=>`${g.name}: ${g.count}`).join(', ')}"><svg class="overview-donut-ring" viewBox="0 0 140 140" aria-hidden="true"><circle class="donut-track" cx="70" cy="70" r="58"/>${segments}</svg><div><strong>${count}</strong><span>files</span></div></div>`;
}
