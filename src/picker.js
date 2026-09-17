import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

const card=document.querySelector('#card'),swatch=document.querySelector('#swatch'),name=document.querySelector('#name'),values=document.querySelector('#values');
const LABELS={hex:'HEX',rgb:'RGB',hsl:'HSL',cmyk:'CMYK'};
const windowLabel=getCurrentWebviewWindow().label;
let formats={hex:true,rgb:true,hsl:false,cmyk:false},currentColor=null,pendingPoint=null,inFlight=false,timer=null,lastRequest=0,confirming=false,cardWidth=170,cardHeight=80,latestPosition=null,positionFrame=0;
function escapeHtml(value){const span=document.createElement('span');span.textContent=String(value);return span.innerHTML}
function positionCard(x,y){latestPosition={x,y};if(positionFrame)return;positionFrame=requestAnimationFrame(()=>{const gap=18,left=latestPosition.x+gap+cardWidth>innerWidth?latestPosition.x-cardWidth-gap:latestPosition.x+gap,top=latestPosition.y+gap+cardHeight>innerHeight?latestPosition.y-cardHeight-gap:latestPosition.y+gap;card.style.transform=`translate3d(${Math.max(8,left)}px,${Math.max(8,top)}px,0)`;card.classList.add('ready');positionFrame=0})}
function renderColor(color){if(currentColor?.hex===color.hex){currentColor=color;return}currentColor=color;swatch.style.background=color.hex;name.textContent=color.name;values.innerHTML=Object.keys(LABELS).filter(key=>formats[key]).map(key=>`<div class="value"><b>${LABELS[key]}</b><span>${escapeHtml(color[key])}</span></div>`).join('');requestAnimationFrame(()=>{const rect=card.getBoundingClientRect();cardWidth=rect.width;cardHeight=rect.height})}
async function runSample(){if(inFlight||!pendingPoint)return;const point=pendingPoint;pendingPoint=null;inFlight=true;lastRequest=performance.now();try{renderColor(await invoke('sample_color',{windowLabel,localX:point.x,localY:point.y,viewWidth:innerWidth,viewHeight:innerHeight}))}catch(error){name.textContent=String(error)}inFlight=false;if(pendingPoint)scheduleSample()}
function scheduleSample(){if(inFlight)return;clearTimeout(timer);timer=setTimeout(runSample,Math.max(0,16-(performance.now()-lastRequest)))}
function queuePoint(event){positionCard(event.clientX,event.clientY);pendingPoint={x:event.clientX,y:event.clientY};scheduleSample()}
document.addEventListener('mousemove',queuePoint,{passive:true});
document.addEventListener('mousedown',async event=>{if(event.button!==0||confirming)return;event.preventDefault();confirming=true;try{const color=await invoke('sample_color',{windowLabel,localX:event.clientX,localY:event.clientY,viewWidth:innerWidth,viewHeight:innerHeight});await invoke('confirm_color',{r:color.r,g:color.g,b:color.b})}catch(error){confirming=false;name.textContent=String(error)}});
document.addEventListener('contextmenu',event=>event.preventDefault());
document.addEventListener('keydown',event=>{if(event.key==='Escape')invoke('cancel_picker')});
async function init(){document.body.focus();const state=await invoke('get_state');formats=state.formats||formats;if(state.theme!=='system')document.documentElement.dataset.theme=state.theme}
init().catch(error=>{name.textContent=String(error)});
