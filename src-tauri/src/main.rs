#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, Window};
use tauri_plugin_dialog::DialogExt;

use tuneport_core::engine::{self, ConvertOptions, ConvertResult, Overwrite, Status};

struct AppState {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    /// 命令行传入的路径(把文件拖到 exe 图标上时会有)
    startup: Vec<String>,
    /// 启动时直接打开设置(调试/截图用)
    open_settings: bool,
}

#[tauri::command]
fn startup_paths(state: State<'_, AppState>) -> Vec<String> {
    state.startup.clone()
}

#[tauri::command]
fn startup_flags(state: State<'_, AppState>) -> serde_json::Value {
    serde_json::json!({ "openSettings": state.open_settings })
}

/// 前端把诊断信息写进启动日志(排查界面问题用)
#[tauri::command]
fn debug_report(msg: String) {
    log(&format!("UI {}", msg));
}

/// 创建目录并验证可写。绕开文件选择框"新建文件夹"在某些盘上会报权限错的问题。
#[tauri::command]
fn ensure_dir(path: String) -> Result<String, String> {
    let p = PathBuf::from(path.trim());
    if p.as_os_str().is_empty() {
        return Err("路径为空".into());
    }
    log(&format!("ensure_dir: {:?}", p));
    std::fs::create_dir_all(&p).map_err(|e| {
        log(&format!("ensure_dir mkdir failed: {}", e));
        format!("创建目录失败:{}", e)
    })?;
    let probe = p.join(".tuneport-write-test");
    std::fs::write(&probe, b"x").map_err(|e| {
        log(&format!("ensure_dir write failed: {} ({:?})", e, probe));
        if restricted_env() {
            RESTRICTED_HINT.to_string()
        } else {
            format!("目录不可写:{}", e)
        }
    })?;
    let _ = std::fs::remove_file(&probe);
    log("ensure_dir ok");
    Ok(p.to_string_lossy().to_string())
}

/// 探测某个目录能否写文件
fn write_probe(dir: &std::path::Path) -> bool {
    probe_detail(dir) == "ok"
}

/// 返回 "ok" 或具体错误,便于定位被什么挡住
fn probe_detail(dir: &std::path::Path) -> String {
    let f = dir.join(".tuneport-probe");
    match std::fs::write(&f, b"x") {
        Ok(_) => {
            let _ = std::fs::remove_file(&f);
            "ok".to_string()
        }
        Err(e) => format!("{}", e),
    }
}

/// 是否运行在受限环境:连临时目录都写不了,只有 exe 同级可写。
/// 典型场景:从聊天窗口/沙箱里打开 exe,继承了受限令牌。
fn restricted_env() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        let tmp_ok = write_probe(&std::env::temp_dir());
        let exe_dir_ok = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| write_probe(p)))
            .unwrap_or(false);
        !tmp_ok && exe_dir_ok
    })
}

const RESTRICTED_HINT: &str =
    "当前程序运行在受限目录里,无法写入任何位置。请关闭窗口,双击同目录下的「启动.bat」重新打开——它会把程序复制到用户目录后再启动,那里不受限制。";

#[tauri::command]
fn env_status() -> serde_json::Value {
    serde_json::json!({
        "restricted": restricted_env(),
        "tempWritable": write_probe(&std::env::temp_dir()),
    })
}

/* ---------------- 设置 ---------------- */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    template: String,
    overwrite: String,   // rename | skip | overwrite
    concurrency: usize,  // 0 = 自动
    out_dir: Option<String>,
    open_after: bool,
    copy_lrc: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            template: "{artist} - {title}".to_string(),
            overwrite: "rename".to_string(),
            concurrency: 0,
            out_dir: None,
            open_after: false,
            copy_lrc: true,
        }
    }
}

impl Settings {
    /// 兜底修正:模板必须含 {title},否则整张专辑会挤成同名
    fn sanitized(mut self) -> Self {
        if !self.template.contains("{title}") {
            self.template = "{artist} - {title}".to_string();
        }
        if self.concurrency > 16 {
            self.concurrency = 16;
        }
        if !matches!(self.overwrite.as_str(), "rename" | "skip" | "overwrite") {
            self.overwrite = "rename".to_string();
        }
        self
    }
}

