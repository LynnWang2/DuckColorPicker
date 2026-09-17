import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

test('自启插件不接收 JSON 对象配置（Windows 和 macOS 启动回归）', () => {
  const config = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8'));
  // tauri-plugin-autostart v2 uses Rust's unit type for plugin configuration.
  // Launcher and arguments belong in the Rust initializer; an object aborts startup.
  assert.equal(config.plugins?.autostart ?? null, null,
    '移除 plugins.autostart 对象；自启方式已在 Rust 初始化中配置');
});
