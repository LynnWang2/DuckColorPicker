import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { enable as enableAutostart, disable as disableAutostart } from '@tauri-apps/plugin-autostart';

const $ = selector => document.querySelector(selector);
const FORMAT_LABELS = { hex: 'HEX', rgb: 'RGB', hsl: 'HSL', cmyk: 'CMYK' };
let appState = { shortcut: '', history: [], theme: 'system', formats: {}, copyFormat: 'hex' };
let latest;

function humanShortcut(value) { return value.replace('CommandOrControl','Ctrl / ⌘').replaceAll('+',' + '); }
function copyValue(value) { return String(value).replace(/^#/, ''); }
function toast(message) { const el=$('#toast'); el.textContent=message; el.classList.add('show'); setTimeout(()=>el.classList.remove('show'),1500); }
function esc(value) { const el=document.createElement('span'); el.textContent=String(value); return el.innerHTML; }
function enabledFormats() { return Object.keys(FORMAT_LABELS).filter(key => appState.formats[key]); }
function setTheme(theme) { if(theme==='system')document.documentElement.removeAttribute('data-theme');else document.documentElement.dataset.theme=theme; }
function valueRows(color) {
  const formats=enabledFormats();
  if(!formats.length)return '<p class="noFormats">已隐藏全部色值，可在“显示色值”中开启</p>';
  return formats.map(key=>`<button class="valueRow" data-copy="${esc(copyValue(color[key]))}"><span>${FORMAT_LABELS[key]}</span><b>${esc(color[key])}</b><em>复制</em></button>`).join('');
}
function historyDetails(color) { const values=enabledFormats().map(key=>color[key]).filter(Boolean);return values.length?values.join(' · '):'色值已隐藏'; }
function render(state) {
  appState=state;setTheme(state.theme);$('#shortcut').value=humanShortcut(state.shortcut);$('#shortcutLabel').textContent=humanShortcut(state.shortcut);$('#theme').value=state.theme;$('#copyFormat').value=state.copyFormat||'hex';$('#autoStart').checked=state.autoStart;
  document.querySelectorAll('[data-format]').forEach(input=>{input.checked=Boolean(state.formats[input.dataset.format])});
  $('#count').textContent=`${state.history.length} 个`;
  $('#history').innerHTML=state.history.length?state.history.map(item=>`<article class="historyItem"><div class="swatch" style="background:${esc(item.hex)}"></div><div class="historyMeta"><b>${esc(item.name)}</b><p>${esc(historyDetails(item))}</p></div><div class="actions"><button data-copy="${esc(copyValue(item.hex))}">复制</button><button data-delete="${esc(item.id)}">×</button></div></article>`).join(''):'<div class="empty"><span>◌</span><b>还没有取过颜色</b><p>第一次取色会显示在这里</p></div>';
  if(latest)$('#latestValues').innerHTML=valueRows(latest);
}
async function updatePreferences(changes){try{await invoke('update_preferences',{preferences:changes})}catch(error){toast(String(error))}}
async function startPick(){try{$('#hint').textContent='移动鼠标并单击取色，按 Esc 取消';await invoke('start_picker')}catch(error){$('#hint').textContent='无法启动取色器';toast(String(error))}}
function acceleratorFromEvent(event){const mods=[];if(event.ctrlKey)mods.push('Control');if(event.metaKey)mods.push('Command');if(event.altKey)mods.push('Alt');if(event.shiftKey)mods.push('Shift');if(['Control','Meta','Alt','Shift'].includes(event.key))return null;const key=event.key===' '?'Space':event.key.length===1?event.key.toUpperCase():event.key;return mods.length?[...mods,key].join('+'):null}

$('#pick').addEventListener('click',startPick);
$('#shortcut').addEventListener('focus',e=>{e.target.classList.add('recording');e.target.value='请按组合键…'});
$('#shortcut').addEventListener('blur',e=>{e.target.classList.remove('recording');e.target.value=humanShortcut(appState.shortcut)});
$('#shortcut').addEventListener('keydown',async event=>{event.preventDefault();const value=acceleratorFromEvent(event);if(!value)return;try{await invoke('save_shortcut',{shortcut:value});toast('快捷键已更新')}catch(error){toast(String(error))}$('#shortcut').blur()});
$('#theme').addEventListener('change',event=>updatePreferences({theme:event.target.value}));
$('#copyFormat').addEventListener('change',event=>updatePreferences({copyFormat:event.target.value}));
$('#autoStart').addEventListener('change',async event=>{try{event.target.checked?await enableAutostart():await disableAutostart();await updatePreferences({autoStart:event.target.checked})}catch(error){event.target.checked=!event.target.checked;toast(String(error))}});
document.querySelectorAll('[data-format]').forEach(input=>input.addEventListener('change',()=>{const formats=Object.fromEntries([...document.querySelectorAll('[data-format]')].map(item=>[item.dataset.format,item.checked]));updatePreferences({formats})}));
$('#clear').addEventListener('click',async()=>{if(appState.history.length){await invoke('clear_history');toast('历史已清空')}});
$('#latestValues').addEventListener('click',async event=>{const target=event.target.closest('[data-copy]');if(target){await invoke('copy_value',{value:target.dataset.copy});toast(`已复制 ${target.dataset.copy}`)}});
$('#history').addEventListener('click',async event=>{const target=event.target.closest('button');if(!target)return;if(target.dataset.copy){await invoke('copy_value',{value:target.dataset.copy});toast(`已复制 ${target.dataset.copy}`)}if(target.dataset.delete)await invoke('delete_history',{id:target.dataset.delete})});

async function init(){
  await listen('state-changed',event=>render(event.payload));
  await listen('picker-cancelled',()=>{$('#hint').textContent='已取消取色'});
  await listen('picker-result',event=>{const {color,copiedFormat,copiedValue}=event.payload;latest=color;$('#latest').classList.remove('hidden');$('#latestSwatch').style.background=color.hex;$('#latestName').textContent=color.name;$('#latestValues').innerHTML=valueRows(color);const label=FORMAT_LABELS[copiedFormat]||'HEX';$('#latestCopyHint').textContent=`已自动复制 ${label}${copiedFormat==='hex'?'（不含 #）':''}`;$('#hint').textContent=`已复制 ${label} 色值，可继续取色`;toast(`已复制 ${copiedValue}`)});
  render(await invoke('get_state'));
}
init().catch(error=>toast(String(error)));