fn settings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
}

/// 读文本,顺手吃掉 UTF-8 BOM(记事本 / PowerShell 写出来会带)
fn read_text_no_bom(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let body = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        &bytes[..]
    };
    Some(String::from_utf8_lossy(body).into_owned())
}

/// 读:文件不存在/损坏都回退默认值,绝不 panic
fn read_settings_file(path: &std::path::Path) -> Settings {
    if let Some(txt) = read_text_no_bom(path) {
        match serde_json::from_str::<Settings>(&txt) {
            Ok(s) => return s.sanitized(),
            Err(e) => log(&format!("settings parse failed, fallback to default: {}", e)),
        }
    }
    Settings::default()
}

/// 写:返回修正后的设置(前端用返回值刷新界面)
fn write_settings_file(path: &std::path::Path, settings: Settings) -> Result<Settings, String> {
    let s = settings.sanitized();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败:{}", e))?;
    }
    let txt = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
    std::fs::write(path, txt).map_err(|e| format!("保存失败:{}", e))?;
    Ok(s)
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Settings {
    match settings_path(&app) {
        Some(p) => {
            let s = read_settings_file(&p);
            log(&format!("settings loaded from {:?}", p));
            s
        }
        None => Settings::default(),
    }
}

/* ---------------- 断点续跑:把列表存盘 ---------------- */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct QueueItem {
    path: String,
    status: String,
    out: Option<String>,
    format: Option<String>,
    title: String,
    artist: String,
    album: String,
    size: u64,
    ms: u64,
    error_code: Option<String>,
    error_msg: Option<String>,
    lrc_copied: bool,
    has_lrc: bool,
}

fn queue_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("queue.json"))
}

fn write_queue_file(path: &std::path::Path, items: &[QueueItem]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败:{}", e))?;
    }
    let txt = serde_json::to_string(items).map_err(|e| e.to_string())?;
    // 先写临时文件再改名,避免写一半被打断留下坏文件
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, txt).map_err(|e| format!("写队列失败:{}", e))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("写队列失败:{}", e))?;
    Ok(())
}

fn read_queue_file(path: &std::path::Path) -> Vec<QueueItem> {
    if !path.exists() {
        return Vec::new();
    }
    match read_text_no_bom(path) {
        Some(txt) => serde_json::from_str(&txt).unwrap_or_else(|e| {
            log(&format!("queue parse failed: {}", e));
            Vec::new()
        }),
        None => Vec::new(),
    }
}

#[tauri::command]
fn save_queue(app: AppHandle, items: Vec<QueueItem>) -> Result<(), String> {
    let p = queue_path(&app).ok_or_else(|| "找不到配置目录".to_string())?;
    write_queue_file(&p, &items)
}

#[tauri::command]
fn load_queue(app: AppHandle) -> Vec<QueueItem> {
    match queue_path(&app) {
        Some(p) => {
            let v = read_queue_file(&p);
            log(&format!("queue restored: {} items", v.len()));
            v
        }
        None => Vec::new(),
    }
}

#[tauri::command]
fn clear_queue(app: AppHandle) -> Result<(), String> {
    if let Some(p) = queue_path(&app) {
        let _ = std::fs::remove_file(&p);
    }
    Ok(())
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    let p = settings_path(&app).ok_or_else(|| "找不到配置目录".to_string())?;
    let s = write_settings_file(&p, settings)?;
    log(&format!("settings saved to {:?}", p));
    Ok(s)
}

#[tauri::command]
fn open_dir(path: String) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

