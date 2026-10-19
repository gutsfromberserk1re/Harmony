#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod presence;
mod store;
mod window;
mod ytdlp;

use presence::Presence;
use serde::Serialize;
use serde_json::Value;
use tauri::{Manager, State, WindowEvent};

/// Discord "Application ID" (discord.com/developers/applications). "" = Discord status off.
const DISCORD_CLIENT_ID: &str = "";
/// Name of the icon you upload under Rich Presence > Art Assets.
const DISCORD_ICON_KEY: &str = "harmony";

struct AppState {
    presence: Presence,
}

#[derive(Serialize)]
struct AppConfig {
    presence_enabled: bool,
    discord_icon_key: &'static str,
}

#[tauri::command]
fn app_config(state: State<AppState>) -> AppConfig {
    AppConfig {
        presence_enabled: state.presence.enabled(),
        discord_icon_key: DISCORD_ICON_KEY,
    }
}

#[tauri::command]
fn load_settings() -> Value {
    store::load()
}

#[tauri::command]
fn save_settings(settings: Value) -> Result<(), String> {
    store::save(&settings)
}

#[tauri::command]
fn set_presence(state: State<AppState>, activity: Option<Value>) {
    state.presence.set(activity);
}

fn main() {
    store::trim_log();

    tauri::Builder::default()
        .manage(AppState {
            presence: Presence::new(DISCORD_CLIENT_ID),
        })
        .invoke_handler(tauri::generate_handler![
            app_config,
            load_settings,
            save_settings,
            set_presence,
            ytdlp::fetch_playlist,
            ytdlp::fetch_stream,
            ytdlp::cookies_status,
            ytdlp::save_cookies,
            ytdlp::clear_cookies,
        ])
        .setup(|app| {
            if let Some(main_window) = app.get_webview_window("main") {
                window::apply_effects(&main_window);
            }
            ytdlp::prepare_cookies();
            tauri::async_runtime::spawn(ytdlp::prepare_and_update());
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::CloseRequested { .. }) {
                window.state::<AppState>().presence.stop();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Harmony");
}
