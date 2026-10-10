//! App directories: data (models, meetings) and settings.
use std::path::PathBuf;

pub const APP_DIR: &str = "DyktandoX";

/// E.g. macOS `~/Library/Application Support/DyktandoX`, Windows `%APPDATA%\DyktandoX`,
/// Linux `~/.local/share/DyktandoX`. The `DYKTANDO_X_HOME` variable overrides it (tests).
pub fn support() -> PathBuf {
    if let Ok(p) = std::env::var("DYKTANDO_X_HOME") {
        return PathBuf::from(p);
    }
    dirs::data_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join(APP_DIR)
}

pub fn models() -> PathBuf {
    support().join("models")
}

pub fn meetings() -> PathBuf {
    support().join("Meetings")
}

pub fn dictations() -> PathBuf {
    support().join("Dictations")
}

/// Logs: macOS `~/Library/Logs/Dyktando X`, elsewhere `<data>/logs`.
pub fn logs() -> PathBuf {
    if std::env::var_os("DYKTANDO_X_HOME").is_none() && cfg!(target_os = "macos") {
        if let Some(home) = dirs::home_dir() {
            return home.join("Library/Logs/Dyktando X");
        }
    }
    support().join("logs")
}

pub fn settings_file() -> PathBuf {
    support().join("settings.json")
}