/// 试听:把音频文件读成字节返回(前端转 Blob 播放,不落地临时文件)
#[tauri::command]
fn read_audio(path: String) -> Result<tauri::ipc::Response, String> {
    let meta = std::fs::metadata(&path).map_err(|e| format!("读取失败:{}", e))?;
    if meta.len() > 200 * 1024 * 1024 {
        return Err("文件超过 200 MB,不支持试听".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("读取失败:{}", e))?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartArgs {
    items: Vec<String>,
    out_dir: Option<String>,
    copy_lrc: Option<bool>,
    template: Option<String>,
    overwrite: Option<String>,
    concurrency: Option<usize>,
}

#[derive(Serialize)]
struct StatItem {
    path: String,
    size: u64,
}

fn to_strings(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect()
}

#[tauri::command]
async fn pick_files(app: AppHandle) -> Result<Vec<String>, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter(
            "加密音频文件",
            &[
                "ncm", "kwm", "qmc", "qmc0", "qmc2", "qmc3", "qmcflac", "qmcogg", "mflac",
                "mflac0", "mflach", "mgg", "mgg0", "mgg1", "mggl",
            ],
        )
        .blocking_pick_files();
    Ok(match picked {
        Some(list) => to_strings(list.into_iter().filter_map(|p| p.into_path().ok()).collect()),
        None => vec![],
    })
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Vec<String>, String> {
    // 支持一次选多个文件夹(按住 Ctrl/Shift 多选)
    let picked = app.dialog().file().blocking_pick_folders();
    let mut out = Vec::new();
    if let Some(dirs) = picked {
        for d in dirs {
            if let Ok(p) = d.into_path() {
                engine::scan_folder(&p, &mut out);
            }
        }
    }
    Ok(to_strings(out))
}

#[tauri::command]
async fn pick_output_dir(app: AppHandle) -> Result<Option<String>, String> {
    let picked = app.dialog().file().blocking_pick_folder();
    Ok(picked
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
fn scan_paths(paths: Vec<String>) -> Vec<String> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        let pb = PathBuf::from(&p);
        if pb.is_dir() {
            engine::scan_folder(&pb, &mut out);
        } else if engine::is_supported(&pb) {
            out.push(pb);
        }
    }
    to_strings(out)
}

#[tauri::command]
fn stat_files(paths: Vec<String>) -> Vec<StatItem> {
    paths
        .into_iter()
        .map(|p| {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            StatItem { path: p, size }
        })
        .collect()
}

#[tauri::command]
fn start_batch(app: AppHandle, state: State<'_, AppState>, args: StartArgs) -> Result<(), String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Err("已有任务在运行".into());
    }
    state.cancel.store(false, Ordering::SeqCst);

    let files: Vec<PathBuf> = args.items.iter().map(PathBuf::from).collect();
    if files.is_empty() {
        state.running.store(false, Ordering::SeqCst);
        return Err("没有待处理的文件".into());
    }

    let opts = ConvertOptions {
        out_dir: args.out_dir.map(PathBuf::from),
        copy_lrc: args.copy_lrc.unwrap_or(true),
        template: args
            .template
            .unwrap_or_else(|| "{artist} - {title}".to_string()),
        overwrite: match args.overwrite.as_deref() {
            Some("skip") => Overwrite::Skip,
            Some("overwrite") => Overwrite::Overwrite,
            _ => Overwrite::Rename,
        },
    };

    let cpu = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(4)
        .max(1);
    let concurrency = args.concurrency.unwrap_or(cpu);

    let cancel = state.cancel.clone();
    let running = state.running.clone();
    let total = files.len();

    let app_start = app.clone();
    let app_result = app.clone();
    let app_done = app.clone();

    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let mut done = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let mut lrc = 0usize;

        engine::run_batch(
            &files,
            &opts,
            concurrency,
            Some(cancel),
            |p| {
                let _ = app_start.emit(
                    "task-start",
                    serde_json::json!({ "path": p.to_string_lossy() }),
                );
            },
            |r: ConvertResult| {
                match r.status {
                    Status::Done => done += 1,
                    Status::Failed => failed += 1,
                    _ => skipped += 1,
                }
                if r.lrc_copied {
                    lrc += 1;
                }
                let _ = app_result.emit("task-result", &r);
            },
        );

        let _ = app_done.emit(
            "batch-done",
            serde_json::json!({
                "total": total,
                "done": done,
                "failed": failed,
                "skipped": skipped,
                "cancelled": 0,
                "lrc": lrc,
                "ms": t0.elapsed().as_millis() as u64,
            }),
        );
        running.store(false, Ordering::SeqCst);
    });

    Ok(())
}

