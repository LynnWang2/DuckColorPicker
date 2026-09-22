import { listen } from '@tauri-apps/api/event';

const message=document.querySelector('#message');
function render({format,value}){
  const label=/^(HEX|RGB|HSL|CMYK)$/.test(format||'')?format:'色值';
  if(label==='HEX'){
    message.textContent=value?`已复制 HEX #${String(value).replace(/^#/,'')}`:'已复制 HEX';
    return;
  }
  const displayValue=String(value||'').replace(/^(rgb|hsl|cmyk)(?=\()/i,'');
  message.textContent=displayValue?`已复制 ${label} ${displayValue}`:`已复制 ${label}`;
}
const query=new URLSearchParams(location.search);
render({format:query.get('format'),value:query.get('value')});
listen('copied-toast-update',event=>render(event.payload));
