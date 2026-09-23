use arboard::Clipboard;
use base64::Engine;
use image::ImageEncoder;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Mutex},
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, State, WebviewUrl,
    WebviewWindowBuilder,
};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use uuid::Uuid;
use xcap::Monitor;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Formats { hex: bool, rgb: bool, hsl: bool, cmyk: bool }

impl Default for Formats {
    fn default() -> Self { Self { hex: true, rgb: true, hsl: false, cmyk: false } }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ColorRecord {
    id: String,
    timestamp: u64,
    r: u8,
    g: u8,
    b: u8,
    hex: String,
    rgb: String,
    hsl: String,
    cmyk: String,
    name: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    shortcut: String,
    theme: String,
    formats: Formats,
    #[serde(default)]
    copy_format: CopyFormat,
    auto_start: bool,
    history: Vec<ColorRecord>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shortcut: "CommandOrControl+Shift+C".into(),
            theme: "system".into(),
            formats: Formats::default(),
            copy_format: CopyFormat::default(),
            auto_start: false,
            history: vec![],
        }
    }
}

#[derive(Default)]
struct CaptureFrame { width: u32, height: u32, rgba: Vec<u8>, screen_bounds: (i32,i32,u32,u32), window_bounds: (i32,i32,u32,u32), data_url: String }