#[tauri::command]
fn cancel_batch(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn reveal(path: String) {
    let p = PathBuf::from(path);
    let arg = format!("/select,{}", p.display());
    let _ = std::process::Command::new("explorer").arg(arg).spawn();
}

#[tauri::command]
fn win_minimize(window: Window) {
    let _ = window.minimize();
}

#[tauri::command]
fn win_maximize(window: Window) {
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    } else {
        let _ = window.maximize();
    }
}

#[tauri::command]
fn win_close(window: Window) {
    let _ = window.close();
}

/// 无界面转换:argv = [输出目录, 文件或目录...]
fn run_smoke(args: &[String]) -> i32 {
    if args.len() < 2 {
        eprintln!("用法: tuneport --smoke <输出目录> <文件或目录...>");
        return 2;
    }
    let out_dir = PathBuf::from(&args[0]);
    let mut files: Vec<PathBuf> = Vec::new();
    for a in &args[1..] {
        let p = PathBuf::from(a);
        if p.is_dir() {
            engine::scan_folder(&p, &mut files);
        } else {
            files.push(p);
        }
    }

    let opts = ConvertOptions {
        out_dir: Some(out_dir.clone()),
        copy_lrc: true,
        ..Default::default()
    };

    let mut ok = 0usize;
    let mut fail = 0usize;
    engine::run_batch(&files, &opts, 2, None, |_| {}, |r: ConvertResult| match r.status {
        Status::Done => {
            ok += 1;
            println!("OK   {} -> {}", r.src, r.out.clone().unwrap_or_default());
        }
        Status::Failed => {
            fail += 1;
            println!("FAIL {} [{}] {}", r.src, r.error_code.clone().unwrap_or_default(), r.error_msg.clone().unwrap_or_default());
        }
        Status::Skipped => {
            println!("SKIP {}", r.src);
        }
    });

    println!("SMOKE ok={} fail={}", ok, fail);
    if ok > 0 && fail == 0 {
        0
    } else {
        1
    }
}

fn log_path() -> PathBuf {
    // 优先放到用户目录,安装目录保持干净(卸载不残留);
    // 受限环境写不了就退回 exe 旁边,再不行退临时目录。
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        let dir = PathBuf::from(base).join("com.tuneport.app");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("tuneport-startup.log");
        if std::fs::OpenOptions::new().create(true).append(true).open(&p).is_ok() {
            return p;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("tuneport-startup.log");
        }
    }
    std::env::temp_dir().join("tuneport-startup.log")
}

/// 启动全过程写文件:release 版是 GUI 子系统,控制台看不到任何输出
pub fn log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(f, "{}", msg);
    }
}

#[cfg(target_os = "windows")]
fn show_error_box(title: &str, msg: &str) {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut std::ffi::c_void, text: *const u16, caption: *const u16, u_type: u32) -> i32;
    }
    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let (t, c) = (wide(msg), wide(title));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), 0x10);
    }
}

/// 把窗口描边设成标题栏同色(仅 Win11 生效;Win10 上是空操作,保持系统默认)
#[cfg(target_os = "windows")]
fn set_window_border_color(hwnd: isize, colorref: u32) {
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attr: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }

    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_CAPTION_COLOR: u32 = 35;
    let color = colorref;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &color as *const u32 as *const _,
            4,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &color as *const u32 as *const _,
            4,
        );
    }
}

