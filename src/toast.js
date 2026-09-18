import { listen } from '@tauri-apps/api/event';

const message=document.querySelector('#message');
function render(format){
  message.textContent=/^(HEX|RGB|HSL|CMYK)$/.test(format||'')?`已复制 ${format} 色值`:'已复制色值';
}
render(new URLSearchParams(location.search).get('format'));
listen('copied-toast-update',event=>render(event.payload));
