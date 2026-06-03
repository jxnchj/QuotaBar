mod claude;
mod claude_local;
mod codex;
mod codex_local;
mod kimi;

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{
    include_image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow,
};

// --- Config persistence -----------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct AppConfig {
    claude_session_key: Option<String>,
    /// kimi-auth JWT (pasted by user) for the Kimi web subscription.
    #[serde(default)]
    kimi_auth_token: Option<String>,
    /// Which providers to surface in the menubar title. `None` = auto (show all
    /// that have data, capped at 2); `Some(list)` = show exactly these
    /// (ids: "claude","codex","kimi"). The menubar shows at most 2.
    #[serde(default)]
    menubar_providers: Option<Vec<String>>,
}

struct ConfigState(Mutex<AppConfig>);

fn config_path() -> PathBuf {
    let base = dirs::config_dir().expect("config dir");
    base.join("QuotaBar").join("config.json")
}

fn load_config() -> AppConfig {
    let path = config_path();
    if !path.exists() {
        return AppConfig::default();
    }
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("read config failed: {e}");
            return AppConfig::default();
        }
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_config(cfg: &AppConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write failed: {e}"))?;
    Ok(())
}

// --- Usage cache shared with frontend ---------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ClaudeSnapshot {
    Idle,                          // no key configured yet
    Loading,                       // fetch in flight, no previous data
    Ok(claude::ClaudeUsage),       // last successful fetch
    Error { message: String },     // last fetch failed (no previous data)
    Stale {                        // last fetch failed but we have prior data
        data: claude::ClaudeUsage,
        error: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum CodexSnapshot {
    Idle,
    Loading,
    Ok(codex::CodexUsage),
    Error { message: String },
}

impl Default for CodexSnapshot {
    fn default() -> Self { CodexSnapshot::Idle }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum KimiSnapshot {
    Idle,
    Loading,
    Ok(kimi::KimiUsage),
    Error { message: String },
}

impl Default for KimiSnapshot {
    fn default() -> Self { KimiSnapshot::Idle }
}

#[derive(Default)]
struct UsageCache {
    claude: Arc<Mutex<ClaudeSnapshot>>,
    claude_local: Arc<Mutex<Option<claude_local::ClaudeTokenSummary>>>,
    codex: Arc<Mutex<CodexSnapshot>>,
    codex_local: Arc<Mutex<Option<codex_local::CodexLocalSummary>>>,
    kimi: Arc<Mutex<KimiSnapshot>>,
}

impl Default for ClaudeSnapshot {
    fn default() -> Self {
        ClaudeSnapshot::Idle
    }
}

// --- Tauri commands ---------------------------------------------------------

#[tauri::command]
fn get_config(state: tauri::State<'_, ConfigState>) -> AppConfig {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
fn set_claude_session_key(
    key: String,
    state: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    let trimmed = key.trim();
    let mut cfg = state.0.lock().unwrap();
    cfg.claude_session_key = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    save_config(&cfg)
}

#[tauri::command]
fn set_kimi_token(token: String, state: tauri::State<'_, ConfigState>) -> Result<(), String> {
    let trimmed = token.trim();
    let mut cfg = state.0.lock().unwrap();
    cfg.kimi_auth_token = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    save_config(&cfg)
}

#[tauri::command]
fn get_kimi_snapshot(cache: tauri::State<'_, UsageCache>) -> KimiSnapshot {
    cache.kimi.lock().unwrap().clone()
}

#[tauri::command]
fn set_menubar_providers(providers: Vec<String>, app: AppHandle) -> Result<(), String> {
    {
        let cfg = app.state::<ConfigState>();
        let mut guard = cfg.0.lock().unwrap();
        guard.menubar_providers = Some(providers);
        save_config(&guard)?;
    }
    update_tray_title(&app);
    Ok(())
}

#[tauri::command]
fn get_claude_snapshot(cache: tauri::State<'_, UsageCache>) -> ClaudeSnapshot {
    cache.claude.lock().unwrap().clone()
}

#[tauri::command]
fn get_claude_local_summary(
    cache: tauri::State<'_, UsageCache>,
) -> Option<claude_local::ClaudeTokenSummary> {
    cache.claude_local.lock().unwrap().clone()
}

#[tauri::command]
fn get_codex_snapshot(cache: tauri::State<'_, UsageCache>) -> CodexSnapshot {
    cache.codex.lock().unwrap().clone()
}

#[tauri::command]
fn get_codex_local_summary(
    cache: tauri::State<'_, UsageCache>,
) -> Option<codex_local::CodexLocalSummary> {
    cache.codex_local.lock().unwrap().clone()
}

#[tauri::command]
async fn refresh_claude_now(app: AppHandle) -> Result<(), String> {
    let cache = app.state::<UsageCache>();
    let cfg = app.state::<ConfigState>();
    let key = cfg.0.lock().unwrap().claude_session_key.clone();
    if let Some(key) = key {
        refresh_claude(&app, &key, &cache.claude).await;
    }
    // Always refresh local even if no sessionKey configured.
    refresh_claude_local(&app);
    refresh_codex(&app).await;
    refresh_codex_local(&app);
    refresh_kimi(&app).await;
    Ok(())
}

// --- Refresh helpers --------------------------------------------------------

async fn refresh_claude(
    app: &AppHandle,
    session_key: &str,
    snapshot: &Arc<Mutex<ClaudeSnapshot>>,
) {
    // Mark loading if we have no data yet, otherwise leave existing visible.
    {
        let mut s = snapshot.lock().unwrap();
        if matches!(*s, ClaudeSnapshot::Idle | ClaudeSnapshot::Error { .. }) {
            *s = ClaudeSnapshot::Loading;
        }
    }

    let result = claude::fetch(session_key).await;

    {
        let mut s = snapshot.lock().unwrap();
        match result {
            Ok(data) => {
                *s = ClaudeSnapshot::Ok(data);
            }
            Err(err) => {
                let message = err.to_string();
                *s = match std::mem::take(&mut *s) {
                    ClaudeSnapshot::Ok(prev) | ClaudeSnapshot::Stale { data: prev, .. } => {
                        ClaudeSnapshot::Stale { data: prev, error: message }
                    }
                    _ => ClaudeSnapshot::Error { message },
                };
            }
        }
    }

    // Update tray title with quick summary and emit to frontend.
    update_tray_title(app);
    let _ = app.emit("claude-snapshot-updated", ());
}

fn refresh_claude_local(app: &AppHandle) {
    // Cheap (~70ms full scan), do it inline.
    let summary = claude_local::scan_token_history();
    {
        let cache = app.state::<UsageCache>();
        *cache.claude_local.lock().unwrap() = Some(summary);
    }
    let _ = app.emit("claude-local-updated", ());
}

fn claude_primary_percent(snap: &ClaudeSnapshot) -> Option<f64> {
    match snap {
        ClaudeSnapshot::Ok(d) | ClaudeSnapshot::Stale { data: d, .. } => {
            d.five_hour.as_ref().map(|w| w.percent)
        }
        _ => None,
    }
}

fn codex_primary_percent(snap: &CodexSnapshot) -> Option<f64> {
    match snap {
        CodexSnapshot::Ok(d) => d.primary.as_ref().map(|w| w.percent),
        _ => None,
    }
}

fn kimi_primary_percent(snap: &KimiSnapshot) -> Option<f64> {
    match snap {
        KimiSnapshot::Ok(d) => d.weekly.as_ref().map(|w| w.percent),
        _ => None,
    }
}

/// Build the menubar title from the configured provider selection. Each provider
/// shows its 5-hour (primary) window percentage, e.g. "C 87% · Cx 44%".
fn update_tray_title(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("main-tray") else { return };
    let cache = app.state::<UsageCache>();
    let selection = app
        .state::<ConfigState>()
        .0
        .lock()
        .unwrap()
        .menubar_providers
        .clone();

    // None = auto (include only providers that have data).
    // Some(list) = include exactly these, showing "—" when a selected one lacks data.
    let explicit = selection.is_some();
    let want = |id: &str| match &selection {
        None => true,
        Some(list) => list.iter().any(|s| s == id),
    };

    let mut parts: Vec<String> = Vec::new();
    if want("claude") {
        match claude_primary_percent(&cache.claude.lock().unwrap()) {
            Some(p) => parts.push(format!("C {:.0}%", p)),
            None if explicit => parts.push("C —".into()),
            None => {}
        }
    }
    if want("codex") {
        match codex_primary_percent(&cache.codex.lock().unwrap()) {
            Some(p) => parts.push(format!("Cx {:.0}%", p)),
            None if explicit => parts.push("Cx —".into()),
            None => {}
        }
    }
    if want("kimi") {
        match kimi_primary_percent(&cache.kimi.lock().unwrap()) {
            Some(p) => parts.push(format!("K {:.0}%", p)),
            None if explicit => parts.push("K —".into()),
            None => {}
        }
    }

    // Menubar space is tight — never show more than two providers.
    parts.truncate(2);

    let title = if parts.is_empty() {
        "Q".to_string()
    } else {
        parts.join(" · ")
    };
    let _ = tray.set_title(Some(title));
}

// --- Popover toggle logic ---------------------------------------------------

fn toggle_popover(app: &AppHandle, tray_pos: Option<PhysicalPosition<f64>>) {
    let Some(window) = app.get_webview_window("popover") else {
        eprintln!("popover window not found");
        return;
    };

    let visible = window.is_visible().unwrap_or(false);
    if visible {
        let _ = window.hide();
        return;
    }

    if let Some(pos) = tray_pos {
        position_popover_near(&window, pos);
    }
    let _ = window.show();
    let _ = window.set_focus();
}

fn position_popover_near(window: &WebviewWindow, tray_pos: PhysicalPosition<f64>) {
    let (win_w, _win_h) = match window.outer_size() {
        Ok(s) => (s.width as f64, s.height as f64),
        Err(_) => (320.0, 240.0),
    };
    let x = tray_pos.x - (win_w / 2.0);
    let y = tray_pos.y + 8.0;
    let _ = window.set_position(PhysicalPosition::new(x.round(), y.round()));
}

// --- Background scheduler ---------------------------------------------------

fn refresh_codex_local(app: &AppHandle) {
    let summary = codex_local::scan_token_history();
    *app.state::<UsageCache>().codex_local.lock().unwrap() = summary;
    let _ = app.emit("codex-local-updated", ());
}

async fn refresh_codex(app: &AppHandle) {
    {
        let cache = app.state::<UsageCache>();
        let mut s = cache.codex.lock().unwrap();
        if matches!(*s, CodexSnapshot::Idle | CodexSnapshot::Error { .. }) {
            *s = CodexSnapshot::Loading;
        }
    }
    let result = codex::fetch().await;
    {
        let cache = app.state::<UsageCache>();
        let mut s = cache.codex.lock().unwrap();
        *s = match result {
            Ok(data) => CodexSnapshot::Ok(data),
            Err(e) => CodexSnapshot::Error { message: e.to_string() },
        };
    }
    update_tray_title(app);
    let _ = app.emit("codex-snapshot-updated", ());
}

async fn refresh_kimi(app: &AppHandle) {
    let manual: Option<String> = {
        let cfg = app.state::<ConfigState>();
        let guard = cfg.0.lock().unwrap();
        guard.kimi_auth_token.clone()
    };
    // Manual token wins; otherwise auto-read it from Chrome (keychain-gated).
    // The read is blocking (file + keychain), so keep it off the async runtime.
    let used_auto = manual.is_none();
    let token = match manual {
        Some(t) => Some(t),
        None => tokio::task::spawn_blocking(kimi::auto_read_token)
            .await
            .ok()
            .flatten(),
    };
    let Some(token) = token else {
        // Surface a hint instead of silently staying Idle, so the auto-read outcome
        // is visible during setup. Don't clobber a previously-good snapshot.
        if used_auto {
            let cache = app.state::<UsageCache>();
            let mut s = cache.kimi.lock().unwrap();
            if !matches!(*s, KimiSnapshot::Ok(_)) {
                *s = KimiSnapshot::Error {
                    message: "未能从 Chrome 自动读取 Kimi 登录态（钥匙串未授权 / 未登录 kimi.com / Chrome 加密不兼容）。可在设置里手动粘贴 kimi-auth。".into(),
                };
            }
            let _ = app.emit("kimi-snapshot-updated", ());
        }
        return;
    };

    {
        let cache = app.state::<UsageCache>();
        let mut s = cache.kimi.lock().unwrap();
        if matches!(*s, KimiSnapshot::Idle | KimiSnapshot::Error { .. }) {
            *s = KimiSnapshot::Loading;
        }
    }
    let result = kimi::fetch(&token).await;
    {
        let cache = app.state::<UsageCache>();
        let mut s = cache.kimi.lock().unwrap();
        *s = match result {
            Ok(data) => KimiSnapshot::Ok(data),
            Err(e) => KimiSnapshot::Error { message: e.to_string() },
        };
    }
    update_tray_title(app);
    let _ = app.emit("kimi-snapshot-updated", ());
}

fn spawn_claude_poller(app: AppHandle) {
    let cache = app.state::<UsageCache>().claude.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let key: Option<String> = {
                let cfg = app.state::<ConfigState>();
                let guard = cfg.0.lock().unwrap();
                guard.claude_session_key.clone()
            };
            if let Some(key) = key {
                refresh_claude(&app, &key, &cache).await;
            }
            // Local JSONL scan is independent of sessionKey — always refresh it.
            refresh_claude_local(&app);
            // Codex wham/usage + local sqlite.
            refresh_codex(&app).await;
            refresh_codex_local(&app);
            // Kimi web subscription (if token configured).
            refresh_kimi(&app).await;
            tokio::time::sleep(Duration::from_secs(300)).await; // 5 minutes
        }
    });
}

// --- Run --------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let initial_cfg = load_config();
    let cfg_state = ConfigState(Mutex::new(initial_cfg));
    let cache = UsageCache::default();

    tauri::Builder::default()
        .manage(cfg_state)
        .manage(cache)
        .invoke_handler(tauri::generate_handler![
            get_config,
            set_claude_session_key,
            set_kimi_token,
            set_menubar_providers,
            get_claude_snapshot,
            get_claude_local_summary,
            get_codex_snapshot,
            get_codex_local_summary,
            get_kimi_snapshot,
            refresh_claude_now,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            let quit_item = MenuItem::with_id(app, "quit", "Quit QuotaBar", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit_item])?;

            let _tray: TrayIcon = TrayIconBuilder::with_id("main-tray")
                .icon(include_image!("./icons/icon.png"))
                .icon_as_template(true)
                .title("Q")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        toggle_popover(tray.app_handle(), Some(position));
                    }
                })
                .build(app)?;

            if let Some(popover) = app.get_webview_window("popover") {
                let popover_clone = popover.clone();
                popover.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        let _ = popover_clone.hide();
                    }
                });
            }

            // Kick off the background poller (it'll wait until sessionKey is set).
            spawn_claude_poller(app.handle().clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running quotabar");
}