/// 建主窗口。WebView2 的数据目录逐个试:用户数据目录 → exe 同级 → 配置目录。
/// (用户目录优先,这样安装版卸载后不会在安装目录留下几十 MB 缓存)
fn build_main_window(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(d) = app.path().app_local_data_dir() {
        candidates.push(d.join("EBWebView"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("webview-data"));
        }
    }
    if let Ok(d) = app.path().app_config_dir() {
        candidates.push(d.join("WebView2"));
    }

    let url = tauri::WebviewUrl::App("index.html".into());
    let mut last_err: Option<String> = None;

    for dir in candidates {
        let _ = std::fs::create_dir_all(&dir);
        log(&format!("try data dir {:?}", dir));
        let builder = tauri::WebviewWindowBuilder::new(app, "main", url.clone())
            .title("TunePort")
            .inner_size(1280.0, 800.0)
            .min_inner_size(1000.0, 640.0)
            .decorations(false)
            .background_color(tauri::window::Color(243, 243, 243, 255))
            .center()
            .data_directory(dir);
        match builder.build() {
            Ok(w) => {
                log("window built OK");
                let _ = w.show();
                let _ = w.set_focus();
                let _ = w.center();
                #[cfg(target_os = "windows")]
                if let Ok(hwnd) = w.hwnd() {
                    set_window_border_color(hwnd.0 as isize, 0x00F3F3F3);
                }
                log("window shown & centered");
                return Ok(());
            }
            Err(e) => {
                log(&format!("build failed: {}", e));
                last_err = Some(e.to_string());
            }
        }
    }

    let msg = format!(
        "窗口创建失败:{}\n\n日志文件:{}",
        last_err.unwrap_or_else(|| "未知原因".into()),
        log_path().display()
    );
    log(&msg);
    #[cfg(target_os = "windows")]
    show_error_box("TunePort · 启动失败", &msg);
    Err(msg.into())
}

