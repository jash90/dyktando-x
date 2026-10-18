//! Dictation flow (the counterpart of AppDelegate in the Swift version):
//! shortcut → microphone → 16 kHz → engine → postprocessing → paste, with state for the HUD bubble.
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::{self, capture::InputCapture, resample};
use crate::dictations;
use crate::engine::Engine;
use crate::focus::FieldText;
use crate::models::EngineId;
use crate::postprocess::{self, Around};
use crate::paste;
use crate::settings::{PasteMode, Settings};
use crate::AppState;

/// A shorter recording is an accidental key brush.
const MIN_SECONDS: f32 = 0.3;
/// Names from the focused window passed to the engine (the most frequent).
const MAX_WINDOW_NAMES: usize = 60;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum HudState {
    Idle,
    Recording { level: f32, seconds: f32 },
    Transcribing,
    Done { text: String, pasted: bool },
    Error { message: String },
    /// A short notice (e.g. a reminder about participants' consent when recording a meeting).
    Info { message: String },
}

/// What the focused app shows, read while recording (it still has focus then).
#[derive(Default)]
struct Context {
    field: Option<FieldText>,
    /// Names and terms from the focused window (see `vocabulary::names_in`).
    names: Vec<String>,
}

impl Context {
    fn read(field: bool, window: bool) -> Self {
        let names = window.then(crate::focus::window_text).flatten();
        Self {
            field: field.then(crate::focus::field_text).flatten(),
            names: names.map(|t| crate::vocabulary::names_in(&t, MAX_WINDOW_NAMES)).unwrap_or_default(),
        }
    }
}

enum Phase {
    Idle,
    Recording { capture: InputCapture, buffer: Arc<Mutex<Vec<f32>>>, started: Instant, context: Option<JoinHandle<Context>> },
    Transcribing,
}

/// Learning from corrections only looks at a recent paste.
const LEARN_WITHIN: Duration = Duration::from_secs(30 * 60);

pub struct Dictation {
    phase: Mutex<Phase>,
    /// The last pasted text, compared with the field at the next dictation (see `learning`).
    last_pasted: Mutex<Option<(String, Instant)>>,
    engine: Arc<Mutex<Option<(EngineId, Engine)>>>,
    level: Arc<AtomicU32>,
    recording: Arc<AtomicBool>,
}

