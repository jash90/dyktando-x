//! Aktualizacje z GitHub Releases (`latest.json` podpisany kluczem minisign, patrz `tauri.conf.json`).
//! Sprawdzanie na żądanie (Ustawienia → System, menu traya) i jedno ciche przy starcie, które
//! tylko zmienia pozycję w trayu — instalacja zawsze wymaga kliknięcia.
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
    /// Wersja znalezionej (jeszcze niezainstalowanej) aktualizacji.
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

/// Sprawdza serwer, zapamiętuje wynik i ogłasza go (`update-status`) — panel System słucha,
/// więc widzi też wynik sprawdzenia uruchomionego z traya albo przy starcie.
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

/// Ostatnio znaleziona aktualizacja, bez pytania serwera.
#[tauri::command]
pub fn known_update(updates: tauri::State<Updates>) -> Option<UpdateInfo> {
    updates.available.lock().unwrap().as_ref().map(info)
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        if state.recorder.status().recording {
            return Err("Trwa nagrywanie spotkania — zatrzymaj je przed aktualizacją.".into());
        }
        if state.jobs.current().is_some() {
            return Err("Trwa przetwarzanie spotkania — poczekaj na koniec albo je przerwij.".into());
        }
    }
    let updates = app.state::<Updates>();
    let Some(update) = updates.available.lock().unwrap().clone() else {
        return Err("Brak aktualizacji do zainstalowania — sprawdź ponownie.".into());
    };
    if updates.installing.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    log::info!("Aktualizacja {} → {}", update.current_version, update.version);
    let mut done = 0u64;
    let mut last_emit = std::time::Instant::now();
    let result = update
        .download_and_install(
            |chunk, total| {
                done += chunk as u64;
                if last_emit.elapsed().as_millis() > 150 {
                    last_emit = std::time::Instant::now();
                    let _ = app.emit("update-progress", UpdateProgress { done, total: total.unwrap_or(0), finished: false, error: None });
                }
            },
            || {},
        )
        .await;
    updates.installing.store(false, Ordering::SeqCst);
    let error = result.err().map(|e| e.to_string());
    let _ = app.emit("update-progress", UpdateProgress { done, total: done, finished: true, error: error.clone() });
    if let Some(e) = error {
        log::error!("Aktualizacja nieudana: {e}");
        return Err(e);
    }
    // Windows: instalator NSIS sam zamyka aplikację; na macOS i Linuksie uruchamiamy nową wersję.
    app.restart();
}

/// Ciche sprawdzenie chwilę po starcie (nie konkuruje z wczytywaniem modelu); błędy tylko do logu.
pub fn check_on_startup(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        match check(&app).await {
            Ok(Some(u)) => log::info!("Dostępna aktualizacja {}", u.version),
            Ok(None) => {}
            Err(e) => log::info!("Sprawdzanie aktualizacji: {e}"),
        }
    });
}

/// Pozycja w trayu: pokazuje ustawienia na panelu System; bez znanej aktualizacji najpierw sprawdza.
pub fn open_from_tray(app: &AppHandle) {
    crate::show_window(app, crate::Window::SettingsPane("system"));
    if app.state::<Updates>().available_version().is_none() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = check(&app).await {
                log::info!("Sprawdzanie aktualizacji: {e}");
            }
        });
    }
}
