//! Komendy interfejsu dla spotkań i AI.
use serde::Serialize;
use std::collections::BTreeMap;
use tauri::{AppHandle, Emitter, Manager};

use crate::ai::{keys, provider::LlmConfig, summarizer};
use crate::dictation::{emit as hud_emit, HudState};
use crate::meetings::live;
use crate::meetings::processing::{self, JobEvent};
use crate::meetings::recorder::RecordingStatus;
use crate::meetings::transcript::Utterance;
use crate::meetings::store::{read_to_string, Meeting, Store};
use crate::models::EngineId;
use crate::settings::ProviderId;
use crate::AppState;

fn store() -> Store {
    Store::default()
}

fn changed(app: &AppHandle) {
    let _ = app.emit("meetings-changed", ());
    crate::refresh_tray(app);
}

/// Zdarzenie transkrypcji na żywo dla interfejsu (`meeting-live`).
#[derive(Clone, Serialize)]
pub struct LivePayload {
    meeting_id: String,
    utterance: Option<Utterance>,
    /// Tekst roboczy trwającej wypowiedzi (zastępuje poprzedni roboczy tej ścieżki; pusty = usuń).
    partial: Option<Utterance>,
    error: Option<String>,
}

pub fn start_meeting_inner(app: &AppHandle) -> Result<Meeting, String> {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    let live = settings.meeting_live_transcription.then(|| {
        let app = app.clone();
        let translate_to = Some(settings.meeting_live_translate_to.trim().to_string()).filter(|t| !t.is_empty());
        let config = live::Config { engine: settings.meeting_engine, language: settings.meeting_language, translate_to };
        let listener: crate::meetings::recorder::LiveListener = Box::new(move |id: &str, e: live::Event| {
            let (utterance, partial, error) = match e {
                live::Event::Utterance(u) => (Some(u), None, None),
                live::Event::Partial(u) => (None, Some(u), None),
                live::Event::Error(m) => (None, None, Some(m)),
            };
            let _ = app.emit("meeting-live", LivePayload { meeting_id: id.to_string(), utterance, partial, error });
        });
        (config, listener)
    });
    let meeting = state.recorder.start(&store(), settings.input_device.as_deref(), live).map_err(|e| e.to_string())?;
    if settings.meeting_live_window {
        crate::live_window::show(app);
    }
    if settings.meeting_consent_reminder {
        hud_emit(app, HudState::Info { message: "Nagrywam spotkanie — pamiętaj, żeby poinformować rozmówców.".into() });
    }
    changed(app);
    Ok(meeting)
}

pub fn stop_meeting_inner(app: &AppHandle) -> Result<Meeting, String> {
    let state = app.state::<AppState>();
    let meeting = state.recorder.stop(&store()).map_err(|e| e.to_string())?;
    crate::live_window::hide(app);
    hud_emit(app, HudState::Info { message: format!("Nagranie zapisane ({})", crate::meetings::transcript::clock(meeting.duration_seconds)) });
    changed(app);
    processing::after_recording(app.clone(), meeting.id.clone());
    Ok(meeting)
}

/// Z menu i ze skrótu: start albo stop, błędy w dymku.
pub fn toggle_meeting_from(app: &AppHandle) {
    let recording = app.state::<AppState>().recorder.is_recording();
    let r = if recording { stop_meeting_inner(app) } else { start_meeting_inner(app) };
    if let Err(e) = r {
        hud_emit(app, HudState::Error { message: e });
    }
}

#[tauri::command]
pub fn meeting_status(state: tauri::State<AppState>) -> RecordingStatus {
    state.recorder.status()
}

/// Wypowiedzi przepisane na żywo w trwającym nagraniu (okno otwarte w trakcie spotkania).
#[tauri::command]
pub fn live_transcript(state: tauri::State<AppState>) -> Vec<Utterance> {
    state.recorder.live_utterances()
}

/// Zamknięcie okna „na żywo” przez użytkownika (nagranie trwa dalej; okno wróci przy następnym).
#[tauri::command]
pub fn hide_live_window(app: AppHandle) {
    crate::live_window::hide(&app);
}

#[tauri::command]
pub fn start_meeting(app: AppHandle) -> Result<Meeting, String> {
    start_meeting_inner(&app)
}

#[tauri::command]
pub fn stop_meeting(app: AppHandle) -> Result<Meeting, String> {
    stop_meeting_inner(&app)
}

#[tauri::command]
pub fn list_meetings() -> Vec<Meeting> {
    store().all()
}

#[derive(Serialize)]
pub struct SummaryFile {
    name: String,
    content: String,
}

#[derive(Serialize)]
pub struct MeetingDetail {
    meeting: Meeting,
    transcript: Option<String>,
    summaries: Vec<SummaryFile>,
    folder: String,
}

#[tauri::command]
pub fn get_meeting(id: String) -> Result<MeetingDetail, String> {
    let s = store();
    let meeting = s.load(&id).ok_or("Brak spotkania")?;
    let summaries = s
        .summaries(&id)
        .into_iter()
        .filter_map(|p| Some(SummaryFile { name: p.file_name()?.to_string_lossy().into(), content: read_to_string(&p)? }))
        .collect();
    Ok(MeetingDetail { transcript: read_to_string(&s.transcript_md(&id)), summaries, folder: s.folder(&id).display().to_string(), meeting })
}

#[tauri::command]
pub fn rename_meeting(app: AppHandle, id: String, title: String) -> Result<(), String> {
    let t = title.trim().to_string();
    store().update(&id, |m| m.title = (!t.is_empty()).then_some(t)).map_err(|e| e.to_string())?;
    changed(&app);
    Ok(())
}

