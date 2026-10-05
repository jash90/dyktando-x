//! Katalogi aplikacji: dane (modele, spotkania) i ustawienia.
use std::path::PathBuf;

pub const APP_DIR: &str = "DyktandoX";

/// Np. macOS `~/Library/Application Support/DyktandoX`, Windows `%APPDATA%\DyktandoX`,
/// Linux `~/.local/share/DyktandoX`. Zmienna `DYKTANDO_X_HOME` nadpisuje (testy).
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

pub fn settings_file() -> PathBuf {
    support().join("settings.json")
}