struct AppState {
    settings: Mutex<Settings>,
    captures: Mutex<HashMap<String, CaptureFrame>>,
    picker_generation: AtomicU64,
    toast_generation: AtomicU64,
    picking: AtomicBool,
    main_minimize_on_picker_ready: AtomicBool,
    capture_setup_done: AtomicBool,
    screen_permission_requested: AtomicBool,
    data_path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Preferences { theme: Option<String>, formats: Option<Formats>, copy_format: Option<CopyFormat>, auto_start: Option<bool> }

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CopyFormat { #[default] Hex, HexWithHash, Rgb, Hsl, Cmyk }

impl CopyFormat {
    fn label(self) -> &'static str { match self { Self::Hex | Self::HexWithHash => "HEX", Self::Rgb => "RGB", Self::Hsl => "HSL", Self::Cmyk => "CMYK" } }
    fn value<'a>(self, color: &'a SampleColor) -> &'a str {
        match self { Self::Hex => color.hex.trim_start_matches('#'), Self::HexWithHash => &color.hex, Self::Rgb => &color.rgb, Self::Hsl => &color.hsl, Self::Cmyk => &color.cmyk }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PickerResult { color: ColorRecord, copied_format: CopyFormat, copied_value: String }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SampleColor {
    r: u8, g: u8, b: u8, hex: String, rgb: String, hsl: String, cmyk: String, name: String,
}

fn hsl(r: u8, g: u8, b: u8) -> String {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = r.max(g).max(b); let min = r.min(g).min(b); let l = (max + min) / 2.0;
    let (mut h, s) = if (max - min).abs() < f64::EPSILON { (0.0, 0.0) } else {
        let d = max - min;
        let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        let h = if (max - r).abs() < f64::EPSILON { (g - b) / d + if g < b { 6.0 } else { 0.0 } }
            else if (max - g).abs() < f64::EPSILON { (b - r) / d + 2.0 }
            else { (r - g) / d + 4.0 };
        (h / 6.0, s)
    };
    h *= 360.0;
    format!("hsl({}, {}%, {}%)", h.round() as i32, (s * 100.0).round() as i32, (l * 100.0).round() as i32)
}

fn cmyk(r: u8, g: u8, b: u8) -> String {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let k = 1.0 - r.max(g).max(b);
    let (c, m, y) = if k >= 0.9999 { (0.0, 0.0, 0.0) } else {
        ((1.0 - r - k) / (1.0 - k), (1.0 - g - k) / (1.0 - k), (1.0 - b - k) / (1.0 - k))
    };
    format!("cmyk({}%, {}%, {}%, {}%)", (c*100.0).round() as i32, (m*100.0).round() as i32, (y*100.0).round() as i32, (k*100.0).round() as i32)
}

fn color_name(r: u8, g: u8, b: u8) -> &'static str {
    let max = r.max(g).max(b); let min = r.min(g).min(b); let delta = max - min;
    let light = (max as u16 + min as u16) / 2;
    if max < 25 { return "黑色"; }
    if delta < 8 { return if min > 242 { "白色" } else if light < 65 { "深灰色" } else if light < 155 { "灰色" } else if light < 220 { "浅灰色" } else { "近白色" }; }
    let saturation=delta as f32/max as f32;
    let (rf,gf,bf)=(r as f32/255.0,g as f32/255.0,b as f32/255.0);
    let d=(max-min) as f32/255.0; let mut hue = if max==r { 60.0*(((gf-bf)/d)%6.0) } else if max==g { 60.0*((bf-rf)/d+2.0) } else { 60.0*((rf-gf)/d+4.0) };
    if hue<0.0 { hue+=360.0; }
    if light>=200 && delta>=8 {
        if (315.0..=340.0).contains(&hue) { return "浅粉色"; }
        if hue>340.0 || hue<=10.0 { return "粉红色"; }
        if (35.0..=78.0).contains(&hue) { return "浅黄色"; }
        if (79.0..=180.0).contains(&hue) { return "浅绿色"; }
        if (181.0..=250.0).contains(&hue) { return if saturation<0.12 {"浅灰蓝色"} else {"浅蓝色"}; }
        if (251.0..315.0).contains(&hue) { return if saturation<0.12 {"浅灰紫色"} else {"淡紫色"}; }
        if (11.0..35.0).contains(&hue) { return "浅橙色"; }
    }
    if min>242 { return "白色"; }
    if delta<10 { return if light<65 {"深灰色"} else if light<155 {"灰色"} else if light<220 {"浅灰色"} else {"近白色"}; }
    if saturation < 0.35 {
        if (190.0..=260.0).contains(&hue) { return "灰蓝色"; }
        if (35.0..=75.0).contains(&hue) { return "灰黄色"; }
        if saturation < 0.14 { return if light<80 {"深灰色"} else if light>210 {"浅灰色"} else {"灰色"}; }
    }
    if (12.0..=48.0).contains(&hue) && light<145 && r<190 && r>g && g>=b {
        return if light<70 {"深棕色"} else {"棕色"};
    }
    if light < 70 { return match hue as i32 { 15..=55 => "深棕色", 56..=175 => "深绿色", 176..=260 => "深蓝色", 261..=335 => "深紫色", _ => "深红色" }; }
    match hue as i32 {
        0..=10 | 350..=359 => if light>200 {"浅红色"} else {"红色"},
        11..=24 => "橘红色", 25..=44 => if light>190 {"浅橙色"} else {"橙色"},
        45..=69 => if light>205 {"浅黄色"} else {"黄色"}, 70..=94 => "黄绿色",
        95..=154 => if light>195 {"浅绿色"} else {"绿色"}, 155..=184 => "青绿色",
        185..=204 => "青色", 205..=249 => if light>190 {"浅蓝色"} else {"蓝色"},
        250..=274 => "蓝紫色", 275..=314 => if light>195 {"浅紫色"} else {"紫色"},
        315..=339 => if light>200 {"浅粉色"} else {"粉色"}, _ => "玫红色"
    }
}

fn sample(r: u8, g: u8, b: u8) -> SampleColor {
    SampleColor { r, g, b, hex: format!("#{r:02X}{g:02X}{b:02X}"), rgb: format!("rgb({r}, {g}, {b})"), hsl: hsl(r,g,b), cmyk: cmyk(r,g,b), name: color_name(r,g,b).into() }
}

fn save(state: &AppState) -> Result<(), String> {
    let value = state.settings.lock().map_err(|_| "设置被占用")?;
    if let Some(parent) = state.data_path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    fs::write(&state.data_path, serde_json::to_vec_pretty(&*value).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn emit_state(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let value = state.settings.lock().map_err(|_| "设置被占用")?.clone();
    app.emit("state-changed", value).map_err(|e| e.to_string())
}

fn show_main(app: &AppHandle) -> Result<(), String> {
    close_pickers(app);
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Regular).map_err(|e| e.to_string())?;
    if let Some(window) = app.get_webview_window("main") {
        window.unminimize().map_err(|e| e.to_string())?;
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn hide_main(app: &AppHandle) -> Result<(), String> {
    // 主窗口的淡入淡出动画已在启动时关掉（见 setup），这里直接隐藏即可。
    if let Some(window) = app.get_webview_window("main") {
        window.hide().map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory).map_err(|e| e.to_string())?;
    Ok(())
}

fn close_pickers(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.picker_generation.fetch_add(1, Ordering::SeqCst);
    state.main_minimize_on_picker_ready.store(false, Ordering::SeqCst);
    state.capture_setup_done.store(false, Ordering::SeqCst);
    let labels: Vec<_> = app.webview_windows().keys().filter(|k| k.starts_with("picker-")).cloned().collect();
    for label in labels { if let Some(window) = app.get_webview_window(&label) { let _ = window.close(); } }
    if let Ok(mut frames) = state.captures.lock() { frames.clear(); }
    state.picking.store(false, Ordering::SeqCst);
}

#[cfg(target_os = "macos")]
const SCREEN_PERMISSION_ERROR: &str = "macOS 尚未授予取色鸭屏幕录制权限。授权后必须完全退出并重新打开应用，权限才会生效。";

#[cfg(target_os = "macos")]
fn ensure_screen_capture_permission(app: &AppHandle) -> Result<(), String> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    if unsafe { CGPreflightScreenCaptureAccess() } { return Ok(()); }
    let state = app.state::<AppState>();
    if !state.screen_permission_requested.swap(true, Ordering::SeqCst)
        && unsafe { CGRequestScreenCaptureAccess() } {
        return Ok(());
    }
    Err(SCREEN_PERMISSION_ERROR.into())
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct PermissionStatus { granted: bool, translocated: bool }

#[cfg(target_os = "macos")]
fn screen_capture_granted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[cfg(target_os = "macos")]
fn app_translocated() -> bool {
    std::env::current_exe().map(|p| p.to_string_lossy().contains("AppTranslocation")).unwrap_or(false)
}

/// macOS 右键弹完菜单后立即把菜单从托盘图标上摘掉。
/// tray-icon 0.24 会把菜单永久挂到 NSStatusItem 上，导致左键点击被吞；
/// 上游在 0.25.1 修成了"只在弹出瞬间挂载"，这里在应用层复刻。
/// `set_menu(None)` 的泛型参数需要显式指定，用 helper 从 tray 自身推断 Runtime。
#[cfg(target_os = "macos")]
fn detach_tray_menu<R: tauri::Runtime>(tray: &tauri::tray::TrayIcon<R>) {
    let _ = tray.set_menu(None::<tauri::menu::Menu<R>>);
}

#[tauri::command]
fn screen_permission_status() -> Result<PermissionStatus, String> {
    #[cfg(target_os = "macos")]
    return Ok(PermissionStatus { granted: screen_capture_granted(), translocated: app_translocated() });
    #[cfg(not(target_os = "macos"))]
    return Ok(PermissionStatus { granted: true, translocated: false });
}

#[tauri::command]
fn request_screen_permission(_app: AppHandle) -> Result<PermissionStatus, String> {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGRequestScreenCaptureAccess() -> bool;
        }
        let state = _app.state::<AppState>();
        // The system prompt fires at most once per process. It returns immediately
        // and does not wait for the user's decision, so the caller must guide the
        // user to System Settings and then relaunch the app.
        if !state.screen_permission_requested.swap(true, Ordering::SeqCst) {
            unsafe { CGRequestScreenCaptureAccess() };
        }
        return Ok(PermissionStatus { granted: screen_capture_granted(), translocated: app_translocated() });
    }
    #[cfg(not(target_os = "macos"))]
    return Ok(PermissionStatus { granted: true, translocated: false });
}

#[tauri::command]
fn open_screen_recording_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return open_external_url("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");
    #[cfg(not(target_os = "macos"))]
    return Err("屏幕录制权限设置仅 macOS 需要".into());
}

#[tauri::command]
fn restart_app(app: AppHandle) -> Result<(), String> {
    // Relaunch so a newly granted Screen Recording permission takes effect.
    // macOS does not apply the grant to the already-running process.
    tauri::process::restart(&app.env())
}

fn copied_toast_url(format: CopyFormat, value: &str) -> String {
    let encoded_value: String=value.bytes().map(|byte|format!("%{byte:02X}")).collect();
    format!("toast.html?format={}&value={encoded_value}", format.label())
}

fn show_copied_toast(app: &AppHandle, format: CopyFormat, value: &str, monitor_bounds: Option<(i32, i32, u32, u32)>) {
    let generation=app.state::<AppState>().toast_generation.fetch_add(1,Ordering::SeqCst)+1;
    // A reused webview can show its previous HEX text before an update event is handled.
    // Give each copy its own URL so the first rendered frame uses the copied format/value.
    for (label, old_window) in app.webview_windows() {
        if label.starts_with("copied-toast-") {
            let _=old_window.hide();
            let _=old_window.close();
        }
    }
    let label=format!("copied-toast-{generation}");
    let window=WebviewWindowBuilder::new(app, &label, WebviewUrl::App(copied_toast_url(format,value).into()))
        .title("已复制色值").decorations(false).transparent(true).shadow(false)
        .always_on_top(true).skip_taskbar(true).focused(false).visible(false)
        .inner_size(390.0, 52.0).center().build().ok();
    if let Some(window)=window {
        let bounds=monitor_bounds.or_else(||window.current_monitor().ok().flatten().map(|monitor|{
            let p=monitor.position();let s=monitor.size();(p.x,p.y,s.width,s.height)
        }));
        if let Some((x,y,width,height))=bounds {
            let toast_width=window.inner_size().map(|size|size.width).unwrap_or(390);
            let left=x+(width.saturating_sub(toast_width)/2) as i32;
            let top=y+(height as f64*0.68) as i32;
            let _=window.set_position(tauri::PhysicalPosition::new(left,top));
        }
        let _=window.show();
        let app=app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            std::thread::sleep(Duration::from_millis(1700));
            if app.state::<AppState>().toast_generation.load(Ordering::SeqCst)==generation {
                let _ = window.hide();
            }
        });
    }
}

