//! Zadania na spotkaniach (jedno naraz): transkrypcja i podsumowanie AI, z postępem wysyłanym do
//! interfejsu (`meeting-job`) i przerywaniem. Po zatrzymaniu nagrania — automatyczny ciąg
//! transkrypcja → (opcjonalnie) podsumowanie, zgodnie z ustawieniami.
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

use super::import;
use super::store::{Meeting, State, Store};
use super::transcriber::{self, Options};
use crate::ai::{keys, provider::LlmConfig, summarizer};
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
            return Err(anyhow!("Trwa już przetwarzanie spotkania {other} — poczekaj albo je przerwij"));
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

/// Małe modele potrzebne do spotkań (VAD 2 MB, mówcy 27 MB) dociągamy same, bez pytania.
/// Pobiera brakujące modele potrzebne do przepisania: VAD, model mówców i sam silnik (np.
/// Whisper ustawiony jako domyślny, ale jeszcze niepobrany).
async fn ensure_support_models(app: &AppHandle, id: &str, diarize: bool, engine: EngineId) -> Result<()> {
    let mut needed = vec![models::asset(AssetId::SileroVad)];
    if diarize {
        needed.push(models::asset(AssetId::SpeakerModel));
    }
    needed.push(engine.asset());
    for asset in needed {
        if asset.is_installed() {
            continue;
        }
        let app2 = app.clone();
        let title = asset.title;
        asset
            .download(&AtomicBool::new(false), move |done, total| {
                let f = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                emit(&app2, event(id, JobKind::Transcribe, format!("Pobieranie: {title} ({:.0}%)", f * 100.0), f * 0.05));
            })
            .await?;
    }
    Ok(())
}

/// `languages: None` = język spotkań z ustawień (zob. `transcriber::Options::languages`).
pub async fn transcribe(app: AppHandle, id: String, engine: Option<EngineId>, languages: Option<Vec<String>>) -> Result<()> {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    let cancel = state.jobs.begin(&id, JobKind::Transcribe)?;
    let store = Store::default();
    let result: Result<()> = async {
        let previous = store.load(&id).ok_or_else(|| anyhow!("Brak spotkania"))?.state;
        store.update(&id, |m| {
            m.state = State::Transcribing;
            m.last_error = None;
        })?;
        emit(&app, event(&id, JobKind::Transcribe, "Przygotowanie", 0.0));
        let engine = engine.unwrap_or(settings.meeting_engine);
        ensure_support_models(&app, &id, settings.meeting_diarization, engine).await?;
        let languages = languages.unwrap_or_else(|| settings.meeting_language.code().map(String::from).into_iter().collect());
        let opts = Options {
            engine,
            languages: languages.clone(),
            vocabulary: settings.vocabulary.clone(),
            diarize: settings.meeting_diarization,
            tuning: Default::default(),
        };
        let (app2, id2, store2, cancel2) = (app.clone(), id.clone(), store.clone(), cancel.clone());
        let r = tauri::async_runtime::spawn_blocking(move || {
            transcriber::transcribe(&store2, &id2, &opts, &cancel2, |p| emit(&app2, event(&id2, JobKind::Transcribe, p.step, p.fraction)))
        })
        .await
        .map_err(|e| anyhow!("{e}"))?;
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
                // Przerwanie wraca do poprzedniego stanu, błąd zapisujemy przy spotkaniu.
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
        let transcript = std::fs::read_to_string(store.transcript_md(&id)).map_err(|_| anyhow!("Najpierw przepisz spotkanie"))?;
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
            let step = if total > 1 { format!("{label} — część {}/{}", (done + 1).min(total), total) } else { label.clone() };
            emit(&app2, event(&id2, JobKind::Summarize, step, 0.05 + 0.9 * done as f32 / total.max(1) as f32));
        });
        // Przerwanie: porzucamy zapytanie (future), stan wraca do poprzedniego.
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
                Err(anyhow!("Przerwano"))
            }
        }
    }
    .await;
    state.jobs.end();
    finish(&app, &id, JobKind::Summarize, result.as_ref().err().map(|e| e.to_string()));
    result
}

fn finish(app: &AppHandle, id: &str, kind: JobKind, error: Option<String>) {
    emit(app, JobEvent { meeting_id: id.into(), kind, step: if error.is_some() { "Błąd".into() } else { "Gotowe".into() }, fraction: 1.0, finished: true, error });
    let _ = app.emit("meetings-changed", ());
}

/// Import nagrania z pliku: tworzy spotkanie od razu (żeby pojawiło się na liście), a w tle
/// wczytuje plik z postępem, potem przepisuje i — jeśli włączone — podsumowuje.
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
        emit(&app, event(&id, JobKind::Import, "Wczytywanie pliku", 0.0));
        let (app2, id2) = (app.clone(), id.clone());
        let r = tauri::async_runtime::spawn_blocking(move || {
            // Postęp co 1 % — przy długim pliku pakietów są dziesiątki tysięcy.
            let mut shown = 0.0f32;
            import::fill(&store, &m, &path, &cancel, |f| {
                if f - shown >= 0.01 {
                    shown = f;
                    emit(&app2, event(&id2, JobKind::Import, "Wczytywanie pliku", f));
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

/// Po zatrzymaniu nagrania: transkrypcja (jeśli włączona), potem podsumowanie (jeśli włączone
/// i jest klucz domyślnego dostawcy).
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
