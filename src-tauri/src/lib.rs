mod claude;
mod claude_local;
mod codex;
mod codex_local;

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

#[derive(Default)]
struct UsageCache {
    claude: Arc<Mutex<ClaudeSnapshot>>,
    claude_local: Arc<Mutex<Option<claude_local::ClaudeTokenSummary>>>,
    codex: Arc<Mutex<CodexSnapshot>>,
    codex_local: Arc<Mutex<Option<codex_local::CodexLocalSummary>>>,
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

fn update_tray_title(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("main-tray") else { return };
    let snap = app.state::<UsageCache>().claude.lock().unwrap().clone();
    let title = match &snap {
        ClaudeSnapshot::Ok(data) | ClaudeSnapshot::Stale { data, .. } => data
            .five_hour
            .as_ref()
            .map(|w| format!("C {:.0}%", w.percent))
            .unwrap_or_else(|| "C —".into()),
        ClaudeSnapshot::Loading => "C …".into(),
        ClaudeSnapshot::Error { .. } => "C !".into(),
        ClaudeSnapshot::Idle => "Q".into(),
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
    let _ = app.emit("codex-snapshot-updated", ());
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
            get_claude_snapshot,
            get_claude_local_summary,
            get_codex_snapshot,
            get_codex_local_summary,
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
