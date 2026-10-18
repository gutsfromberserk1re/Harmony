//! Data folder, log file and persistent settings.

use serde_json::{Map, Value};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_LOG_BYTES: u64 = 512 * 1024;

/// `%LOCALAPPDATA%\Harmony` (created on demand).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Harmony");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn log_path() -> PathBuf {
    data_dir().join("harmony.log")
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// Keep the log small: start over once it grows past the limit.
pub fn trim_log() {
    if fs::metadata(log_path()).map_or(false, |m| m.len() > MAX_LOG_BYTES) {
        let _ = fs::remove_file(log_path());
    }
}

pub fn log_error(message: &str) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(f, "[{secs}] {message}");
    }
}

/// Loads settings. On the very first run the settings of the old Python/Qt version
/// (stored in the registry) are imported so playlists and preferences carry over.
pub fn load() -> Value {
    if let Ok(text) = fs::read_to_string(settings_path()) {
        if let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(&text) {
            return value;
        }
    }
    import_legacy()
}

pub fn save(settings: &Value) -> Result<(), String> {
    let tmp = settings_path().with_extension("json.tmp");
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, settings_path()).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn import_legacy() -> Value {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    let mut out = Map::new();
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Kiyo_Kumo_Ltd\Harmony")
    else {
        return Value::Object(out);
    };
    let text = |name: &str| key.get_value::<String, _>(name).ok();
    let int = |name: &str| {
        text(name)
            .and_then(|s| s.trim().parse::<i64>().ok())
            .or_else(|| key.get_value::<u32, _>(name).ok().map(i64::from))
    };
    let boolean = |name: &str| text(name).map(|s| s.trim().eq_ignore_ascii_case("true"));

    if let Some(v) = text("theme_name") {
        out.insert("theme_name".into(), v.into());
    }
    if let Some(v) = int("bg_alpha") {
        out.insert("bg_alpha".into(), v.into());
    }
    if let Some(v) = text("volume").and_then(|s| s.trim().parse::<f64>().ok()) {
        out.insert("volume".into(), v.into());
    }
    for name in ["repeat", "shuffle", "discord_rpc"] {
        if let Some(v) = boolean(name) {
            out.insert(name.into(), v.into());
        }
    }
    if let Some(v) = text("playlists").and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        if v.is_object() {
            out.insert("playlists".into(), v);
        }
    }
    Value::Object(out)
}

#[cfg(not(windows))]
fn import_legacy() -> Value {
    Value::Object(Map::new())
}
