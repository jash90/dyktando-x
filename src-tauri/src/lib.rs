//! Dyktando X — wieloplatformowe dyktowanie po polsku (Tauri 2).
mod ai;
mod audio;
mod commands;
mod dictation;
mod engine;
mod focus;
mod hotkeys;
mod hud;
mod live_window;
mod meetings;
mod models;
mod paste;
mod paths;
mod postprocess;
mod settings;
mod tray;

use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use dictation::Dictation;
use models::{AssetId, ASSETS};
use settings::Settings;

pub struct AppState {
    pub settings: Mutex<Settings>,
    pub dictation: Dictation,
    pub recorder: meetings::recorder::Recorder,
    pub jobs: meetings::processing::Jobs,
    pub tray: Mutex<Option<tray::TrayItems>>,
    hotkeys: Mutex<Option<hotkeys::Hotkeys>>,
    downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

pub fn refresh_tray(app: &AppHandle) {
    tray::refresh(app);
}

fn settings_snapshot(app: &AppHandle) -> Settings {
    app.state::<AppState>().settings.lock().unwrap().clone()
}

fn on_hotkey(app: &AppHandle, action: hotkeys::Action) {
    let state = app.state::<AppState>();
    let settings = settings_snapshot(app);
    match action {
        hotkeys::Action::PushToTalkDown => state.dictation.start(app, &settings),
        hotkeys::Action::PushToTalkUp => state.dictation.stop(app, settings, false),
        hotkeys::Action::Cancel => state.dictation.stop(app, settings, true),
        hotkeys::Action::Toggle => state.dictation.toggle(app, &settings),
        hotkeys::Action::Meeting => commands::toggle_meeting_from(app),
    }
}

fn apply_hotkeys(app: &AppHandle) -> Vec<String> {
    let s = settings_snapshot(app);
    let config = hotkeys::Config {
        push_to_talk: s.shortcut_push_to_talk.clone(),
        toggle: s.shortcut_toggle.clone(),
        modifier: s.modifier_push_to_talk.clone(),
        meeting: s.shortcut_meeting.clone(),
    };
    let state = app.state::<AppState>();
    let mut slot = state.hotkeys.lock().unwrap();
    *slot = None; // najpierw zwolnij stare (blokowanie klawiszy, wątki)
    let handle = app.clone();
    let hk = hotkeys::Hotkeys::start(&config, move |a| on_hotkey(&handle, a));
    let warnings = hk.warnings.clone();
    for w in &warnings {
        log::warn!("{w}");
    }
    *slot = Some(hk);
    warnings
}

// MARK: - Komendy dla interfejsu

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<Vec<String>, String> {
    let state = app.state::<AppState>();
    let previous = state.settings.lock().unwrap().clone();
    settings.save().map_err(|e| e.to_string())?;
    *state.settings.lock().unwrap() = settings.clone();
    if settings.engine != previous.engine {
        state.dictation.preload(settings.engine);
    }
    let hotkeys_changed = settings.shortcut_push_to_talk != previous.shortcut_push_to_talk
        || settings.shortcut_toggle != previous.shortcut_toggle
        || settings.modifier_push_to_talk != previous.modifier_push_to_talk
        || settings.shortcut_meeting != previous.shortcut_meeting;
    if hotkeys_changed {
        return Ok(apply_hotkeys(&app));
    }
    let warnings = state.hotkeys.lock().unwrap().as_ref().map(|h| h.warnings.clone()).unwrap_or_default();
    Ok(warnings)
}

#[derive(Serialize)]
struct Devices {
    devices: Vec<String>,
    default: Option<String>,
}

#[tauri::command]
fn list_input_devices() -> Devices {
    Devices { devices: audio::capture::input_device_names(), default: audio::capture::default_input_name() }
}

#[derive(Serialize)]
struct ModelInfo {
    id: AssetId,
    key: String,
    title: &'static str,
    description: &'static str,
    size: u64,
    installed: bool,
    downloading: bool,
}

fn asset_key(id: AssetId) -> String {
    serde_json::to_string(&id).unwrap_or_default()
}

#[tauri::command]
fn list_models(state: tauri::State<AppState>) -> Vec<ModelInfo> {
    let downloads = state.downloads.lock().unwrap();
    ASSETS
        .iter()
        .map(|a| ModelInfo {
            id: a.id,
            key: asset_key(a.id),
            title: a.title,
            description: a.description,
            size: a.total_size(),
            installed: a.is_installed(),
            downloading: downloads.contains_key(&asset_key(a.id)),
        })
        .collect()
}

#[derive(Clone, Serialize)]
struct DownloadEvent {
    key: String,
    done: u64,
    total: u64,
    finished: bool,
    error: Option<String>,
}

#[tauri::command]
async fn download_model(app: AppHandle, id: AssetId) -> Result<(), String> {
    let key = asset_key(id);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let state = app.state::<AppState>();
        let mut d = state.downloads.lock().unwrap();
        if d.contains_key(&key) {
            return Ok(());
        }
        d.insert(key.clone(), cancel.clone());
    }
    let asset = models::asset(id);
    let mut last_emit = std::time::Instant::now();
    let result = asset
        .download(&cancel, |done, total| {
            if last_emit.elapsed().as_millis() > 150 || done == total {
                last_emit = std::time::Instant::now();
                let _ = app.emit("model-download", DownloadEvent { key: key.clone(), done, total, finished: false, error: None });
            }
        })
        .await;
    let state = app.state::<AppState>();
    state.downloads.lock().unwrap().remove(&key);
    let error = result.as_ref().err().map(|e| e.to_string());
    let _ = app.emit("model-download", DownloadEvent { key, done: 0, total: 0, finished: true, error: error.clone() });
    if error.is_none() {
        if let AssetId::Engine(e) = id {
            if e == settings_snapshot(&app).engine {
                state.dictation.preload(e);
            }
        }
    }
    error.map_or(Ok(()), Err)
}

