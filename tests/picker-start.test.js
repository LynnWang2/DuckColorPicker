import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

async function pickerHarness(points) {
  const calls=[], timers=new Map(), listeners={}, classes=new Set();
  let id=0;
  const elements=Object.fromEntries(['card','swatch','name','values'].map(key=>[key,{
    style:{},textContent:'',innerHTML:'',classList:{add:v=>classes.add(v),remove:v=>classes.delete(v)},
    getBoundingClientRect:()=>({width:170,height:80})
  }]));
  const context={
    document:{querySelector:s=>elements[s.slice(1)],body:{focus(){}},documentElement:{dataset:{}},
      createElement:()=>({set textContent(v){this.innerHTML=v}}),addEventListener:(event,fn)=>listeners[event]=fn},
    window:{addEventListener:(event,fn)=>listeners[event]=fn},innerWidth:1000,innerHeight:800,
    getCurrentWebviewWindow:()=>({label:'picker-0'}),performance:{now:()=>100},
    setTimeout:(fn)=>{timers.set(++id,fn);return id},clearTimeout:id=>timers.delete(id),
    requestAnimationFrame:fn=>{queueMicrotask(fn);return ++id},cancelAnimationFrame(){},
    invoke:async(command,args)=>{
      calls.push({command,args});
      if(command==='get_state')return {formats:{hex:true},theme:'system'};
      if(command==='picker_pointer'){const point=points.length>1?points.shift():points[0];if(point instanceof Error)throw point;return point}
      if(command==='sample_color')return {hex:'#123456',name:'蓝色'};
    }
  };
  vm.runInNewContext(fs.readFileSync('src/picker.js','utf8').replace(/^import .*;\r?\n/gm,''),context);
  const settle=()=>new Promise(resolve=>setImmediate(resolve));
  await settle();
  return {calls,classes,elements,listeners,timers,async tick(){const pending=[...timers.values()];timers.clear();for(const fn of pending)await fn();await settle()}};
}

test('启动后无需鼠标事件就显示当前位置的色值',async()=>{
  const h=await pickerHarness([[0.25,0.5]]);
  assert.deepEqual(h.calls.filter(c=>c.command==='sample_color').map(c=>[c.args.localX,c.args.localY]),[[250,400]]);
  assert.equal(h.elements.swatch.style.background,'#123456');
  assert.ok(h.classes.has('ready'));
});
test('窗口未收到鼠标事件时仍跟随原生鼠标位置',async()=>{
  const h=await pickerHarness([[0.25,0.5],[0.75,0.25]]);await h.tick();
  assert.deepEqual(h.calls.filter(c=>c.command==='sample_color').map(c=>[c.args.localX,c.args.localY]),[[250,400],[750,200]]);
});
test('鼠标在另一屏时隐藏本屏卡片，进入后立即显示',async()=>{
  const h=await pickerHarness([null,[0.1,0.2]]);assert.equal(h.classes.has('ready'),false);
  await h.tick();assert.ok(h.classes.has('ready'));
});
test('首次查询暂时失败会重试，窗口关闭后停止查询',async()=>{
  const h=await pickerHarness([new Error('not ready'),[0.1,0.2]]);
  await h.tick();assert.ok(h.classes.has('ready'));
  h.listeners.pagehide();const count=h.calls.length;await h.tick();assert.equal(h.calls.length,count);
});
