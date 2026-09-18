import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

test('主界面保留全部设置',()=>{const html=fs.readFileSync('index.html','utf8');for(const id of ['shortcut','theme','copyFormat','autoStart','history'])assert.match(html,new RegExp(`id="${id}"`))});
test('取色卡显示四种可选格式',()=>{const js=fs.readFileSync('src/picker.js','utf8');for(const name of ['HEX','RGB','HSL','CMYK'])assert.match(js,new RegExp(name))});
test('HEX显示保留#且复制在Rust侧处理',()=>{const js=fs.readFileSync('src/picker.js','utf8');assert.doesNotMatch(js,/replace\(['"]#/) });
