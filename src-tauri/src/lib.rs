use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{
    include_image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, PhysicalPosition, WebviewWindow,
};

// --- Config persistence -----------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct AppConfig {
    claude_session_key: Option<String>,
}

struct ConfigState(Mutex<AppConfig>);

fn config_path() -> PathBuf {
    // ~/Library/Application Support/QuotaBar/config.json on macOS
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
    // Center popover horizontally under the tray icon.
    // tray_pos is in PHYSICAL pixels (CGFloat from macOS event), so scale by monitor factor.
    let (win_w, _win_h) = match window.outer_size() {
        Ok(s) => (s.width as f64, s.height as f64),
        Err(_) => (320.0, 240.0),
    };

    // Tray icon is at the top of the screen; pop the window just below it.
    let x = tray_pos.x - (win_w / 2.0);
    let y = tray_pos.y + 8.0;

    let _ = window.set_position(PhysicalPosition::new(x.round(), y.round()));
}

// --- Run --------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let initial_cfg = load_config();
    let state = ConfigState(Mutex::new(initial_cfg));

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![get_config, set_claude_session_key])
        .setup(|app| {
            // Hide from Dock on macOS — pure menubar app.
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            // Build a tiny default tray menu (right-click only). The main
            // popover is shown on LEFT-click via TrayIconEvent::Click.
            let quit_item = MenuItem::with_id(app, "quit", "Quit QuotaBar", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit_item])?;

            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(include_image!("./icons/icon.png"))
                .icon_as_template(true)
                .title("Q") // text shown next to icon; we'll replace with % later
                .menu(&menu)
                .show_menu_on_left_click(false) // do NOT show NSMenu on left click
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

            // Auto-hide popover when it loses focus (popover semantics).
            if let Some(popover) = app.get_webview_window("popover") {
                let popover_clone = popover.clone();
                popover.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        let _ = popover_clone.hide();
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running quotabar");
}