impl Default for Dictation {
    fn default() -> Self {
        Self {
            phase: Mutex::new(Phase::Idle),
            last_pasted: Mutex::new(None),
            engine: Arc::new(Mutex::new(None)),
            level: Arc::new(AtomicU32::new(0)),
            recording: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub fn emit(app: &AppHandle, state: HudState) {
    let _ = app.emit("hud-state", &state);
    crate::hud::update(app, &state);
}

impl Dictation {
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// Loads the engine in the background so the first dictation doesn't wait several seconds.
    pub fn preload(&self, id: EngineId) {
        if !id.asset().is_installed() {
            return;
        }
        let engine = self.engine.clone();
        std::thread::spawn(move || {
            let mut guard = engine.lock().unwrap();
            if guard.as_ref().map(|(i, _)| *i) == Some(id) {
                return;
            }
            *guard = None; // free the old model first (memory)
            match Engine::load(id) {
                Ok(e) => *guard = Some((id, e)),
                Err(e) => log::error!("preload {id:?}: {e}"),
            }
        });
    }

    pub fn unload(&self) {
        *self.engine.lock().unwrap() = None;
    }

    pub fn start(&self, app: &AppHandle, settings: &Settings) {
        let mut phase = self.phase.lock().unwrap();
        if !matches!(*phase, Phase::Idle) {
            return;
        }
        if !settings.engine.asset().is_installed() {
            emit(app, HudState::Error { message: crate::i18n::t_with("dictation.download_model_first", &[("model", &settings.engine.asset().label())]) });
            return;
        }
        let buffer = Arc::new(Mutex::new(Vec::<f32>::with_capacity(48_000 * 30)));
        let sink_buf = buffer.clone();
        let level = self.level.clone();
        let sink = Box::new(move |mono: &[f32]| {
            if let Ok(mut b) = sink_buf.lock() {
                b.extend_from_slice(mono);
            }
            level.store(audio::rms(mono).to_bits(), Ordering::Relaxed);
        });
        match InputCapture::start(settings.input_device.as_deref(), sink) {
            Ok(capture) => {
                log::info!("Recording: {} @ {} Hz", capture.device_name, capture.sample_rate);
                let started = Instant::now();
                let (field, window) = (settings.dictation_context, settings.dictation_window_context);
                let context = (field || window).then(|| std::thread::spawn(move || Context::read(field, window)));
                *phase = Phase::Recording { capture, buffer, started, context };
                self.recording.store(true, Ordering::Relaxed);
                emit(app, HudState::Recording { level: 0.0, seconds: 0.0 });
                self.spawn_level_ticker(app.clone(), started);
            }
            Err(e) => emit(app, HudState::Error { message: crate::i18n::t_with("mic.error", &[("error", &e)]) }),
        }
    }

    fn spawn_level_ticker(&self, app: AppHandle, started: Instant) {
        let recording = self.recording.clone();
        let level = self.level.clone();
        std::thread::spawn(move || {
            while recording.load(Ordering::Relaxed) {
                let l = f32::from_bits(level.load(Ordering::Relaxed));
                let _ = app.emit("hud-state", HudState::Recording { level: l, seconds: started.elapsed().as_secs_f32() });
                std::thread::sleep(Duration::from_millis(60));
            }
        });
    }

    pub fn toggle(&self, app: &AppHandle, settings: &Settings) {
        if self.is_recording() {
            self.stop(app, settings.clone(), false);
        } else {
            self.start(app, settings);
        }
    }

    /// Ends the recording; `cancel` discards it without transcription.
    pub fn stop(&self, app: &AppHandle, settings: Settings, cancel: bool) {
        let mut phase = self.phase.lock().unwrap();
        let Phase::Recording { capture, buffer, started, context } = std::mem::replace(&mut *phase, Phase::Idle) else {
            return;
        };
        self.recording.store(false, Ordering::Relaxed);
        let rate = capture.sample_rate;
        capture.stop();
        let samples = std::mem::take(&mut *buffer.lock().unwrap());
        let seconds = started.elapsed().as_secs_f32();
        if cancel || seconds < MIN_SECONDS {
            emit(app, HudState::Idle);
            return;
        }
        *phase = Phase::Transcribing;
        drop(phase);
        emit(app, HudState::Transcribing);

        let app = app.clone();
        let engine = self.engine.clone();
        let created_at = chrono::Local::now();
        std::thread::spawn(move || {
            let Context { field, names } = context.and_then(|h| h.join().ok()).unwrap_or_default();
            if !names.is_empty() {
                // Only the count: the names are other people's content (chats, mail).
                log::info!("Names from the window: {}", names.len());
            }
            let state = app.state::<AppState>();
            let previous = state.dictation.last_pasted.lock().unwrap().take();
            let result = transcribe_and_insert(&engine, &samples, rate, &settings, field.as_ref(), &names);
            *state.dictation.phase.lock().unwrap() = Phase::Idle;
            if let (Some((text, at)), Some(field), true) = (previous, field.as_ref(), settings.vocabulary_learning) {
                if at.elapsed() < LEARN_WITHIN {
                    // An error stays in the bubble: the notice would replace it.
                    learn(&app, &text, field, result.is_ok());
                }
            }
            match result {
                Ok(Some(done)) => {
                    let pasted = done.outcome == paste::Outcome::Pasted;
                    if pasted {
                        *state.dictation.last_pasted.lock().unwrap() = Some((done.text.clone(), Instant::now()));
                    }
                    emit(&app, HudState::Done { text: done.text.clone(), pasted });
                    // After pasting — saving to history doesn't delay inserting the text.
                    if settings.dictation_history {
                        let entry = dictations::Entry {
                            id: dictations::new_id(created_at),
                            created_at,
                            duration_seconds: done.audio16.len() as f64 / 16_000.0,
                            text: done.text,
                            raw: done.raw,
                            engine: settings.engine.asset().title.to_string(),
                            languages: settings.language.code().map(String::from).into_iter().collect(),
                            pasted,
                            audio_deleted: false,
                        };
                        dictations::record(&app, &entry, &done.audio16);
                    }
                }
                Ok(None) => emit(&app, HudState::Idle),
                Err(e) => emit(&app, HudState::Error { message: e.to_string() }),
            }
        });
    }
}

/// Adds the names the user corrected in the previously pasted text to the vocabulary (the field as
/// read at this dictation's start), and tells them in the bubble (`notify`) once this dictation's
/// is gone.
fn learn(app: &AppHandle, pasted: &str, field: &FieldText, notify: bool) {
    let text = format!("{}{}{}", field.before, field.selected, field.after);
    let found = crate::learning::corrections(pasted, &text);
    if found.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let settings = {
        let mut s = state.settings.lock().unwrap();
        let entries = crate::learning::entries(&found, &s.vocabulary);
        if entries.is_empty() {
            return;
        }
        log::info!("Learned from corrections: {entries:?}");
        s.vocabulary = crate::learning::append(&s.vocabulary, &entries);
        if let Err(e) = s.save() {
            log::error!("saving the learned vocabulary: {e}");
        }
        let message = crate::i18n::t_with("hud.vocabulary_learned", &[("words", &entries.join(", "))]);
        (s.clone(), message)
    };
    let _ = app.emit("settings-changed", &settings.0);
    if !notify {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1600));
        if !app.state::<AppState>().dictation.is_recording() {
            emit(&app, HudState::Info { message: settings.1 });
        }
    });
}

