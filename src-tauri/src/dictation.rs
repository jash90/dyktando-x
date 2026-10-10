//! Dictation flow (the counterpart of AppDelegate in the Swift version):
//! shortcut → microphone → 16 kHz → engine → postprocessing → paste, with state for the HUD bubble.
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::{self, capture::InputCapture, resample};
use crate::dictations;
use crate::engine::Engine;
use crate::models::EngineId;
use crate::paste;
use crate::settings::{PasteMode, Settings};
use crate::AppState;

/// A shorter recording is an accidental key brush.
const MIN_SECONDS: f32 = 0.3;

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

enum Phase {
    Idle,
    Recording { capture: InputCapture, buffer: Arc<Mutex<Vec<f32>>>, started: Instant },
    Transcribing,
}

pub struct Dictation {
    phase: Mutex<Phase>,
    engine: Arc<Mutex<Option<(EngineId, Engine)>>>,
    level: Arc<AtomicU32>,
    recording: Arc<AtomicBool>,
}

impl Default for Dictation {
    fn default() -> Self {
        Self {
            phase: Mutex::new(Phase::Idle),
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
            emit(app, HudState::Error { message: format!("Najpierw pobierz model {} w Ustawieniach", settings.engine.asset().title) });
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
                log::info!("Nagrywanie: {} @ {} Hz", capture.device_name, capture.sample_rate);
                let started = Instant::now();
                *phase = Phase::Recording { capture, buffer, started };
                self.recording.store(true, Ordering::Relaxed);
                emit(app, HudState::Recording { level: 0.0, seconds: 0.0 });
                self.spawn_level_ticker(app.clone(), started);
            }
            Err(e) => emit(app, HudState::Error { message: format!("Mikrofon: {e}") }),
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
        let Phase::Recording { capture, buffer, started } = std::mem::replace(&mut *phase, Phase::Idle) else {
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
            let result = transcribe_and_insert(&engine, &samples, rate, &settings);
            let state = app.state::<AppState>();
            *state.dictation.phase.lock().unwrap() = Phase::Idle;
            match result {
                Ok(Some(done)) => {
                    let pasted = done.outcome == paste::Outcome::Pasted;
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
) -> Result<Option<Done>> {
    if audio::is_digital_silence(samples) {
        return Err(anyhow!(
            "Mikrofon nagrał cyfrową ciszę — system nie dał dostępu do mikrofonu. Sprawdź uprawnienia w ustawieniach prywatności."
        ));
    }
    let audio16 = resample::to_16k(samples, rate)?;
    let t0 = Instant::now();
    let raw = {
        let mut guard = engine.lock().unwrap();
        if guard.as_ref().map(|(i, _)| *i) != Some(settings.engine) {
            *guard = None;
            *guard = Some((settings.engine, Engine::load(settings.engine)?));
        }
        let (_, e) = guard.as_mut().expect("silnik wczytany");
        e.set_vocabulary(&settings.vocabulary);
        e.transcribe(&audio16, settings.language)?
    };
    log::info!(
        "Transkrypcja {:.1} s audio w {:.2} s: {raw:?}",
        audio16.len() as f32 / 16_000.0,
        t0.elapsed().as_secs_f32()
    );
    let text = crate::postprocess::apply(&raw);
    if text.is_empty() {
        return Ok(None);
    }
    let should_paste = match settings.paste_mode {
        PasteMode::ClipboardOnly => false,
        PasteMode::Always => true,
        PasteMode::Auto => crate::focus::should_paste(),
    };
    let outcome = paste::insert(&text, should_paste)?;
    Ok(Some(Done { text, raw, outcome, audio16 }))
}
