//! Meeting jobs (one at a time): transcription and AI summary, with progress sent to the
//! UI (`meeting-job`) and cancellation. After recording stops — an automatic chain
//! transcription → (optionally) summary, according to the settings.
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

use super::import;
use super::store::{Meeting, State, Store};
use super::transcriber::{self, Options};
use crate::ai::{keys, provider::LlmConfig, summarizer};
use crate::i18n;
use crate::models::{self, AssetId, EngineId};
use crate::settings::ProviderId;
use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Import,
    Transcribe,
    Summarize,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobEvent {
    pub meeting_id: String,
    pub kind: JobKind,
    pub step: String,
    pub fraction: f32,
    pub finished: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Jobs {
    current: Mutex<Option<(String, JobKind, Arc<AtomicBool>)>>,
    last: Mutex<Option<JobEvent>>,
}

impl Jobs {
    pub fn current(&self) -> Option<JobEvent> {
        self.last.lock().unwrap().clone().filter(|e| !e.finished)
    }

    pub fn cancel(&self) {
        if let Some((_, _, flag)) = self.current.lock().unwrap().as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    fn begin(&self, id: &str, kind: JobKind) -> Result<Arc<AtomicBool>> {
        let mut cur = self.current.lock().unwrap();
        if let Some((other, _, _)) = cur.as_ref() {
            return Err(anyhow!(i18n::t_with("job.busy", &[("id", other)])));
        }
        let flag = Arc::new(AtomicBool::new(false));
        *cur = Some((id.to_string(), kind, flag.clone()));
        Ok(flag)
    }

    fn end(&self) {
        *self.current.lock().unwrap() = None;
    }
}

fn emit(app: &AppHandle, ev: JobEvent) {
    *app.state::<AppState>().jobs.last.lock().unwrap() = Some(ev.clone());
    let _ = app.emit("meeting-job", &ev);
    crate::refresh_tray(app);
}

fn event(id: &str, kind: JobKind, step: impl Into<String>, fraction: f32) -> JobEvent {
    JobEvent { meeting_id: id.into(), kind, step: step.into(), fraction, finished: false, error: None }
}

/// Downloads missing models needed for transcription: VAD (2 MB), the speaker model (27 MB) and the
/// engine itself (e.g. Whisper set as default but not downloaded yet — 1.6 GB). The download is
/// visible (and cancellable) in Settings → Models as well; "Cancel" on the meeting cancels it
/// too. If the same model is already downloading from Settings, we wait for it to finish.
async fn ensure_support_models(app: &AppHandle, id: &str, diarize: bool, engine: EngineId, cancel: &Arc<AtomicBool>) -> Result<()> {
    let mut needed = vec![models::asset(AssetId::SileroVad)];
    if diarize {
        needed.push(models::asset(AssetId::SpeakerModel));
    }
    needed.push(engine.asset());
    let state = app.state::<AppState>();
    for asset in needed {
        let key = crate::asset_key(asset.id);
        loop {
            if asset.is_installed() || cancel.load(Ordering::Relaxed) {
                break;
            }
            let ours = {
                let mut d = state.downloads.lock().unwrap();
                if d.contains_key(&key) {
                    false
                } else {
                    d.insert(key.clone(), cancel.clone());
                    true
                }
            };
            if !ours {
                emit(app, event(id, JobKind::Transcribe, i18n::t_with("job.waiting_for_download", &[("model", &asset.label())]), 0.0));
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            let (app2, title, key2) = (app.clone(), asset.label(), key.clone());
            let mut last_percent = u32::MAX;
            let result = asset
                .download(cancel, move |done, total| {
                    let f = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                    let percent = (f * 100.0) as u32;
                    if percent != last_percent {
                        last_percent = percent;
                        emit(&app2, event(id, JobKind::Transcribe, i18n::t_with("job.downloading", &[("model", &title), ("percent", &percent)]), f * 0.05));
                        // The same progress in Settings → Models.
                        let _ = app2.emit("model-download", crate::DownloadEvent { key: key2.clone(), done, total, finished: false, error: None });
                    }
                })
                .await;
            state.downloads.lock().unwrap().remove(&key);
            let error = result.as_ref().err().map(|e| e.to_string());
            let _ = app.emit("model-download", crate::DownloadEvent { key: key.clone(), done: 0, total: 0, finished: true, error });
            result?;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!(super::CANCELLED));
        }
    }
    Ok(())
}

/// `languages: None` = meeting language from settings (see `transcriber::Options::languages`).
pub async fn transcribe(app: AppHandle, id: String, engine: Option<EngineId>, languages: Option<Vec<String>>) -> Result<()> {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    let cancel = state.jobs.begin(&id, JobKind::Transcribe)?;
    let store = Store::default();
    let result: Result<()> = async {
        let previous = store.load(&id).ok_or_else(|| anyhow!(i18n::t("meeting.not_found")))?.state;
        store.update(&id, |m| {
            m.state = State::Transcribing;
            m.last_error = None;
        })?;
        emit(&app, event(&id, JobKind::Transcribe, i18n::t("job.preparing"), 0.0));
        let engine = engine.unwrap_or(settings.meeting_engine);
        let languages = languages.unwrap_or_else(|| settings.meeting_language.code().map(String::from).into_iter().collect());
        // Model download and transcription together: an error or cancellation of either part
        // restores the meeting state below (instead of leaving it in "transcribing").
        let r = async {
            ensure_support_models(&app, &id, settings.meeting_diarization, engine, &cancel).await?;
            let opts = Options {
                engine,
                languages: languages.clone(),
                vocabulary: settings.vocabulary.clone(),
                diarize: settings.meeting_diarization,
                tuning: Default::default(),
            };
            let (app2, id2, store2, cancel2) = (app.clone(), id.clone(), store.clone(), cancel.clone());
            tauri::async_runtime::spawn_blocking(move || {
                transcriber::transcribe(&store2, &id2, &opts, &cancel2, |p| emit(&app2, event(&id2, JobKind::Transcribe, p.step, p.fraction)))
            })
            .await
            .map_err(|e| anyhow!("{e}"))?
        }
        .await;
        match r {
            Ok(doc) => {
                store.update(&id, |m| {
                    m.state = State::Transcribed;
                    m.transcript_engine = Some(doc.engine.clone());
                    m.transcript_languages = Some(languages);
                })?;
                Ok(())
            }
            Err(e) => {
                // Cancellation returns to the previous state, an error is saved on the meeting.
                let cancelled = cancel.load(Ordering::Relaxed);
                store.update(&id, |m| {
                    m.state = if cancelled { previous } else { State::Failed };
                    m.last_error = (!cancelled).then(|| e.to_string());
                })?;
                Err(e)
            }
        }
    }
    .await;
    state.jobs.end();
    finish(&app, &id, JobKind::Transcribe, result.as_ref().err().map(|e| e.to_string()));
    result
}

pub async fn summarize(app: AppHandle, id: String, provider: ProviderId, model: Option<String>) -> Result<()> {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    let cancel = state.jobs.begin(&id, JobKind::Summarize)?;
    let store = Store::default();
    let result: Result<()> = async {
        let transcript = std::fs::read_to_string(store.transcript_md(&id)).map_err(|_| anyhow!(i18n::t("meeting.transcribe_first")))?;
        let mut config = LlmConfig::load(&settings, provider).map_err(|e| anyhow!(e.message))?;
        if let Some(m) = model.filter(|m| !m.trim().is_empty()) {
            config.model = m;
        }
        let previous = store.load(&id).map(|m| m.state).unwrap_or(State::Transcribed);
        store.update(&id, |m| m.state = State::Summarizing)?;
        emit(&app, event(&id, JobKind::Summarize, format!("{} · {}", provider.display_name(), config.model), 0.02));
        let prompt = (!settings.ai_prompt.trim().is_empty()).then_some(settings.ai_prompt.as_str());
        let app2 = app.clone();
        let id2 = id.clone();
        let label = format!("{} · {}", provider.display_name(), config.model);
        let work = summarizer::summarize(&config, &transcript, prompt, move |done, total| {
            let step = if total > 1 { i18n::t_with("job.part", &[("label", &label), ("part", &(done + 1).min(total)), ("total", &total)]) } else { label.clone() };
            emit(&app2, event(&id2, JobKind::Summarize, step, 0.05 + 0.9 * done as f32 / total.max(1) as f32));
        });
        // Cancellation: we drop the request (future), the state returns to the previous one.
        let outcome = tokio::select! {
            r = work => Some(r),
            _ = async { while !cancel.load(Ordering::Relaxed) { tokio::time::sleep(std::time::Duration::from_millis(200)).await } } => None,
        };
        match outcome {
            Some(Ok(text)) => {
                summarizer::save(&text, &store.summaries_folder(&id), &config, chrono::Local::now())?;
                store.update(&id, |m| {
                    m.state = State::Summarized;
                    m.last_error = None;
                })?;
                Ok(())
            }
            Some(Err(e)) => {
                store.update(&id, |m| {
                    m.state = previous;
                    m.last_error = Some(e.message.clone());
                })?;
                Err(anyhow!(e.message))
            }
            None => {
                store.update(&id, |m| m.state = previous)?;
                Err(anyhow!(super::CANCELLED))
            }
        }
    }
    .await;
    state.jobs.end();
    finish(&app, &id, JobKind::Summarize, result.as_ref().err().map(|e| e.to_string()));
    result
}

fn finish(app: &AppHandle, id: &str, kind: JobKind, error: Option<String>) {
    emit(app, JobEvent { meeting_id: id.into(), kind, step: i18n::t(if error.is_some() { "job.error" } else { "job.done" }), fraction: 1.0, finished: true, error });
    let _ = app.emit("meetings-changed", ());
}

/// Recording import from a file: creates the meeting right away (so it appears in the list), and in
/// the background loads the file with progress, then transcribes and — if enabled — summarises.
pub fn import_file(app: AppHandle, path: std::path::PathBuf) -> Result<Meeting> {
    let store = Store::default();
    let meeting = import::create(&store, &path)?;
    let cancel = match app.state::<AppState>().jobs.begin(&meeting.id, JobKind::Import) {
        Ok(c) => c,
        Err(e) => {
            let _ = store.delete(&meeting.id);
            return Err(e);
        }
    };
    let (m, id) = (meeting.clone(), meeting.id.clone());
    tauri::async_runtime::spawn(async move {
        emit(&app, event(&id, JobKind::Import, i18n::t("job.loading_file"), 0.0));
        let (app2, id2) = (app.clone(), id.clone());
        let r = tauri::async_runtime::spawn_blocking(move || {
            // Progress every 1 % — a long file has tens of thousands of packets.
            let mut shown = 0.0f32;
            import::fill(&store, &m, &path, &cancel, |f| {
                if f - shown >= 0.01 {
                    shown = f;
                    emit(&app2, event(&id2, JobKind::Import, i18n::t("job.loading_file"), f));
                }
            })
        })
        .await
        .map_err(|e| anyhow!("{e}"))
        .and_then(|r| r);
        app.state::<AppState>().jobs.end();
        finish(&app, &id, JobKind::Import, r.as_ref().err().map(|e| e.to_string()));
        if r.is_err() {
            return;
        }
        let settings = app.state::<AppState>().settings.lock().unwrap().clone();
        if transcribe(app.clone(), id.clone(), None, None).await.is_ok() && settings.meeting_auto_summarize && keys::has(settings.ai_provider) {
            let _ = summarize(app, id, settings.ai_provider, None).await;
        }
    });
    Ok(meeting)
}

/// After recording stops: transcription (if enabled), then a summary (if enabled and there's
/// a key for the default provider).
pub fn after_recording(app: AppHandle, id: String) {
    tauri::async_runtime::spawn(async move {
        let settings = app.state::<AppState>().settings.lock().unwrap().clone();
        if !settings.meeting_auto_transcribe {
            return;
        }
        if transcribe(app.clone(), id.clone(), None, None).await.is_err() {
            return;
        }
        if settings.meeting_auto_summarize && keys::has(settings.ai_provider) {
            let _ = summarize(app, id, settings.ai_provider, None).await;
        }
    });
}