/// A transcribed and inserted dictation (with the 16 kHz recording — for history).
struct Done {
    text: String,
    raw: String,
    outcome: paste::Outcome,
    audio16: Vec<f32>,
}

fn transcribe_and_insert(
    engine: &Mutex<Option<(EngineId, Engine)>>,
    samples: &[f32],
    rate: u32,
    settings: &Settings,
    field: Option<&FieldText>,
    window_names: &[String],
) -> Result<Option<Done>> {
    if audio::is_digital_silence(samples) {
        return Err(anyhow!(crate::i18n::t("dictation.digital_silence")));
    }
    let audio16 = resample::to_16k(samples, rate)?;
    let should_paste = match settings.paste_mode {
        PasteMode::ClipboardOnly => false,
        PasteMode::Always => true,
        PasteMode::Auto => crate::focus::should_paste(),
    };
    let t0 = Instant::now();
    let (raw, text) = {
        let mut guard = engine.lock().unwrap();
        if guard.as_ref().map(|(i, _)| *i) != Some(settings.engine) {
            *guard = None;
            *guard = Some((settings.engine, Engine::load(settings.engine)?));
        }
        let (_, e) = guard.as_mut().expect("engine loaded");
        e.set_vocabulary(&settings.vocabulary);
        e.set_window_names(window_names);
        e.set_context(field.map(|f| f.before.as_str()));
        let raw = e.transcribe(&audio16, settings.language)?;
        // Joined with the field only when the text goes into it.
        let text = match field.filter(|_| should_paste) {
            Some(f) => {
                let (sees, model) = (e.sees_context(), postprocess::written_so(&raw));
                postprocess::apply_in(&raw, &Around { before: &f.before, after: &f.after }, |w| e.has_term(w) || (sees && model(w)))
            }
            None => postprocess::apply(&raw),
        };
        (raw, text)
    };
    log::info!(
        "Transcribed {:.1} s of audio in {:.2} s: {raw:?}",
        audio16.len() as f32 / 16_000.0,
        t0.elapsed().as_secs_f32()
    );
    if text.trim().is_empty() {
        return Ok(None);
    }
    let outcome = paste::insert(&text, should_paste)?;
    // History and the bubble show the text without the spaces that join it with the field.
    Ok(Some(Done { text: text.trim().to_string(), raw, outcome, audio16 }))
}
