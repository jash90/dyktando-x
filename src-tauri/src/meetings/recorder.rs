//! Nagrywanie spotkania: mikrofon (cpal) + dźwięk systemowy, każda ścieżka przez resampler do
//! 16 kHz i zapis na dysk (nic nie rośnie w RAM). Ścieżki trzymamy wyrównane względem zegara
//! nagrania: gdy któraś zaczyna później albo ma przerwę (np. tap utworzony na nowo po zgodzie
//! użytkownika), lukę wypełniamy ciszą — znaczniki czasu obu ścieżek się zgadzają.
use anyhow::{anyhow, Result};
use chrono::Local;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::store::{Meeting, State, Store, MIC, SYSTEM};
use super::system::{self, SystemCapture};
use super::writer::{SegmentedWriter, RATE};
use crate::audio::capture::InputCapture;
use crate::audio::resample::StreamResampler;

/// Luka większa niż to (w próbkach 16 kHz) jest wypełniana ciszą.
const GAP_TOLERANCE: u64 = RATE as u64 / 2;

struct Track {
    writer: Option<SegmentedWriter>,
    resampler: Option<StreamResampler>,
    started: Instant,
    error: Option<String>,
}

impl Track {
    fn new(writer: SegmentedWriter, started: Instant) -> Self {
        Self { writer: Some(writer), resampler: None, started, error: None }
    }

    fn set_rate(&mut self, rate: u32) {
        self.resampler = StreamResampler::new(rate).map_err(|e| self.error = Some(e.to_string())).ok();
    }

    fn push(&mut self, samples: &[f32]) {
        let (Some(r), Some(w)) = (self.resampler.as_mut(), self.writer.as_mut()) else { return };
        let out = r.push(samples);
        let expected = (self.started.elapsed().as_secs_f64() * RATE as f64) as u64;
        let after = w.samples_written() + out.len() as u64;
        let result = if expected > after + GAP_TOLERANCE { w.append_silence(expected - after) } else { Ok(()) }.and_then(|_| w.append(&out));
        if let Err(e) = result {
            // Pełny dysk itp. — zapamiętaj pierwszy błąd, pokażemy go przy zatrzymaniu.
            if self.error.is_none() {
                log::error!("zapis ścieżki: {e}");
                self.error = Some(e.to_string());
            }
        }
    }

