import { invoke } from '@tauri-apps/api/core';

const card=document.querySelector('#card'),swatch=document.querySelector('#swatch'),name=document.querySelector('#name'),values=document.querySelector('#values'),screenCapture=document.querySelector('#screenCapture');
const LABELS={hex:'HEX',rgb:'RGB',hsl:'HSL',cmyk:'CMYK'};
let formats={hex:true,rgb:true,hsl:false,cmyk:false},currentColor=null,pendingPoint=null,inFlight=false,timer=null,lastRequest=0,confirming=false,cardWidth=170,cardHeight=80,latestPosition=null,positionFrame=0,pointerTimer=null,stopped=false,motionVersion=0,screenReady=false;
function escapeHtml(value){const span=document.createElement('span');span.textContent=String(value);return span.innerHTML}
function displayValue(key,value){return key==='hex'?value:String(value).replace(/^[a-z]+(?=\()/i,'')}
function positionCard(x,y){latestPosition={x,y};if(positionFrame)return;positionFrame=requestAnimationFrame(()=>{const gap=18,left=latestPosition.x+gap+cardWidth>innerWidth?latestPosition.x-cardWidth-gap:latestPosition.x+gap,top=latestPosition.y+gap+cardHeight>innerHeight?latestPosition.y-cardHeight-gap:latestPosition.y+gap;card.style.transform=`translate3d(${Math.max(8,Math.min(innerWidth-cardWidth-8,left))}px,${Math.max(8,Math.min(innerHeight-cardHeight-8,top))}px,0)`;card.classList.add('ready');positionFrame=0})}
function renderColor(color){if(currentColor?.hex===color.hex){currentColor=color;return}currentColor=color;swatch.style.background=color.hex;name.textContent=color.name;values.innerHTML=Object.keys(LABELS).filter(key=>formats[key]).map(key=>`<div class="value"><b>${LABELS[key]}</b><span>${escapeHtml(displayValue(key,color[key]))}</span></div>`).join('');requestAnimationFrame(()=>{const rect=card.getBoundingClientRect();cardWidth=rect.width;cardHeight=rect.height})}
async function runSample(){if(inFlight||!pendingPoint)return;const point=pendingPoint;pendingPoint=null;inFlight=true;lastRequest=performance.now();try{renderColor(await invoke('sample_color'));positionCard(point.x,point.y)}catch(error){name.textContent=String(error)}inFlight=false;if(pendingPoint)scheduleSample()}
function scheduleSample(){if(inFlight)return;clearTimeout(timer);timer=setTimeout(runSample,Math.max(0,16-(performance.now()-lastRequest)))}
function queuePoint(event){if(!screenReady)return;motionVersion++;pendingPoint={x:event.clientX,y:event.clientY};scheduleSample()}
async function pollPointer(){
  if(stopped)return;
  const version=motionVersion;
  try{
    const point=await invoke('picker_pointer');
    if(stopped||confirming||version!==motionVersion)return;
    if(!point){card.classList.remove('ready');latestPosition=null;return}
    const x=point[0]*innerWidth,y=point[1]*innerHeight;
    if(!latestPosition||Math.abs(latestPosition.x-x)>0.1||Math.abs(latestPosition.y-y)>0.1){
      pendingPoint={x,y};await runSample();
    }
  }catch(error){if(!currentColor)name.textContent=String(error)}
  finally{if(!stopped)pointerTimer=setTimeout(pollPointer,32)}
}
window.addEventListener('pagehide',()=>{stopped=true;clearTimeout(pointerTimer);clearTimeout(timer);cancelAnimationFrame(positionFrame)});
document.addEventListener('mousemove',queuePoint,{passive:true});
async function confirmAtPointer(){if(confirming||!screenReady)return;confirming=true;try{const color=await invoke('sample_color');await invoke('confirm_color',{r:color.r,g:color.g,b:color.b})}catch(error){confirming=false;name.textContent=String(error)}}
document.addEventListener('mousedown',event=>{if(event.button!==0)return;event.preventDefault();confirmAtPointer()});
document.addEventListener('contextmenu',event=>event.preventDefault());
document.addEventListener('keydown',event=>{if(event.key==='Escape')invoke('cancel_picker');else if(event.key===' '||event.key==='Enter'){event.preventDefault();confirmAtPointer()}});
async function showWhenReady(){for(let attempt=0;attempt<150;attempt++){if(await invoke('show_ready_picker'))return;await new Promise(resolve=>setTimeout(resolve,20))}throw new Error('取色画面准备超时')}
async function init(){document.body.focus();const [dataUrl,state]=await Promise.all([invoke('get_capture_image'),invoke('get_state')]);formats=state.formats||formats;if(state.theme!=='system')document.documentElement.dataset.theme=state.theme;await new Promise((resolve,reject)=>{screenCapture.onload=resolve;screenCapture.onerror=()=>reject(new Error('取色截图无法显示'));screenCapture.src=dataUrl});if(screenCapture.decode)await screenCapture.decode();await showWhenReady();screenReady=true;await pollPointer()}
init().catch(async error=>{stopped=true;name.textContent=String(error);card.classList.add('ready');try{await invoke('show_ready_picker')}catch{}});