#[tauri::command]
pub fn delete_meeting(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.recorder.status().meeting_id.as_deref() == Some(id.as_str()) {
        return Err("Najpierw zatrzymaj nagrywanie".into());
    }
    if state.jobs.current().is_some_and(|j| j.meeting_id == id) {
        return Err("To spotkanie jest właśnie przetwarzane".into());
    }
    store().delete(&id).map_err(|e| e.to_string())?;
    changed(&app);
    Ok(())
}

#[tauri::command]
pub fn reveal_meeting(app: AppHandle, id: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let s = store();
    let target = if s.transcript_md(&id).exists() { s.transcript_md(&id) } else { s.folder(&id) };
    app.opener().reveal_item_in_dir(target).map_err(|e| e.to_string())
}

/// Okno wyboru pliku z nagraniem rozmowy → nowe spotkanie (wczytanie i transkrypcja w tle).
/// `None` = użytkownik zamknął okno bez wyboru.
#[tauri::command]
pub async fn import_meeting(app: AppHandle) -> Result<Option<Meeting>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title("Wybierz nagranie rozmowy")
        .add_filter("Nagrania audio", crate::meetings::import::EXTENSIONS)
        .blocking_pick_file();
    let Some(file) = picked else { return Ok(None) };
    let path = file.into_path().map_err(|e| e.to_string())?;
    let meeting = processing::import_file(app.clone(), path).map_err(|e| e.to_string())?;
    changed(&app);
    Ok(Some(meeting))
}

#[tauri::command]
pub async fn transcribe_meeting(app: AppHandle, id: String, engine: Option<EngineId>) -> Result<(), String> {
    processing::transcribe(app, id, engine).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn summarize_meeting(app: AppHandle, id: String, provider: ProviderId, model: Option<String>) -> Result<(), String> {
    processing::summarize(app, id, provider, model).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cancel_job(state: tauri::State<AppState>) {
    state.jobs.cancel();
}

#[tauri::command]
pub fn job_status(state: tauri::State<AppState>) -> Option<JobEvent> {
    state.jobs.current()
}

// MARK: - AI

#[derive(Serialize)]
pub struct ProviderInfo {
    id: ProviderId,
    name: &'static str,
    has_key: bool,
    default_base_url: &'static str,
    default_model: &'static str,
    model_placeholder: &'static str,
}

#[tauri::command]
pub fn ai_providers() -> Vec<ProviderInfo> {
    ProviderId::ALL
        .iter()
        .map(|&id| ProviderInfo {
            id,
            name: id.display_name(),
            has_key: keys::has(id),
            default_base_url: id.default_base_url(),
            default_model: id.default_model(),
            model_placeholder: id.model_placeholder(),
        })
        .collect()
}

/// Zapis klucza w systemowym magazynie (pusty = usunięcie). Klucz nigdy nie wraca do interfejsu.
#[tauri::command]
pub fn set_ai_key(provider: ProviderId, key: String) -> Result<(), String> {
    keys::set(provider, key.trim()).map_err(|e| e.to_string())
}

/// „Testuj połączenie”: lista modeli dostawcy (sprawdza klucz i adres).
#[tauri::command]
pub async fn test_ai(app: AppHandle, provider: ProviderId) -> Result<Vec<String>, String> {
    let settings = app.state::<AppState>().settings.lock().unwrap().clone();
    let key = keys::get(provider).ok_or_else(|| format!("Brak klucza API dla {}", provider.display_name()))?;
    let (model, base_url) = settings.provider(provider);
    let config = LlmConfig { provider, api_key: key, model, base_url };
    config.list_models().await.map_err(|e| e.message)
}

#[tauri::command]
pub fn default_summary_prompt() -> &'static str {
    summarizer::DEFAULT_PROMPT
}

#[tauri::command]
pub fn ai_key_status() -> BTreeMap<ProviderId, bool> {
    ProviderId::ALL.iter().map(|&id| (id, keys::has(id))).collect()
}

/// Import kluczy z Dyktando dla macOS (wersja Swift).
#[tauri::command]
pub fn import_legacy_keys() -> Vec<&'static str> {
    keys::import_legacy()
}

// MARK: - Podpowiedź „Wykryto spotkanie”

pub const PROMPT_LABEL: &str = "prompt";

/// Małe okno w prawym górnym rogu; nie przejmuje fokusu (rozmowa trwa w innej aplikacji).
pub fn show_meeting_prompt(app: &AppHandle, app_name: &str) {
    if let Some(w) = app.get_webview_window(PROMPT_LABEL) {
        let _ = w.close();
    }
    let url = format!("index.html#prompt?app={}", app_name.replace(' ', "%20"));
    let built = tauri::WebviewWindowBuilder::new(app, PROMPT_LABEL, tauri::WebviewUrl::App(url.into()))
        .title("Dyktando X")
        .inner_size(380.0, 112.0)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .focused(false)
        .visible(false)
        .build();
    let Ok(w) = built else { return };
    if let Ok(Some(m)) = w.primary_monitor() {
        let scale = m.scale_factor();
        let x = m.position().x + m.size().width as i32 - ((380.0 + 20.0) * scale) as i32;
        let y = m.position().y + (40.0 * scale) as i32;
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = w.show();
    // Bez odpowiedzi podpowiedź znika po 30 s.
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(30));
        if let Some(w) = app.get_webview_window(PROMPT_LABEL) {
            let _ = w.close();
        }
    });
}

#[tauri::command]
pub fn prompt_answer(app: AppHandle, record: bool) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(PROMPT_LABEL) {
        let _ = w.close();
    }
    if record && !app.state::<AppState>().recorder.is_recording() {
        start_meeting_inner(&app)?;
    }
    Ok(())
}
