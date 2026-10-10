//! Updates from GitHub Releases (`latest.json` signed with a minisign key, see `tauri.conf.json`).
//! Checks on demand (Settings → System, tray menu) and one silent check at startup, which
//! only changes the tray item — installing always requires a click.
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::AppState;

#[derive(Default)]
pub struct Updates {
    available: Mutex<Option<Update>>,
    installing: AtomicBool,
}

impl Updates {
    /// Version of the found (not yet installed) update.
    pub fn available_version(&self) -> Option<String> {
        self.available.lock().unwrap().as_ref().map(|u| u.version.clone())
    }
}

#[derive(Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

#[derive(Clone, Serialize)]
struct UpdateProgress {
    done: u64,
    total: u64,
    finished: bool,
    error: Option<String>,
}

fn info(u: &Update) -> UpdateInfo {
    UpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: u.body.clone().filter(|b| !b.trim().is_empty()),
        date: u.date.map(|d| d.date().to_string()),
    }
}

/// Checks the server, stores and broadcasts the result (`update-status`) — the System panel listens,
/// so it also sees the result of a check started from the tray or at startup.
async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let found = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let info = found.as_ref().map(info);
    *app.state::<Updates>().available.lock().unwrap() = found;
    crate::refresh_tray(app);
    let _ = app.emit("update-status", &info);
    Ok(info)
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    check(&app).await
}

/// The most recently found update, without asking the server.
#[tauri::command]
pub fn known_update(updates: tauri::State<Updates>) -> Option<UpdateInfo> {
    updates.available.lock().unwrap().as_ref().map(info)
}

/// Installing closes or restarts the app — it must not be done during a meeting.
fn busy(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.recorder.status().recording {
        return Err(crate::i18n::t("update.busy_recording"));
    }
    if state.jobs.current().is_some() {
        return Err(crate::i18n::t("update.busy_processing"));
    }
    Ok(())
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    busy(&app)?;
    let updates = app.state::<Updates>();
    let Some(update) = updates.available.lock().unwrap().clone() else {
        return Err(crate::i18n::t("update.none"));
    };
    if updates.installing.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    log::info!("Update {} → {}", update.current_version, update.version);
    let mut done = 0u64;
    let mut last_emit = std::time::Instant::now();
    let downloaded = update
        .download(
            |chunk, total| {
                done += chunk as u64;
                if last_emit.elapsed().as_millis() > 150 {
                    last_emit = std::time::Instant::now();
                    let _ = app.emit("update-progress", UpdateProgress { done, total: total.unwrap_or(0), finished: false, error: None });
                }
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string());
    // A meeting may have started during the download — we check again right before installing.
    let result = downloaded.and_then(|bytes| {
        busy(&app)?;
        update.install(bytes).map_err(|e| e.to_string())
    });
    updates.installing.store(false, Ordering::SeqCst);
    let error = result.err();
    let _ = app.emit("update-progress", UpdateProgress { done, total: done, finished: true, error: error.clone() });
    if let Some(e) = error {
        log::error!("Update failed: {e}");
        return Err(e);
    }
    // Windows: the NSIS installer closes the app itself; on macOS and Linux we start the new version.
    app.restart();
}

/// Silent check shortly after startup (doesn't compete with model loading); errors only to the log.
pub fn check_on_startup(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        match check(&app).await {
            Ok(Some(u)) => log::info!("Update available: {}", u.version),
            Ok(None) => {}
            Err(e) => log::info!("Update check: {e}"),
        }
    });
}

/// Tray item: shows settings on the System panel; without a known update it checks first.
pub fn open_from_tray(app: &AppHandle) {
    crate::show_window(app, crate::Window::SettingsPane("system"));
    if app.state::<Updates>().available_version().is_none() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = check(&app).await {
                log::info!("Update check: {e}");
                // System panel already open — it shows the error instead of staying silent.
                let _ = app.emit("update-check-failed", e);
            }
        });
    }
}
