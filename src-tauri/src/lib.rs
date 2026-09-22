use arboard::Clipboard;
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
struct CaptureFrame { width: u32, height: u32, rgba: Vec<u8> }

struct AppState {
    settings: Mutex<Settings>,
    captures: Mutex<HashMap<String, CaptureFrame>>,
    picker_generation: AtomicU64,
    toast_generation: AtomicU64,
    picking: AtomicBool,
    screen_permission_requested: AtomicBool,
    data_path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Preferences { theme: Option<String>, formats: Option<Formats>, copy_format: Option<CopyFormat>, auto_start: Option<bool> }

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CopyFormat { #[default] Hex, Rgb, Hsl, Cmyk }

impl CopyFormat {
    fn label(self) -> &'static str { match self { Self::Hex => "HEX", Self::Rgb => "RGB", Self::Hsl => "HSL", Self::Cmyk => "CMYK" } }
    fn value<'a>(self, color: &'a SampleColor) -> &'a str {
        match self { Self::Hex => color.hex.trim_start_matches('#'), Self::Rgb => &color.rgb, Self::Hsl => &color.hsl, Self::Cmyk => &color.cmyk }
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
    if min > 242 { return "白色"; }
    if delta < 10 { return if light < 65 { "深灰色" } else if light < 155 { "灰色" } else if light < 220 { "浅灰色" } else { "近白色" }; }
    let (rf,gf,bf)=(r as f32/255.0,g as f32/255.0,b as f32/255.0);
    let d=(max-min) as f32/255.0; let mut hue = if max==r { 60.0*(((gf-bf)/d)%6.0) } else if max==g { 60.0*((bf-rf)/d+2.0) } else { 60.0*((rf-gf)/d+4.0) };
    if hue<0.0 { hue+=360.0; }
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
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn hide_main(app: &AppHandle) -> Result<(), String> {
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
    let labels: Vec<_> = app.webview_windows().keys().filter(|k| k.starts_with("picker-")).cloned().collect();
    for label in labels { if let Some(window) = app.get_webview_window(&label) { let _ = window.close(); } }
    if let Ok(mut frames) = state.captures.lock() { frames.clear(); }
    state.picking.store(false, Ordering::SeqCst);
}

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
    Err("macOS 尚未向当前安装的取色鸭授予录屏权限。请确认安装的是正式签名版本，并在系统设置 → 隐私与安全性 → 屏幕与系统音频录制中允许后重新打开应用。临时签名的测试包更新后可能需要重新授权。".into())
}

fn show_copied_toast(app: &AppHandle, format: CopyFormat) {
    let generation=app.state::<AppState>().toast_generation.fetch_add(1,Ordering::SeqCst)+1;
    let window=if let Some(existing)=app.get_webview_window("copied-toast") {
        let _=app.emit_to("copied-toast","copied-toast-update",format.label());
        let _=existing.show();
        Some(existing)
    } else {
        let url = format!("toast.html?format={}", format.label());
        WebviewWindowBuilder::new(app, "copied-toast", WebviewUrl::App(url.into()))
            .title("已复制色值").decorations(false).transparent(true).shadow(false)
            .always_on_top(true).skip_taskbar(true).focused(false)
            .inner_size(390.0, 52.0).center().build().ok()
    };
    if let Some(window)=window {
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

fn open_picker(app: &AppHandle, request: u64, source: PickerSource) -> Result<(), String> {
    let state = app.state::<AppState>();
    if request != state.picker_generation.load(Ordering::SeqCst) { return Ok(()); }
    if state.picking.swap(true, Ordering::SeqCst) { return Ok(()); }
    if let Ok(mut frames) = state.captures.lock() { frames.clear(); }
    let result=(|| {
        #[cfg(target_os = "macos")]
        ensure_screen_capture_permission(app)?;
        if matches!(source, PickerSource::MainButton) {
            if let Some(window) = app.get_webview_window("main") {
                window.minimize().map_err(|e| e.to_string())?;
                // Let the minimization animation finish before capturing the desktop.
                std::thread::sleep(Duration::from_millis(180));
            }
        }
        let monitors=Monitor::all().map_err(|e| format!("无法读取屏幕：{e}"))?;
        if monitors.is_empty(){return Err("没有检测到显示器".into());}
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
            state.captures.lock().map_err(|_| "取色缓存被占用")?
                .insert(label.clone(),CaptureFrame{width,height,rgba:image.into_raw()});
            if request != state.picker_generation.load(Ordering::SeqCst) {
                if let Ok(mut frames) = state.captures.lock() { frames.remove(&label); }
                return Ok(());
            }
            let window=WebviewWindowBuilder::new(app,&label,WebviewUrl::App("picker.html".into()))
                .title("取色鸭").decorations(false).transparent(true).shadow(false).always_on_top(true).skip_taskbar(true)
                .position(x,y).inner_size(view_width,view_height).accept_first_mouse(true).focused(false).build().map_err(|e|e.to_string())?;
            if request != state.picker_generation.load(Ordering::SeqCst) {
                let _ = window.close();
                if let Ok(mut frames) = state.captures.lock() { frames.remove(&label); }
                return Ok(());
            }
            #[cfg(target_os = "windows")]
            {
                let (x,y,width,height)=physical_bounds;
                window.set_position(tauri::PhysicalPosition::new(x,y)).map_err(|e|e.to_string())?;
                window.set_size(tauri::PhysicalSize::new(width,height)).map_err(|e|e.to_string())?;
            }
            // Only the screen under the cursor should take focus. Otherwise the
            // last monitor steals the first confirmation click on another screen.
            let cursor = window.cursor_position().map_err(|e| e.to_string())?;
            let origin = window.inner_position().map_err(|e| e.to_string())?;
            let size = window.inner_size().map_err(|e| e.to_string())?;
            if cursor.x >= origin.x as f64 && cursor.y >= origin.y as f64
                && cursor.x < origin.x as f64 + size.width as f64
                && cursor.y < origin.y as f64 + size.height as f64 {
                window.set_focus().map_err(|e|e.to_string())?;
            }
        }
        Ok(())
    })();
    if result.is_err(){
        close_pickers(app);
        if matches!(source, PickerSource::MainButton) {
            if let Some(window) = app.get_webview_window("main") { let _ = window.unminimize(); }
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
    // Native coordinates are physical pixels on both platforms. Normalize against
    // the actual window bounds so Retina and Windows display scaling agree with CSS.
    tauri::async_runtime::spawn_blocking(move || {
        let cursor = window.cursor_position().map_err(|e| e.to_string())?;
        let origin = window.inner_position().map_err(|e| e.to_string())?;
        let size = window.inner_size().map_err(|e| e.to_string())?;
        let x = cursor.x - origin.x as f64;
        let y = cursor.y - origin.y as f64;
        if size.width == 0 || size.height == 0 || x < 0.0 || y < 0.0 || x >= size.width as f64 || y >= size.height as f64 {
            return Ok(None);
        }
        Ok(Some((x / size.width as f64, y / size.height as f64)))
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn sample_color(state: State<AppState>, window_label: String, local_x: f64, local_y: f64, view_width: f64, view_height: f64) -> Result<SampleColor,String>{
    let frames=state.captures.lock().map_err(|_|"取色缓存被占用")?; let frame=frames.get(&window_label).ok_or("取色画面不存在")?;
    let x=((local_x/view_width.max(1.0))*frame.width as f64).floor().clamp(0.0,(frame.width-1)as f64)as u32;
    let y=((local_y/view_height.max(1.0))*frame.height as f64).floor().clamp(0.0,(frame.height-1)as f64)as u32;
    let i=((y*frame.width+x)*4)as usize; if i+2>=frame.rgba.len(){return Err("取色坐标超出范围".into());}
    Ok(sample(frame.rgba[i],frame.rgba[i+1],frame.rgba[i+2]))
}

#[tauri::command]
fn confirm_color(app:AppHandle,state:State<AppState>,r:u8,g:u8,b:u8)->Result<(),String>{
    let color=sample(r,g,b); let record=ColorRecord{id:Uuid::new_v4().to_string(),timestamp:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()as u64,r,g,b,hex:color.hex.clone(),rgb:color.rgb.clone(),hsl:color.hsl.clone(),cmyk:color.cmyk.clone(),name:color.name.clone()};
    let copy_format=state.settings.lock().map_err(|_|"设置被占用")?.copy_format;
    let copied_value=copy_format.value(&color).to_string();
    Clipboard::new().and_then(|mut c|c.set_text(&copied_value)).map_err(|e|format!("复制失败：{e}"))?;
    {let mut s=state.settings.lock().map_err(|_|"设置被占用")?;s.history.insert(0,record.clone());s.history.truncate(24);}
    save(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        close_pickers(&app);
        let _ = emit_state(&app, &app.state::<AppState>());
        show_copied_toast(&app, copy_format);
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
fn copy_value(value:String)->Result<(),String>{Clipboard::new().and_then(|mut c|c.set_text(value.trim_start_matches('#'))).map_err(|e|e.to_string())}

pub fn run(){
    tauri::Builder::default()
        // Configure autostart here: this plugin accepts no JSON object configuration.
        // Adding plugins.autostart to tauri.conf.json aborts startup on both platforms.
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(|app,_shortcut,event|{if event.state()==ShortcutState::Pressed{let request=next_picker_request(app);let app=app.clone();tauri::async_runtime::spawn_blocking(move||{let _=open_picker(&app,request,PickerSource::Shortcut);});}}).build())
        .setup(|app|{
            let data_path=app.path().app_data_dir()?.join("settings.json"); let settings: Settings=fs::read(&data_path).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_default();
            let shortcut=settings.shortcut.clone(); app.manage(AppState{settings:Mutex::new(settings),captures:Mutex::new(HashMap::new()),picker_generation:AtomicU64::new(0),toast_generation:AtomicU64::new(0),picking:AtomicBool::new(false),screen_permission_requested:AtomicBool::new(false),data_path});
            app.global_shortcut().register(shortcut.as_str())?;
            let show=MenuItem::with_id(app,"show","打开取色鸭",true,None::<&str>)?;let pick=MenuItem::with_id(app,"pick","开始取色",true,None::<&str>)?;let quit=MenuItem::with_id(app,"quit","退出",true,None::<&str>)?;let menu=Menu::with_items(app,&[&show,&pick,&quit])?;
            // Windows needs the colored, transparent rounded icon: the old tray.png
            // was an opaque white square and disappeared against the taskbar.
            #[cfg(target_os = "windows")]
            let tray_icon_bytes = include_bytes!("../icons/32x32.png").as_slice();
            #[cfg(not(target_os = "windows"))]
            let tray_icon_bytes = include_bytes!("../../src/assets/tray.png").as_slice();
            let tray_icon = image::load_from_memory(tray_icon_bytes)
                .map(|image| {
                    let rgba = image.into_rgba8();
                    let (width, height) = rgba.dimensions();
                    tauri::image::Image::new_owned(rgba.into_raw(), width, height)
                })
                .unwrap_or_else(|_| app.default_window_icon().unwrap().clone());
            let tray=TrayIconBuilder::new().icon(tray_icon).icon_as_template(cfg!(target_os="macos")).tooltip("取色鸭 · Duck Color Picker").menu(&menu).on_menu_event(|app,event|match event.id.as_ref(){"show"=>{let _=show_main(app);},"pick"=>{let request=next_picker_request(app);let app=app.clone();tauri::async_runtime::spawn_blocking(move||{let _=open_picker(&app,request,PickerSource::Tray);});},"quit"=>app.exit(0),_=>{}});
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            let tray=tray.show_menu_on_left_click(false).on_tray_icon_event(|tray,event|if let TrayIconEvent::Click{button:MouseButton::Left,button_state:MouseButtonState::Up,..}=event{let _=show_main(tray.app_handle());});
            tray.build(app)?;
            if !std::env::args().any(|v|v=="--hidden"){show_main(app.handle()).map_err(std::io::Error::other)?;} else {hide_main(app.handle()).map_err(std::io::Error::other)?;}
            Ok(())
        })
        .on_window_event(|window,event|if window.label()=="main"{if let tauri::WindowEvent::CloseRequested{api,..}=event{api.prevent_close();let _=hide_main(window.app_handle());}})
        .invoke_handler(tauri::generate_handler![get_state,update_preferences,save_shortcut,start_picker,picker_pointer,sample_color,confirm_color,cancel_picker,clear_history,delete_history,copy_value])
        .run(tauri::generate_context!()).expect("取色鸭启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(serde_json::from_str::<CopyFormat>("\"unknown\"").is_err());
    }
}
