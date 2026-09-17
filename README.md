# 取色鸭

取色鸭（Duck Color Picker）是一款面向 Windows 和 macOS 的轻量中文屏幕取色器。2.0 版本由 Electron 重构为 Tauri 2 + Rust，并保留原版功能。

## 功能

- 自定义全局快捷键，默认 `Ctrl/Command + Shift + C`
- 无弹出截图预览：进入透明取色层，鼠标移动时即时读取缓存像素
- 跟随鼠标的颜色卡片，单击确认，`Esc` 取消
- 显示普通中文颜色名，不使用传统色名
- HEX、RGB、HSL、CMYK 可自由勾选
- HEX 显示包含 `#`，复制自动移除 `#`
- 最近 24 次取色历史
- 跟随系统、浅色、深色三种外观
- 系统托盘和随系统启动
- 多显示器与缩放坐标换算

## 本地开发

前置条件：Node.js 20+、Rust stable，以及 Tauri 2 对应的平台依赖。

```bash
npm install
npm run tauri dev
```

构建当前平台安装包：

```bash
npm run tauri build
```

仅验证前端：

```bash
npm test
npm run build
```

## macOS 权限与发布

首次取色时 macOS 会要求“屏幕与系统音频录制”权限。若拒绝，请到“系统设置 → 隐私与安全性 → 屏幕与系统音频录制”中允许取色鸭，然后重启软件。

要避免“App 已损坏”提示，公开分发的 `.dmg` 应使用 Apple Developer ID 签名并经过 Apple 公证。GitHub Actions 已预留 Tauri 官方签名变量；没有证书时生成的是未签名测试包。

## Windows 发布

未签名安装包可能触发 SmartScreen。正式分发建议配置代码签名证书。托盘图标使用打包进可执行文件的多尺寸 `.ico`，不依赖开发目录，解决旧版安装后托盘图标丢失的问题。

## 架构说明

- `src/`：主界面和高帧率取色浮层
- `src-tauri/src/lib.rs`：屏幕捕获、像素采样、快捷键、托盘、剪贴板、历史记录
- `src-tauri/tauri.conf.json`：Windows/macOS 安装包配置
- `.github/workflows/build.yml`：双平台自动构建