#[tauri::command]
fn get_state(state: State<AppState>) -> Result<Settings, String> { state.settings.lock().map(|v| v.clone()).map_err(|_| "设置被占用".into()) }

#[tauri::command]
fn update_preferences(app: AppHandle, state: State<AppState>, preferences: Preferences) -> Result<(), String> {
    { let mut s=state.settings.lock().map_err(|_| "设置被占用")?; if let Some(v)=preferences.theme {s.theme=v;} if let Some(v)=preferences.formats{s.formats=v;} if let Some(v)=preferences.copy_format{s.copy_format=v;} if let Some(v)=preferences.auto_start{s.auto_start=v;} }
    save(&state)?; emit_state(&app,&state)
}

#[tauri::command]
fn save_shortcut(app: AppHandle, state: State<AppState>, shortcut: String) -> Result<(), String> {
    if shortcut.split('+').count() < 2 { return Err("快捷键必须包含修饰键".into()); }
    let old=state.settings.lock().map_err(|_| "设置被占用")?.shortcut.clone();
    app.global_shortcut().unregister(old.as_str()).map_err(|e| e.to_string())?;
    if let Err(error)=app.global_shortcut().register(shortcut.as_str()) { let _=app.global_shortcut().register(old.as_str()); return Err(format!("快捷键不可用：{error}")); }
    state.settings.lock().map_err(|_| "设置被占用")?.shortcut=shortcut; save(&state)?; emit_state(&app,&state)
}

fn next_picker_request(app: &AppHandle) -> u64 {
    let state=app.state::<AppState>();
    if state.picking.load(Ordering::SeqCst) { return state.picker_generation.load(Ordering::SeqCst); }
    state.picker_generation.fetch_add(1, Ordering::SeqCst) + 1
}

#[derive(Clone, Copy)]
enum PickerSource { Shortcut, MainButton, Tray }

impl PickerSource {
    fn changes_main_window(self) -> bool { matches!(self, Self::MainButton) }
}

fn encode_capture_image(rgba:&[u8],width:u32,height:u32)->Result<String,String>{
    let mut png=Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba,width,height,image::ExtendedColorType::Rgba8)
        .map_err(|e|format!("无法编码取色画面：{e}"))?;
    Ok(format!("data:image/png;base64,{}",base64::engine::general_purpose::STANDARD.encode(png)))
}