#[tauri::command]
fn cancel_download(state: tauri::State<AppState>, id: AssetId) {
    if let Some(flag) = state.downloads.lock().unwrap().get(&asset_key(id)) {
        flag.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
fn delete_model(state: tauri::State<AppState>, id: AssetId) -> Result<(), String> {
    if let AssetId::Engine(_) = id {
        state.dictation.unload();
    }
    models::asset(id).remove().map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Environment {
    os: &'static str,
    wayland: bool,
    can_send_keys: bool,
    hotkey_warnings: Vec<String>,
    data_dir: String,
}

#[tauri::command]
fn environment(state: tauri::State<AppState>) -> Environment {
    let hotkey_warnings = state.hotkeys.lock().unwrap().as_ref().map(|h| h.warnings.clone()).unwrap_or_default();
    Environment {
        os: std::env::consts::OS,
        wayland: paste::is_wayland(),
        can_send_keys: paste::can_send_keys(),
        hotkey_warnings,
        data_dir: paths::support().display().to_string(),
    }
}

#[tauri::command]
fn open_accessibility_settings(app: AppHandle) -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    handy_keys::open_accessibility_settings().map_err(|e| e.to_string())?;
    Ok(apply_hotkeys(&app))
}

/// Wyłącza skróty na czas nagrywania nowego skrótu w ustawieniach (inaczej F5 by nie dotarł
/// do okna, bo blokujemy go globalnie). `reload_hotkeys` włącza je z powrotem.
#[tauri::command]
fn pause_hotkeys(state: tauri::State<AppState>) {
    *state.hotkeys.lock().unwrap() = None;
}

#[tauri::command]
fn js_error(message: String, location: String) {
    log::error!("JS ({location}): {message}");
}

#[tauri::command]
fn open_meetings(app: AppHandle) {
    show_window(&app, Window::Meetings);
}

/// Wątki w tle: licznik nagrywania w pasku (co sekundę), odzyskiwanie spotkań po awarii
/// i retencja audio (przy starcie i raz na dobę).
fn start_background(app: &AppHandle) {
    let store = meetings::store::Store::default();
    let recovered = store.recover();
    if !recovered.is_empty() {
        log::warn!("Odzyskane spotkania po przerwanym nagrywaniu: {recovered:?}");
    }
    // Wykrywanie spotkań (co 5 s, gdy włączone w ustawieniach).
    let detector_app = app.clone();
    std::thread::spawn(move || {
        let mut logic = meetings::detector::DetectionLogic::default();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let state = detector_app.state::<AppState>();
            let enabled = state.settings.lock().unwrap().meeting_detection_prompt;
            let active = if enabled { meetings::detector::processes_using_microphone() } else { Vec::new() };
            if let Some(name) = logic.update(&active, state.recorder.is_recording(), enabled) {
                log::info!("Wykryto spotkanie: {name}");
                let handle = detector_app.clone();
                let name = name.to_string();
                let _ = detector_app.run_on_main_thread(move || commands::show_meeting_prompt(&handle, &name));
            }
        }
    });
    // Status nagrania 4×/s (poziomy w oknie na żywo); pasek odświeżany tylko, gdy zmieni się sekunda.
    let handle = app.clone();
    std::thread::spawn(move || {
        let mut last_retention = std::time::Instant::now() - std::time::Duration::from_secs(86_400);
        let mut was_recording = false;
        let mut last_second = u64::MAX;
        loop {
            let state = handle.state::<AppState>();
            let status = state.recorder.status();
            if status.recording || was_recording {
                let second = if status.recording { status.seconds as u64 } else { u64::MAX };
                if second != last_second || status.recording != was_recording {
                    tray::refresh(&handle);
                }
                last_second = second;
                let _ = handle.emit("meeting-status", &status);
            }
            was_recording = status.recording;
            if last_retention.elapsed().as_secs() >= 86_400 {
                let days = state.settings.lock().unwrap().meeting_audio_retention_days;
                let n = store.apply_retention(days, chrono::Local::now());
                if n > 0 {
                    log::info!("Retencja: usunięto audio {n} spotkań starszych niż {days} dni");
                }
                last_retention = std::time::Instant::now();
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
}

#[tauri::command]
fn autostart_enabled(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let al = app.autolaunch();
    if enabled { al.enable() } else { al.disable() }.map_err(|e| e.to_string())
}

/// Ponowna rejestracja skrótów — np. po nadaniu uprawnień.
#[tauri::command]
fn reload_hotkeys(app: AppHandle) -> Vec<String> {
    apply_hotkeys(&app)
}

#[derive(Clone, Copy)]
pub enum Window {
    Settings,
    Meetings,
}

/// Okna powstają dopiero przy pierwszym otwarciu (a potem są tylko chowane): ukryte okno
/// utworzone na starcie nie ma sensu, a zjada pamięć procesu WebKit/WebView2.
pub fn show_window(app: &AppHandle, which: Window) {
    let (label, url, title, size, min) = match which {
        Window::Settings => ("main", "index.html", "Dyktando X — Ustawienia", (860.0, 620.0), (720.0, 480.0)),
        Window::Meetings => ("meetings", "index.html#meetings", "Dyktando X — Spotkania", (1040.0, 700.0), (760.0, 480.0)),
    };
    let window = match app.get_webview_window(label) {
        Some(w) => w,
        None => match tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App(url.into()))
            .title(title)
            .inner_size(size.0, size.1)
            .min_inner_size(min.0, min.1)
            .center()
            .build()
        {
            Ok(w) => w,
            Err(e) => {
                log::error!("Okno {label}: {e}");
                return;
            }
        },
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

fn show_settings(app: &AppHandle) {
    show_window(app, Window::Settings);
}

fn autostart_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let builder = tauri_plugin_autostart::Builder::new();
    // `macos_launcher` istnieje tylko w kompilacji na macOS.
    #[cfg(target_os = "macos")]
    let builder = builder.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);
    builder.build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,ort=warn,whisper_rs=warn,transcribe_rs=warn")).try_init();
    let settings = Settings::load();
    tauri::Builder::default()
        // Ponowne uruchomienie (dwuklik w Finderze / menu Start) otwiera ustawienia działającej kopii.
        // `--meetings` otwiera okno spotkań (np. ze skrótu systemowego).
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.iter().any(|a| a == "--meetings") {
                show_window(app, Window::Meetings);
            } else {
                show_settings(app);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        // Autostart: macOS przez LaunchAgent (bez zgody „Elementy logowania” dla każdej wersji),
        // Windows przez rejestr Run, Linux przez ~/.config/autostart.
        .plugin(autostart_plugin())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            settings: Mutex::new(settings),
            dictation: Dictation::default(),
            recorder: Default::default(),
            jobs: Default::default(),
            tray: Mutex::new(None),
            hotkeys: Mutex::new(None),
            downloads: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            list_input_devices,
            list_models,
            download_model,
            cancel_download,
            delete_model,
            environment,
            open_accessibility_settings,
            reload_hotkeys,
            pause_hotkeys,
            js_error,
            commands::meeting_status,
            commands::live_transcript,
            commands::hide_live_window,
            commands::start_meeting,
            commands::stop_meeting,
            commands::list_meetings,
            commands::get_meeting,
            commands::rename_meeting,
            commands::delete_meeting,
            commands::reveal_meeting,
            commands::transcribe_meeting,
            commands::summarize_meeting,
            commands::cancel_job,
            commands::job_status,
            commands::ai_providers,
            commands::set_ai_key,
            commands::test_ai,
            commands::default_summary_prompt,
            commands::ai_key_status,
            commands::import_legacy_keys,
            commands::prompt_answer,
            autostart_enabled,
            set_autostart,
            open_meetings,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle().clone();
            let items = tray::build(&handle)?;
            *handle.state::<AppState>().tray.lock().unwrap() = Some(items);
            tray::refresh(&handle);
            start_background(&handle);
            hud::create(&handle)?;
            apply_hotkeys(&handle);
            let s = settings_snapshot(&handle);
            handle.state::<AppState>().dictation.preload(s.engine);
            if std::env::args().any(|a| a == "--meetings") {
                show_window(&handle, Window::Meetings);
            }
            // Pierwsze uruchomienie (brak modelu) — od razu pokaż ustawienia.
            if !s.engine.asset().is_installed() {
                show_settings(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Zamknięcie okna ustawień tylko je chowa — aplikacja żyje w zasobniku.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" || window.label() == "meetings" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("błąd uruchamiania Dyktando X")
        .run(|app, event| {
            // macOS: dwuklik w Finderze / Spotlight na działającej aplikacji nie startuje drugiej
            // kopii, tylko wysyła „reopen” — wtedy pokazujemy ustawienia.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_settings(app);
            }
            let _ = (app, event);
        });
}
