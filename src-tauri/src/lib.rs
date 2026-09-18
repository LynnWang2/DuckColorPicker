use arboard::Clipboard;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Mutex},
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WebviewUrl,
    WebviewWindowBuilder,
};
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
    auto_start: bool,
    history: Vec<ColorRecord>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shortcut: "CommandOrControl+Shift+C".into(),
            theme: "system".into(),
            formats: Formats::default(),
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
    picking: AtomicBool,
    data_path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Preferences { theme: Option<String>, formats: Option<Formats>, auto_start: Option<bool> }

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

fn close_pickers(app: &AppHandle) {
    let state = app.state::<AppState>();
    let labels: Vec<_> = app.webview_windows().keys().filter(|k| k.starts_with("picker-")).cloned().collect();
    for label in labels { if let Some(window) = app.get_webview_window(&label) { let _ = window.close(); } }
    if let Ok(mut frames) = state.captures.lock() { frames.clear(); }
    state.picking.store(false, Ordering::SeqCst);
}

#[cfg(target_os = "macos")]
fn ensure_screen_capture_permission() -> Result<(), String> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    if unsafe { CGPreflightScreenCaptureAccess() } { return Ok(()); }
    // Do not call CGRequestScreenCaptureAccess while picking: on an unsigned
    // build with a stale TCC grant, macOS may show its dialog on every attempt.
    Err("当前安装的取色鸭尚未获得有效录屏权限。请在系统设置 → 隐私与安全性 → 屏幕与系统音频录制中重新添加并允许当前版本，然后完全退出并重新打开应用".into())
}

fn show_copied_toast(app: &AppHandle, hex: &str) {
    if let Some(existing) = app.get_webview_window("copied-toast") { let _ = existing.close(); }
    let url = format!("toast.html?hex={}", hex.trim_start_matches('#'));
    if let Ok(window) = WebviewWindowBuilder::new(app, "copied-toast", WebviewUrl::App(url.into()))
        .title("已复制色值").decorations(false).transparent(true).shadow(false)
        .always_on_top(true).skip_taskbar(true).focused(false)
        .inner_size(240.0, 52.0).center().build() {
        tauri::async_runtime::spawn_blocking(move || {
            std::thread::sleep(Duration::from_millis(1700));
            let _ = window.close();
        });
    }
}

#[tauri::command]
fn get_state(state: State<AppState>) -> Result<Settings, String> { state.settings.lock().map(|v| v.clone()).map_err(|_| "设置被占用".into()) }

#[tauri::command]
fn update_preferences(app: AppHandle, state: State<AppState>, preferences: Preferences) -> Result<(), String> {
    { let mut s=state.settings.lock().map_err(|_| "设置被占用")?; if let Some(v)=preferences.theme {s.theme=v;} if let Some(v)=preferences.formats{s.formats=v;} if let Some(v)=preferences.auto_start{s.auto_start=v;} }
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

fn open_picker(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.picking.swap(true, Ordering::SeqCst) { return Ok(()); }
    close_pickers(app); state.picking.store(true,Ordering::SeqCst);
    let result=(|| {
        #[cfg(target_os = "macos")]
        ensure_screen_capture_permission()?;
        if let Some(main) = app.get_webview_window("main") { main.hide().map_err(|e| e.to_string())?; }
        // Give the compositor time to remove the main window before taking the snapshot.
        std::thread::sleep(Duration::from_millis(180));
        let monitors=Monitor::all().map_err(|e| format!("无法读取屏幕：{e}"))?;
        if monitors.is_empty(){return Err("没有检测到显示器".into());}
        let mut frames=state.captures.lock().map_err(|_| "取色缓存被占用")?;
        for (index,monitor) in monitors.into_iter().enumerate(){
            let image=monitor.capture_image().map_err(|e| format!("无法截取屏幕，请授予屏幕录制权限：{e}"))?;
            let label=format!("picker-{index}"); let width=image.width(); let height=image.height();
            let x=monitor.x().map_err(|e|e.to_string())? as f64; let y=monitor.y().map_err(|e|e.to_string())? as f64;
            let view_width=monitor.width().map_err(|e|e.to_string())? as f64;
            let view_height=monitor.height().map_err(|e|e.to_string())? as f64;
            WebviewWindowBuilder::new(app,&label,WebviewUrl::App("picker.html".into()))
                .title("取色鸭").decorations(false).transparent(true).shadow(false).always_on_top(true).skip_taskbar(true)
                .position(x,y).inner_size(view_width,view_height).focused(true).build().map_err(|e|e.to_string())?;
            frames.insert(label,CaptureFrame{width,height,rgba:image.into_raw()});
        }
        Ok(())
    })();
    if result.is_err(){ close_pickers(app); }
    result
}

#[tauri::command]
async fn start_picker(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_picker(&app)).await.map_err(|e| e.to_string())?
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
    Clipboard::new().and_then(|mut c|c.set_text(color.hex.trim_start_matches('#'))).map_err(|e|format!("复制失败：{e}"))?;
    {let mut s=state.settings.lock().map_err(|_|"设置被占用")?;s.history.insert(0,record.clone());s.history.truncate(24);} save(&state)?;close_pickers(&app);emit_state(&app,&state)?;show_copied_toast(&app,&record.hex);app.emit("picker-result",record).map_err(|e|e.to_string())
}