    fn finish(&mut self) -> Result<u64> {
        if let Some(r) = self.resampler.as_mut() {
            let tail = r.flush();
            if let Some(w) = self.writer.as_mut() {
                w.append(&tail)?;
            }
        }
        self.writer.take().map(|w| w.finish()).unwrap_or(Ok(0))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordingStatus {
    pub recording: bool,
    pub meeting_id: Option<String>,
    pub seconds: f64,
    pub has_system_audio: bool,
    pub warning: Option<String>,
}

struct Active {
    meeting: Meeting,
    started: Instant,
    mic: Option<InputCapture>,
    system: Arc<Mutex<Option<SystemCapture>>>,
    mic_track: Arc<Mutex<Track>>,
    sys_track: Arc<Mutex<Track>>,
    stop_monitor: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
    warning: Arc<Mutex<Option<String>>>,
}

#[derive(Default)]
pub struct Recorder {
    active: Mutex<Option<Active>>,
}

fn system_sink(track: Arc<Mutex<Track>>) -> system::Sink {
    Box::new(move |s: &[f32]| {
        if let Ok(mut t) = track.lock() {
            t.push(s);
        }
    })
}

fn start_system(track: &Arc<Mutex<Track>>) -> Result<SystemCapture> {
    let capture = SystemCapture::start(system_sink(track.clone()))?;
    track.lock().unwrap().set_rate(capture.sample_rate);
    Ok(capture)
}

impl Recorder {
    pub fn status(&self) -> RecordingStatus {
        let guard = self.active.lock().unwrap();
        match guard.as_ref() {
            Some(a) => RecordingStatus {
                recording: true,
                meeting_id: Some(a.meeting.id.clone()),
                seconds: a.started.elapsed().as_secs_f64(),
                has_system_audio: a.system.lock().unwrap().is_some(),
                warning: a.warning.lock().unwrap().clone(),
            },
            None => RecordingStatus { recording: false, meeting_id: None, seconds: 0.0, has_system_audio: false, warning: None },
        }
    }

    pub fn is_recording(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    pub fn start(&self, store: &Store, input_device: Option<&str>) -> Result<Meeting> {
        let mut guard = self.active.lock().unwrap();
        if guard.is_some() {
            return Err(anyhow!("Nagrywanie już trwa"));
        }
        let system_available = system::availability();
        let meeting = store.create(Local::now(), system_available.is_ok())?;
        let audio = store.audio_folder(&meeting.id);
        let started = Instant::now();
        let mic_track = Arc::new(Mutex::new(Track::new(SegmentedWriter::new(&audio, MIC)?, started)));
        let sys_track = Arc::new(Mutex::new(Track::new(SegmentedWriter::new(&audio, SYSTEM)?, started)));
        let warning = Arc::new(Mutex::new(system_available.clone().err()));

        let sink_track = mic_track.clone();
        let mic = match InputCapture::start(
            input_device,
            Box::new(move |s: &[f32]| {
                if let Ok(mut t) = sink_track.lock() {
                    t.push(s);
                }
            }),
        ) {
            Ok(m) => m,
            Err(e) => {
                let _ = store.delete(&meeting.id);
                return Err(anyhow!("Mikrofon: {e}"));
            }
        };
        mic_track.lock().unwrap().set_rate(mic.sample_rate);

        let system = Arc::new(Mutex::new(None));
        if system_available.is_ok() {
            match start_system(&sys_track) {
                Ok(c) => *system.lock().unwrap() = Some(c),
                Err(e) => {
                    log::warn!("{e}");
                    *warning.lock().unwrap() = Some(format!("{e} Nagrywam tylko mikrofon."));
                }
            }
        }

        // Nadzór: tap bez żadnych buforów po kilku sekundach = utworzony przed zgodą → od nowa.
        let stop_monitor = Arc::new(AtomicBool::new(false));
        let monitor = {
            let (stop, system, sys_track, warning) = (stop_monitor.clone(), system.clone(), sys_track.clone(), warning.clone());
            std::thread::spawn(move || {
                let mut since = Instant::now();
                let mut attempts = 0;
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                    let dead = system.lock().unwrap().as_ref().is_some_and(|c| c.buffers_received() == 0);
                    if dead && since.elapsed() > Duration::from_secs(4) && attempts < 30 {
                        attempts += 1;
                        log::info!("Dźwięk systemowy milczy — tworzę przechwytywanie od nowa (próba {attempts})");
                        if let Some(old) = system.lock().unwrap().take() {
                            old.stop();
                        }
                        match start_system(&sys_track) {
                            Ok(c) => *system.lock().unwrap() = Some(c),
                            Err(e) => *warning.lock().unwrap() = Some(e.to_string()),
                        }
                        since = Instant::now();
                    }
                    if !dead && attempts > 0 {
                        *warning.lock().unwrap() = None;
                        attempts = 0;
                    }
                }
            })
        };

        log::info!("Nagrywanie spotkania {} (mikrofon: {})", meeting.id, mic.device_name);
        *guard = Some(Active {
            meeting: meeting.clone(),
            started,
            mic: Some(mic),
            system,
            mic_track,
            sys_track,
            stop_monitor,
            monitor: Some(monitor),
            warning,
        });
        Ok(meeting)
    }

    /// Kończy nagranie, wyrównuje długości ścieżek i zapisuje `meeting.json` (stan `recorded`).
    pub fn stop(&self, store: &Store) -> Result<Meeting> {
        let mut a = self.active.lock().unwrap().take().ok_or_else(|| anyhow!("Nic nie jest nagrywane"))?;
        a.stop_monitor.store(true, Ordering::Relaxed);
        if let Some(m) = a.monitor.take() {
            let _ = m.join();
        }
        if let Some(mic) = a.mic.take() {
            mic.stop();
        }
        let had_system = a.system.lock().unwrap().is_some();
        if let Some(sys) = a.system.lock().unwrap().take() {
            sys.stop();
        }
        let seconds = a.started.elapsed().as_secs_f64();
        let expected = (seconds * RATE as f64) as u64;
        let mut errors = Vec::new();
        for track in [&a.mic_track, &a.sys_track] {
            let mut t = track.lock().unwrap();
            if let Some(w) = t.writer.as_mut() {
                let have = w.samples_written();
                if expected > have {
                    let _ = w.append_silence(expected - have);
                }
            }
            if let Err(e) = t.finish() {
                errors.push(e.to_string());
            }
            if let Some(e) = t.error.take() {
                errors.push(e);
            }
        }
        let meeting = store.update(&a.meeting.id, |m| {
            m.state = State::Recorded;
            m.ended_at = Some(Local::now());
            m.duration_seconds = seconds;
            m.has_system_audio = had_system;
            m.last_error = (!errors.is_empty()).then(|| errors.join("; "));
        })?;
        log::info!("Koniec nagrania {} ({:.0} s)", meeting.id, seconds);
        Ok(meeting)
    }
}