fn main() {    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{}", info);
        log(&format!("PANIC {}", msg));
        #[cfg(target_os = "windows")]
        show_error_box(
            "TunePort · 启动失败",
            &format!("{}\n\n日志文件:{}", msg, log_path().display()),
        );
    }));
    log(&format!(
        "=== start exe={:?} cwd={:?}",
        std::env::current_exe().ok(),
        std::env::current_dir().ok()
    ));

    // 无界面模式:--smoke <输出目录> <文件或目录...>
    // 用于在没法开窗口的环境里验证二进制与引擎,也方便脚本化。
    let argv: Vec<String> = std::env::args().collect();
    if let Some(pos) = argv.iter().position(|a| a == "--smoke") {
        std::process::exit(run_smoke(&argv[pos + 1..]));
    }

    log("before tauri::Builder");

    {
        let tmp = std::env::temp_dir();
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default();
        log(&format!(
            "probe: temp({:?})={} | localappdata({:?})={} | exedir={} | F_root={}",
            tmp,
            probe_detail(&tmp),
            local,
            probe_detail(&local),
            probe_detail(&exe_dir),
            probe_detail(std::path::Path::new("F:\\"))
        ));
    }

    let startup: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let open_settings = std::env::args().any(|a| a == "--settings");
    log(&format!("startup paths: {:?}", startup));

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            startup,
            open_settings,
        })
        .setup(|app| {
            log("setup() reached");
            build_main_window(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pick_files,
            pick_folder,
            pick_output_dir,
            scan_paths,
            stat_files,
            startup_paths,
            startup_flags,
            debug_report,
            ensure_dir,
            env_status,
            load_settings,
            save_settings,
            save_queue,
            load_queue,
            clear_queue,
            open_dir,
            read_audio,
            start_batch,
            cancel_batch,
            reveal,
            win_minimize,
            win_maximize,
            win_close
        ])
        .run(tauri::generate_context!());

    match result {
        Ok(()) => log("run() returned Ok (窗口正常关闭)"),
        Err(e) => log(&format!("run() ERROR: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        // 用 target/ 下的目录:某些环境对 %TEMP% 写入有限制
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        let d = base.join(format!("{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn settings_roundtrip() {
        let d = tmp("roundtrip");
        let p = d.join("settings.json");
        let s = Settings {
            template: "{artist} - {title} - {album}".into(),
            overwrite: "skip".into(),
            concurrency: 4,
            out_dir: Some("F:\\Music\\out".into()),
            open_after: true,
            copy_lrc: false,
        };
        let saved = write_settings_file(&p, s).unwrap();
        assert_eq!(saved.concurrency, 4);

        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(raw.contains("\"template\""), "{}", raw);
        assert!(raw.contains("\"openAfter\""), "字段名应为 camelCase:{}", raw);

        let back = read_settings_file(&p);
        assert_eq!(back.template, "{artist} - {title} - {album}");
        assert_eq!(back.overwrite, "skip");
        assert_eq!(back.concurrency, 4);
        assert_eq!(back.out_dir.as_deref(), Some("F:\\Music\\out"));
        assert!(back.open_after);
        assert!(!back.copy_lrc);

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_or_broken_file_falls_back_to_default() {
        let d = tmp("fallback");
        let p = d.join("settings.json");

        let s = read_settings_file(&p);
        assert_eq!(s.template, "{artist} - {title}");
        assert!(s.copy_lrc);

        std::fs::write(&p, b"{ this is not json").unwrap();
        let s2 = read_settings_file(&p);
        assert_eq!(s2.template, "{artist} - {title}", "损坏文件应回退默认值,不能崩");

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sanitize_repairs_bad_values() {
        let s = Settings {
            template: "{album}".into(),
            overwrite: "boom".into(),
            concurrency: 999,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s.template, "{artist} - {title}", "模板缺 title 要回退");
        assert_eq!(s.overwrite, "rename");
        assert_eq!(s.concurrency, 16);
    }

    #[test]
    fn queue_roundtrip() {
        let d = tmp("queue");
        let p = d.join("queue.json");
        let items = vec![
            QueueItem {
                path: r"C:\m\a.ncm".into(),
                status: "done".into(),
                out: Some(r"C:\m\A - B.flac".into()),
                format: Some("flac".into()),
                title: "B".into(),
                artist: "A".into(),
                album: "C".into(),
                size: 12345,
                ms: 900,
                ..Default::default()
            },
            QueueItem {
                path: r"C:\m\b.kwm".into(),
                status: "failed".into(),
                error_code: Some("E_KEY".into()),
                error_msg: Some("密钥错误".into()),
                size: 999,
                ..Default::default()
            },
        ];
        write_queue_file(&p, &items).unwrap();

        let back = read_queue_file(&p);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].path, r"C:\m\a.ncm");
        assert_eq!(back[0].status, "done");
        assert_eq!(back[0].out.as_deref(), Some(r"C:\m\A - B.flac"));
        assert_eq!(back[0].title, "B");
        assert_eq!(back[0].size, 12345);
        assert_eq!(back[1].error_code.as_deref(), Some("E_KEY"));

        // 临时文件不该留下
        assert!(!p.with_extension("json.tmp").exists(), "不该残留 .tmp");

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn queue_missing_or_broken_is_empty() {
        let d = tmp("queue-bad");
        let p = d.join("queue.json");
        assert!(read_queue_file(&p).is_empty(), "文件不存在应返回空");
        std::fs::write(&p, b"[{ broken").unwrap();
        assert!(read_queue_file(&p).is_empty(), "坏文件应返回空而不是崩");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reads_files_with_utf8_bom() {
        // 记事本 / PowerShell 写出来的 UTF-8 会带 BOM
        let d = tmp("bom");
        let p = d.join("queue.json");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"[{"path":"C:\\a.ncm","status":"pending"}]"#);
        std::fs::write(&p, bytes).unwrap();
        let items = read_queue_file(&p);
        assert_eq!(items.len(), 1, "带 BOM 也要能读出来");
        assert_eq!(items[0].path, r"C:\a.ncm");

        let sp = d.join("settings.json");
        let mut sb = vec![0xEF, 0xBB, 0xBF];
        sb.extend_from_slice(br#"{"template":"{title} - {artist}"}"#);
        std::fs::write(&sp, sb).unwrap();
        assert_eq!(read_settings_file(&sp).template, "{title} - {artist}");

        let _ = std::fs::remove_dir_all(&d);
    }
}