#[tauri::command]
fn cancel_picker(app:AppHandle)->Result<(),String>{close_pickers(&app);app.emit("picker-cancelled",()).map_err(|e|e.to_string())}

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
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(|app,_shortcut,event|{if event.state()==ShortcutState::Pressed{let app=app.clone();tauri::async_runtime::spawn_blocking(move||{let _=open_picker(&app);});}}).build())
        .setup(|app|{
            let data_path=app.path().app_data_dir()?.join("settings.json"); let settings: Settings=fs::read(&data_path).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_default();
            let shortcut=settings.shortcut.clone(); app.manage(AppState{settings:Mutex::new(settings),captures:Mutex::new(HashMap::new()),picking:AtomicBool::new(false),data_path});
            app.global_shortcut().register(shortcut.as_str())?;
            let show=MenuItem::with_id(app,"show","打开取色鸭",true,None::<&str>)?;let pick=MenuItem::with_id(app,"pick","开始取色",true,None::<&str>)?;let quit=MenuItem::with_id(app,"quit","退出",true,None::<&str>)?;let menu=Menu::with_items(app,&[&show,&pick,&quit])?;
            TrayIconBuilder::new().icon(app.default_window_icon().unwrap().clone()).tooltip("取色鸭 · Duck Color Picker").menu(&menu).on_menu_event(|app,event|match event.id.as_ref(){"show"=>{if let Some(w)=app.get_webview_window("main"){let _=w.show();let _=w.set_focus();}},"pick"=>{let app=app.clone();tauri::async_runtime::spawn_blocking(move||{let _=open_picker(&app);});},"quit"=>app.exit(0),_=>{}}).on_tray_icon_event(|tray,event|if let TrayIconEvent::Click{button:MouseButton::Left,button_state:MouseButtonState::Up,..}=event{let app=tray.app_handle();if let Some(w)=app.get_webview_window("main"){let _=w.show();let _=w.set_focus();}}).build(app)?;
            if !std::env::args().any(|v|v=="--hidden"){if let Some(w)=app.get_webview_window("main"){w.show()?;}}
            Ok(())
        })
        .on_window_event(|window,event|if window.label()=="main"{if let tauri::WindowEvent::CloseRequested{api,..}=event{api.prevent_close();let _=window.hide();}})
        .invoke_handler(tauri::generate_handler![get_state,update_preferences,save_shortcut,start_picker,sample_color,confirm_color,cancel_picker,clear_history,delete_history,copy_value])
        .run(tauri::generate_context!()).expect("取色鸭启动失败");
}