#[cfg(target_os = "windows")]
mod window_composition {
    use std::{ffi::c_void, mem::size_of};
    use tauri::WebviewWindow;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: *mut c_void, attribute: u32, value: *const c_void, size: u32) -> i32;
        fn DwmFlush() -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowDisplayAffinity(hwnd: *mut c_void, affinity: u32) -> i32;
    }

    pub fn disable_transitions(window: &WebviewWindow) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;
        let disabled: i32 = 1;
        // DWMWA_TRANSITIONS_FORCEDISABLED = 3 (dwmapi.h).
        let result = unsafe {
            DwmSetWindowAttribute(hwnd.0, 3, (&disabled as *const i32).cast(), size_of::<i32>() as u32)
        };
        if result < 0 { return Err(format!("无法关闭窗口动画：0x{result:08X}")); }
        Ok(())
    }

    pub fn wait_for_hidden_frame() {
        // Wait for the hide to pass through desktop composition before xcap reads it.
        // Two presents also cover a WebView frame submitted immediately before hide.
        unsafe { DwmFlush(); DwmFlush(); }
    }

    pub fn exclude_from_capture(window: &WebviewWindow, exclude: bool) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;
        // WDA_EXCLUDEFROMCAPTURE removes this window from the captured desktop
        // while it remains visible to the user (Windows 10 2004 and later).
        let affinity = if exclude { 0x11 } else { 0 };
        if unsafe { SetWindowDisplayAffinity(hwnd.0, affinity) } == 0 {
            return Err(format!("无法设置截图窗口排除状态：{}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

/// macOS: 直接调 AppKit 的小工具。
///
/// 两条硬规则（之前都违反了，这就是授予屏幕录制权限后点取色直接闪退的原因）：
/// 1. objc_msgSend 必须按被调方法的真实签名声明，绝不能写成 C 可变参数 `(...)`。
///    ARM64 上可变参数的调用约定与普通函数完全不同，参数会传错；
///    Apple 官方文档明确要求把 objc_msgSend cast 成被调方法的真实原型。
/// 2. AppKit 的调用必须在主线程执行。
///    - disable_show_hide_animation 在 setup（主线程）里调用一次即可，
///      NSWindow 的 animationBehavior 是持久属性，设一次永久生效；
///    - raise_above_dock 在取色流程（后台线程）里用 run_on_main_thread 派发。
#[cfg(target_os = "macos")]
mod mac_appkit {
    use std::ffi::{c_void, CString};
    use std::os::raw::c_char;
    use tauri::WebviewWindow;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        // 下面两个方法都是 `- (void)xxx:(NSInteger)`，照此声明。
        fn objc_msgSend(receiver: *mut c_void, sel: *mut c_void, arg: i64);
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGShieldingWindowLevel() -> i32;
    }

    fn send_integer(window: &WebviewWindow, selector: &str, value: i64, null_msg: &str) -> Result<(), String> {
        unsafe {
            let ns_window = window.ns_window().map_err(|e| e.to_string())?;
            if ns_window.is_null() {
                return Err(null_msg.into());
            }
            let sel_name = CString::new(selector).map_err(|e| e.to_string())?;
            let sel = sel_registerName(sel_name.as_ptr());
            objc_msgSend(ns_window, sel, value);
        }
        Ok(())
    }

    /// 把取色窗口抬到 Dock 之上。
    ///
    /// Tauri 的 `always_on_top` 在 macOS 只对应 NSFloatingWindowLevel，
    /// 仍然低于 Dock——取色层会被 Dock 图标盖住，鼠标靠近还会触发 Dock 放大。
    /// CGShieldingWindowLevel 是截图/取色类全屏覆盖层的标准层级，
    /// 盖住 Dock 与菜单栏后才能像 Windows 版一样全屏取色（含 Dock 区域）。
    /// 必须在主线程调用（调用方用 run_on_main_thread 派发）。
    pub fn raise_above_dock(window: &WebviewWindow) -> Result<(), String> {
        // CGWindowLevel 是 int32_t；setLevel: 要的是 NSInteger（64 位上为 i64）。
        let level = unsafe { CGShieldingWindowLevel() } as i64;
        send_integer(window, "setLevel:", level, "取色窗口系统句柄为空")
    }

    /// 关掉主窗口的显示/隐藏/最小化动画。
    ///
    /// NSWindow 默认在显示/隐藏/最小化时有动画；从主窗口点"开始取色"时，
    /// 最小化之后立刻截图会把动画中的窗口截进取色层，留下残影。
    /// 设为 NSWindowAnimationBehaviorNone 后三者都是瞬时的，
    /// 截图前只需等窗口服务合成一帧即可（约 20ms）。
    /// 快捷键/菜单栏图标触发本来就不动主窗口，不需要这段延迟。
    /// 在 setup 里调用一次即可；必须在主线程调用。
    pub fn disable_show_hide_animation(window: &WebviewWindow) -> Result<(), String> {
        // NSWindowAnimationBehaviorNone = 1（NSInteger，64 位上为 i64）。
        send_integer(window, "setAnimationBehavior:", 1, "主窗口系统句柄为空")
    }
}

/// macOS: 点按 Dock 图标时重新显示主窗口。
///
/// Tauri 默认不处理 applicationShouldHandleReopen，
/// Dock 图标点按后主窗口不会自己回来。这里只负责显示窗口，
/// 不打断正在进行的取色。
#[cfg(target_os = "macos")]
fn reopen_main_window(app: &AppHandle) {
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn open_picker(app: &AppHandle, request: u64, source: PickerSource) -> Result<(), String> {
    let state = app.state::<AppState>();
    if request != state.picker_generation.load(Ordering::SeqCst) { return Ok(()); }
    if state.picking.swap(true, Ordering::SeqCst) { return Ok(()); }
    state.capture_setup_done.store(false, Ordering::SeqCst);
    if let Ok(mut frames) = state.captures.lock() { frames.clear(); }
    #[cfg(target_os = "windows")]
    let mut main_capture_excluded = false;
    let result=(|| {
        #[cfg(target_os = "macos")]
        ensure_screen_capture_permission(app)?;
        if source.changes_main_window() {
            #[cfg(target_os = "windows")]
            if let Some(window) = app.get_webview_window("main") {
                {
                    let _ = window_composition::disable_transitions(&window);
                    if window_composition::exclude_from_capture(&window, true).is_ok() {
                        main_capture_excluded = true;
                        state.main_minimize_on_picker_ready.store(true, Ordering::SeqCst);
                        window_composition::wait_for_hidden_frame();
                    } else {
                        window.minimize().map_err(|e| e.to_string())?;
                        window_composition::wait_for_hidden_frame();
                    }
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                // macOS: 从主窗口点"开始取色"时，最小化主窗口到 Dock，
                // 而不是直接隐藏——隐藏会连 Dock 图标一起收起（Accessory），
                // 看起来像窗口被关掉了。
                // 最小化动画已在 setup 里随显示/隐藏动画一起关掉
                // （setAnimationBehavior: NSWindowAnimationBehaviorNone），
                // 最小化是瞬时的，这里只等窗口服务合成一帧再截图（约 20ms）。
                // 快捷键/菜单栏触发不走这里，无延迟。
                if let Some(window) = app.get_webview_window("main") {
                    window.minimize().map_err(|e| e.to_string())?;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let monitors=Monitor::all().map_err(|e| format!("无法读取屏幕：{e}"))?;
        if monitors.is_empty(){return Err("没有检测到显示器".to_string());}
        for (index,monitor) in monitors.into_iter().enumerate(){
            if request != state.picker_generation.load(Ordering::SeqCst) { return Ok(()); }
            let image=monitor.capture_image().map_err(|e| format!("无法截取屏幕，请授予屏幕录制权限：{e}"))?;
            if request != state.picker_generation.load(Ordering::SeqCst) { return Ok(()); }
            let label=format!("picker-{index}"); let width=image.width(); let height=image.height();
            let x=monitor.x().map_err(|e|e.to_string())? as f64; let y=monitor.y().map_err(|e|e.to_string())? as f64;
            let view_width=monitor.width().map_err(|e|e.to_string())? as f64;
            let view_height=monitor.height().map_err(|e|e.to_string())? as f64;
            #[cfg(target_os = "windows")]
            let physical_bounds=(x as i32,y as i32,view_width as u32,view_height as u32);
            #[cfg(target_os = "windows")]
            let (x,y,view_width,view_height) = {
                let scale = monitor.scale_factor().map_err(|e|e.to_string())? as f64;
                (x / scale, y / scale, view_width / scale, view_height / scale)
            };
            let rgba=image.into_raw();
            let data_url=encode_capture_image(&rgba,width,height)?;
            state.captures.lock().map_err(|_| "取色缓存被占用")?
                .insert(label.clone(),CaptureFrame{width,height,rgba,screen_bounds:(0,0,0,0),window_bounds:(0,0,0,0),data_url});
            if request != state.picker_generation.load(Ordering::SeqCst) {
                if let Ok(mut frames) = state.captures.lock() { frames.remove(&label); }
                return Ok(());
            }
            let window=WebviewWindowBuilder::new(app,&label,WebviewUrl::App("picker.html".into()))
                .title("取色鸭").decorations(false).transparent(true).shadow(false).always_on_top(true).skip_taskbar(true)
                .position(x,y).inner_size(view_width,view_height).accept_first_mouse(true).focused(false).visible(false).build().map_err(|e|e.to_string())?;
            // macOS: always_on_top 盖不住 Dock，抬到 CGShieldingWindowLevel
            // 才能对 Dock 区域取色，且鼠标过去不再触发 Dock 放大。
            // AppKit 调用必须在主线程执行，这里是后台线程，用 run_on_main_thread 派发。
            #[cfg(target_os = "macos")]
            {
                let picker_window = window.clone();
                app.run_on_main_thread(move || {
                    let _ = mac_appkit::raise_above_dock(&picker_window);
                })
                .map_err(|e| e.to_string())?;
            }
            if request != state.picker_generation.load(Ordering::SeqCst) {
                let _ = window.close();
                if let Ok(mut frames) = state.captures.lock() { frames.remove(&label); }
                return Ok(());
            }
            #[cfg(target_os = "windows")]
            {
                let _ = window_composition::disable_transitions(&window);
                let (x,y,width,height)=physical_bounds;
                window.set_position(tauri::PhysicalPosition::new(x,y)).map_err(|e|e.to_string())?;
                window.set_size(tauri::PhysicalSize::new(width,height)).map_err(|e|e.to_string())?;
            }
            let screen=window.current_monitor().map_err(|e|e.to_string())?.ok_or("无法确定取色窗口所在显示器")?;
            let position=screen.position();let size=screen.size();
            let window_position=window.inner_position().map_err(|e|e.to_string())?;
            let window_size=window.inner_size().map_err(|e|e.to_string())?;
            if let Some(frame)=state.captures.lock().map_err(|_|"取色缓存被占用")?.get_mut(&label) {
                frame.screen_bounds=(position.x,position.y,size.width,size.height);
                frame.window_bounds=(window_position.x,window_position.y,window_size.width,window_size.height);
            }
        }
        Ok(())
    })();
    #[cfg(target_os = "windows")]
    if main_capture_excluded {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window_composition::exclude_from_capture(&window, false);
        }
    }
    if result.is_ok() && request == state.picker_generation.load(Ordering::SeqCst) {
        state.capture_setup_done.store(true, Ordering::SeqCst);
    }
    if result.is_err(){
        close_pickers(app);
        #[cfg(target_os = "macos")]
        if let Err(ref message) = result {
            // Tray/shortcut launches have no visible window: bring the main window
            // back and let it show the permission guide dialog.
            if message.as_str() == SCREEN_PERMISSION_ERROR && !source.changes_main_window() {
                let _ = show_main(app);
                let _ = app.emit("permission-missing", ());
            }
        }
        if source.changes_main_window() {
            let _=show_main(app);
        }
    }
    result
}

#[tauri::command]
async fn start_picker(app: AppHandle) -> Result<(), String> {
    let request=next_picker_request(&app);
    tauri::async_runtime::spawn_blocking(move || open_picker(&app,request,PickerSource::MainButton)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn picker_pointer(window: tauri::WebviewWindow) -> Result<Option<(f64, f64)>, String> {
    // Use the full monitor, including the taskbar/menu bar/Dock region.
    // CSS coordinates only position the floating card; sampling uses native pixels.
    tauri::async_runtime::spawn_blocking(move || {
        let cursor = window.cursor_position().map_err(|e| e.to_string())?;
        let monitor = window.current_monitor().map_err(|e| e.to_string())?.ok_or("取色窗口不在显示器上")?;
        let origin = monitor.position();
        let size = monitor.size();
        let x = cursor.x - origin.x as f64;
        let y = cursor.y - origin.y as f64;
        if size.width == 0 || size.height == 0 || x < 0.0 || y < 0.0 || x >= size.width as f64 || y >= size.height as f64 {
            return Ok(None);
        }
        Ok(Some((x / size.width as f64, y / size.height as f64)))
    }).await.map_err(|e| e.to_string())?
}

fn pixel_for_cursor(bounds:(i32,i32,u32,u32),width:u32,height:u32,cursor:(f64,f64))->Option<(u32,u32)>{
    let (left,top,screen_width,screen_height)=bounds;
    if screen_width==0||screen_height==0||width==0||height==0
        ||cursor.0<left as f64||cursor.1<top as f64
        ||cursor.0>=left as f64+screen_width as f64||cursor.1>=top as f64+screen_height as f64{return None}
    let x=(((cursor.0-left as f64)/screen_width as f64)*width as f64).floor().min((width-1)as f64)as u32;
    let y=(((cursor.1-top as f64)/screen_height as f64)*height as f64).floor().min((height-1)as f64)as u32;
    Some((x,y))
}

#[tauri::command]
fn get_capture_image(state:State<AppState>,window:tauri::WebviewWindow)->Result<String,String>{
    state.captures.lock().map_err(|_|"取色缓存被占用")?
        .get(window.label()).map(|frame|frame.data_url.clone()).ok_or("取色画面不存在".into())
}

#[tauri::command]
fn show_ready_picker(state:State<AppState>,window:tauri::WebviewWindow)->Result<bool,String>{
    if !state.captures.lock().map_err(|_|"取色缓存被占用")?.contains_key(window.label()) {
        return Err("取色画面不存在".into());
    }
    if !state.capture_setup_done.load(Ordering::SeqCst) { return Ok(false); }
    window.show().map_err(|e|e.to_string())?;
    if state.main_minimize_on_picker_ready.swap(false, Ordering::SeqCst) {
        if let Some(main) = window.app_handle().get_webview_window("main") {
            main.minimize().map_err(|e|e.to_string())?;
        }
    }
    // Focus only the display under the pointer; another display must not steal
    // the first click while its hidden webview finishes loading.
    let cursor=window.cursor_position().map_err(|e|e.to_string())?;
    let origin=window.inner_position().map_err(|e|e.to_string())?;
    let size=window.inner_size().map_err(|e|e.to_string())?;
    if cursor.x>=origin.x as f64&&cursor.y>=origin.y as f64
        &&cursor.x<origin.x as f64+size.width as f64
        &&cursor.y<origin.y as f64+size.height as f64 {
        window.set_focus().map_err(|e|e.to_string())?;
    }
    Ok(true)
}

#[tauri::command]
fn sample_color(state: State<AppState>, window: tauri::WebviewWindow) -> Result<SampleColor,String>{
    let cursor=window.cursor_position().map_err(|e|e.to_string())?;
    let frames=state.captures.lock().map_err(|_|"取色缓存被占用")?;
    let (frame,(x,y))=frames.values().find_map(|frame|{
        pixel_for_cursor(frame.window_bounds,frame.width,frame.height,(cursor.x,cursor.y))
            .or_else(||pixel_for_cursor(frame.screen_bounds,frame.width,frame.height,(cursor.x,cursor.y)))
            .map(|pixel|(frame,pixel))
    }).ok_or("鼠标不在取色画面内")?;
    let i=((y*frame.width+x)*4)as usize; if i+2>=frame.rgba.len(){return Err("取色坐标超出范围".into());}
    Ok(sample(frame.rgba[i],frame.rgba[i+1],frame.rgba[i+2]))
}

#[tauri::command]
fn confirm_color(app:AppHandle,state:State<AppState>,window:tauri::WebviewWindow,r:u8,g:u8,b:u8)->Result<(),String>{
    let color=sample(r,g,b); let record=ColorRecord{id:Uuid::new_v4().to_string(),timestamp:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()as u64,r,g,b,hex:color.hex.clone(),rgb:color.rgb.clone(),hsl:color.hsl.clone(),cmyk:color.cmyk.clone(),name:color.name.clone()};
    let copy_format=state.settings.lock().map_err(|_|"设置被占用")?.copy_format;
    let copied_value=copy_format.value(&color).to_string();
    Clipboard::new().and_then(|mut c|c.set_text(&copied_value)).map_err(|e|format!("复制失败：{e}"))?;
    {let mut s=state.settings.lock().map_err(|_|"设置被占用")?;s.history.insert(0,record.clone());s.history.truncate(24);}
    save(&state)?;
    let monitor_bounds=window.current_monitor().ok().flatten().map(|monitor|{
        let p=monitor.position();let s=monitor.size();(p.x,p.y,s.width,s.height)
    });
    tauri::async_runtime::spawn_blocking(move || {
        close_pickers(&app);
        let _ = emit_state(&app, &app.state::<AppState>());
        show_copied_toast(&app, copy_format, &copied_value, monitor_bounds);
        let _ = app.emit("picker-result",PickerResult{color:record,copied_format:copy_format,copied_value});
    });
    Ok(())
}

#[tauri::command]
fn cancel_picker(app:AppHandle)->Result<(),String>{
    tauri::async_runtime::spawn_blocking(move || {
        close_pickers(&app);
        let _ = app.emit("picker-cancelled",());
    });
    Ok(())
}

#[tauri::command]
fn clear_history(app:AppHandle,state:State<AppState>)->Result<(),String>{state.settings.lock().map_err(|_|"设置被占用")?.history.clear();save(&state)?;emit_state(&app,&state)}

#[tauri::command]
fn delete_history(app:AppHandle,state:State<AppState>,id:String)->Result<(),String>{state.settings.lock().map_err(|_|"设置被占用")?.history.retain(|v|v.id!=id);save(&state)?;emit_state(&app,&state)}

#[tauri::command]
fn copy_value(value:String)->Result<(),String>{Clipboard::new().and_then(|mut c|c.set_text(value)).map_err(|e|e.to_string())}

fn open_external_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    result.map(|_| ()).map_err(|error| error.to_string())
}

#[tauri::command]
fn open_project_url() -> Result<(), String> {
    open_external_url("https://lynnwang2.github.io/DuckColorPicker/")
}

#[tauri::command]
fn open_xiaohongshu_url() -> Result<(), String> {
    open_external_url("https://xhslink.cn/o/HidvZgySfF")
}

pub fn run(){
    tauri::Builder::default()
        // Configure autostart here: this plugin accepts no JSON object configuration.
        // Adding plugins.autostart to tauri.conf.json aborts startup on both platforms.
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(|app,_shortcut,event|{if event.state()==ShortcutState::Pressed{let request=next_picker_request(app);let app=app.clone();tauri::async_runtime::spawn_blocking(move||{let _=open_picker(&app,request,PickerSource::Shortcut);});}}).build())
        .setup(|app|{
            let data_path=app.path().app_data_dir()?.join("settings.json"); let mut settings: Settings=fs::read(&data_path).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_default();
            for color in &mut settings.history { color.name=color_name(color.r,color.g,color.b).into(); }
            let shortcut=settings.shortcut.clone(); app.manage(AppState{settings:Mutex::new(settings),captures:Mutex::new(HashMap::new()),picker_generation:AtomicU64::new(0),toast_generation:AtomicU64::new(0),picking:AtomicBool::new(false),main_minimize_on_picker_ready:AtomicBool::new(false),capture_setup_done:AtomicBool::new(false),screen_permission_requested:AtomicBool::new(false),data_path});
            app.global_shortcut().register(shortcut.as_str())?;
            let show=MenuItem::with_id(app,"show","打开取色鸭",true,None::<&str>)?;let pick=MenuItem::with_id(app,"pick","开始取色",true,None::<&str>)?;let quit=MenuItem::with_id(app,"quit","退出",true,None::<&str>)?;let menu=Menu::with_items(app,&[&show,&pick,&quit])?;
            // Windows needs the colored, transparent rounded icon: the old tray.png
            // was an opaque white square and disappeared against the taskbar.
            // macOS uses the designed white duck silhouette (tray-mac.png) as a
            // template image: the alpha channel is the mask, so it adapts to
            // light/dark menu bars. It must keep real transparency — a fully
            // opaque PNG renders as a solid white box in the menu bar.
            #[cfg(target_os = "windows")]
            let tray_icon_bytes = include_bytes!("../icons/32x32.png").as_slice();
            #[cfg(not(target_os = "windows"))]
            let tray_icon_bytes = include_bytes!("../icons/tray-mac.png").as_slice();
            let tray_icon = image::load_from_memory(tray_icon_bytes)
                .map(|image| {
                    let rgba = image.into_rgba8();
                    let (width, height) = rgba.dimensions();
                    tauri::image::Image::new_owned(rgba.into_raw(), width, height)
                })
                .unwrap_or_else(|_| app.default_window_icon().unwrap().clone());
            let tray_builder=TrayIconBuilder::new().icon(tray_icon).icon_as_template(cfg!(target_os="macos")).tooltip("取色鸭 · Duck Color Picker").on_menu_event(|app,event|match event.id.as_ref(){"show"=>{let _=show_main(app);},"pick"=>{let request=next_picker_request(app);let app=app.clone();tauri::async_runtime::spawn_blocking(move||{let _=open_picker(&app,request,PickerSource::Tray);});},"quit"=>app.exit(0),_=>{}});
            // Windows 照旧在构建时挂载菜单。macOS 不行：tray-icon 0.24 会把菜单永久挂到
            // NSStatusItem 上，macOS 27 的 AppKit 遇到已挂载菜单会直接弹菜单，
            // 左键点击到不了 TrayIconEvent::Click（上游在 tray-icon 0.25.1 修了，
            // 但 tauri 2.11.6 锁定 tray-icon ^0.24，只能在应用层复刻这个修法）。
            #[cfg(not(target_os="macos"))]
            let tray_builder=tray_builder.menu(&menu);
            #[cfg(target_os="windows")]
            let tray=tray_builder.show_menu_on_left_click(false).on_tray_icon_event(|tray,event|if let TrayIconEvent::Click{button:MouseButton::Left,button_state:MouseButtonState::Up,..}=event{let app=tray.app_handle().clone();let request=next_picker_request(&app);tauri::async_runtime::spawn_blocking(move||{let _=open_picker(&app,request,PickerSource::Tray);});});
            // macOS：平时不挂载菜单，左键单击直达取色（对齐 Windows 托盘行为）；
            // 右键按下时才临时挂载并弹出菜单，关闭后立即摘掉，避免吞掉左键点击。
            #[cfg(target_os="macos")]
            let tray=tray_builder.show_menu_on_left_click(false).on_tray_icon_event(move|tray,event|if let TrayIconEvent::Click{button,button_state,..}=event{match(button,button_state){
                (MouseButton::Left,MouseButtonState::Up)=>{let app=tray.app_handle().clone();let request=next_picker_request(&app);tauri::async_runtime::spawn_blocking(move||{let _=open_picker(&app,request,PickerSource::Tray);});},
                (MouseButton::Right,MouseButtonState::Down)=>{let _=tray.set_menu(Some(menu.clone()));let _=tray.with_inner_tray_icon(|t|t.show_menu());detach_tray_menu(tray);},
                _=>{},
            }});
            #[cfg(not(any(target_os="windows",target_os="macos")))]
            let tray=tray_builder;
            tray.build(app)?;
            // macOS: 启动时一次性关掉主窗口的显示/隐藏动画（NSWindow 的持久属性）。
            // 之后 hide()/show() 都是瞬时的，从主窗口点"开始取色"截图不会抓到淡出残影。
            // AppKit 调用必须在主线程执行，setup 本来就在主线程。
            #[cfg(target_os = "macos")]
            if let Some(main) = app.get_webview_window("main") {
                let _ = mac_appkit::disable_show_hide_animation(&main);
            }
            if !std::env::args().any(|v|v=="--hidden"){show_main(app.handle()).map_err(std::io::Error::other)?;} else {hide_main(app.handle()).map_err(std::io::Error::other)?;}
            Ok(())
        })
        .on_window_event(|window,event|if window.label()=="main"{if let tauri::WindowEvent::CloseRequested{api,..}=event{api.prevent_close();let _=hide_main(window.app_handle());}})
        .invoke_handler(tauri::generate_handler![get_state,update_preferences,save_shortcut,start_picker,picker_pointer,get_capture_image,show_ready_picker,sample_color,confirm_color,cancel_picker,clear_history,delete_history,copy_value,open_project_url,open_xiaohongshu_url,screen_permission_status,request_screen_permission,open_screen_recording_settings,restart_app])
        .build(tauri::generate_context!())
        .expect("取色鸭启动失败")
        .run(|app_handle, event| {
            // macOS: 点按 Dock 图标必须重新显示主窗口（Tauri 默认不处理 Reopen）。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                reopen_main_window(app_handle);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_main_button_changes_the_main_window_for_picking() {
        assert!(PickerSource::MainButton.changes_main_window());
        assert!(!PickerSource::Tray.changes_main_window());
        assert!(!PickerSource::Shortcut.changes_main_window());
    }

    #[test]
    fn old_settings_keep_hex_as_default_copy_format() {
        let old = r##"{"shortcut":"CommandOrControl+Shift+C","theme":"system","formats":{"hex":true,"rgb":true,"hsl":false,"cmyk":false},"autoStart":false,"history":[]}"##;
        let settings: Settings = serde_json::from_str(old).unwrap();
        assert!(matches!(settings.copy_format, CopyFormat::Hex));
        let color = sample(255, 0, 0);
        assert_eq!(settings.copy_format.value(&color), "FF0000");
    }

    #[test]
    fn selected_copy_format_uses_its_own_color_value() {
        let color = sample(255, 0, 0);
        assert_eq!(CopyFormat::Rgb.value(&color), "rgb(255, 0, 0)");
        assert_eq!(CopyFormat::Hsl.value(&color), "hsl(0, 100%, 50%)");
        assert_eq!(CopyFormat::Cmyk.value(&color), "cmyk(0%, 100%, 100%, 0%)");
        assert_eq!(CopyFormat::HexWithHash.value(&color), "#FF0000");
        assert!(matches!(serde_json::from_str::<CopyFormat>("\"hexwithhash\"").unwrap(), CopyFormat::HexWithHash));
        assert!(serde_json::from_str::<CopyFormat>("\"unknown\"").is_err());
    }

    #[test]
    fn copied_toast_url_uses_the_selected_format_and_copied_value() {
        let color = sample(222, 222, 222);
        for format in [CopyFormat::Hex, CopyFormat::HexWithHash, CopyFormat::Rgb, CopyFormat::Hsl, CopyFormat::Cmyk] {
            let value = format.value(&color);
            let url = copied_toast_url(format, value);
            assert!(url.starts_with(&format!("toast.html?format={}&value=", format.label())));
            let encoded_value: String = value.bytes().map(|byte| format!("%{byte:02X}")).collect();
            assert!(url.ends_with(&encoded_value));
        }
    }

    #[test]
    fn muted_and_brown_colors_have_specific_chinese_names() {
        assert_eq!(color_name(128, 80, 40), "棕色");
        assert_eq!(color_name(145, 160, 170), "灰蓝色");
        assert_eq!(color_name(180, 170, 130), "灰黄色");
        assert_eq!(color_name(128, 128, 128), "灰色");
        assert_eq!(color_name(255, 0, 0), "红色");
    }

    #[test]
    fn pale_colors_keep_their_hue_instead_of_becoming_gray() {
        assert_eq!(color_name(0xfa,0xe0,0xec),"浅粉色");
        assert_eq!(color_name(0xff,0xb6,0xc1),"粉红色");
        assert_eq!(color_name(0xff,0xfa,0xcd),"浅黄色");
        assert_eq!(color_name(0xdf,0xf6,0xdd),"浅绿色");
        assert_eq!(color_name(0xed,0xe7,0xf2),"浅灰紫色");
        assert_eq!(color_name(0xfd,0xf4,0xff),"浅灰紫色");
        assert_eq!(color_name(0xf1,0xf1,0xf1),"近白色");
    }

    #[test]
    fn mac_tray_icon_keeps_transparency_for_template_mode() {
        // The macOS menu bar icon is used as a template image: macOS takes the
        // alpha channel as the mask. A fully opaque PNG shows up as a solid
        // white box in the menu bar (this exact bug shipped once).
        let bytes = include_bytes!("../icons/tray-mac.png");
        let icon = image::load_from_memory(bytes).expect("tray-mac.png must decode").into_rgba8();
        let (width, height) = icon.dimensions();
        assert_eq!(width, height, "menu bar icon must be square");
        assert!(width >= 36, "menu bar icon should be at least 36px for retina");
        let mut transparent = false;
        let mut opaque = false;
        for pixel in icon.pixels() {
            if pixel[3] == 0 { transparent = true; }
            if pixel[3] == 255 { opaque = true; }
        }
        assert!(transparent, "tray-mac.png needs transparent pixels for the template mask");
        assert!(opaque, "tray-mac.png needs opaque pixels for the duck shape");
    }

    #[test]
    fn native_cursor_maps_full_monitor_including_system_bars() {
        let bounds=(-1920,0,1920,1080);
        assert_eq!(pixel_for_cursor(bounds,3840,2160,(-1920.0,0.0)),Some((0,0)));
        assert_eq!(pixel_for_cursor(bounds,3840,2160,(-1.0,1079.0)),Some((3838,2158)));
        assert_eq!(pixel_for_cursor(bounds,3840,2160,(0.0,500.0)),None);
        assert_eq!(pixel_for_cursor((0,0,100,100),200,200,(25.0,25.0)),Some((50,50)));
    }

    #[test]
    fn displayed_capture_uses_the_same_pixels_as_sampling() {
        let rgba=[0xcc,0xe8,0xff,0xff,0xe0,0xe0,0xe0,0xff];
        let url=encode_capture_image(&rgba,2,1).unwrap();
        let png=base64::engine::general_purpose::STANDARD.decode(url.strip_prefix("data:image/png;base64,").unwrap()).unwrap();
        assert_eq!(image::load_from_memory(&png).unwrap().into_rgba8().into_raw(),rgba);
    }
}
