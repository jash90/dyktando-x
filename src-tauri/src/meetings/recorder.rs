//! Nagrywanie spotkania: mikrofon (cpal) + dźwięk systemowy, każda ścieżka przez resampler do
//! 16 kHz i zapis na dysk (nic nie rośnie w RAM). Ścieżki trzymamy wyrównane względem zegara
//! nagrania: gdy któraś zaczyna później albo ma przerwę (np. tap utworzony na nowo po zgodzie
//! użytkownika), lukę wypełniamy ciszą — znaczniki czasu obu ścieżek się zgadzają.
//! Opcjonalnie próbki 16 kHz obu ścieżek idą też do transkrypcji na żywo (`live`).
use anyhow::{anyhow, Result};
use chrono::Local;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::live::{self, Live};
use super::store::{Meeting, State, Store, MIC, SYSTEM};
use super::system::{self, SystemCapture};
use super::transcript::{self, TranscriptDocument, Utterance};
use super::writer::{SegmentedWriter, RATE};
use crate::audio::capture::InputCapture;
use crate::audio::resample::StreamResampler;

/// Luka większa niż to (w próbkach 16 kHz) jest wypełniana ciszą.
const GAP_TOLERANCE: u64 = RATE as u64 / 2;

/// Pilnuje, czy źródło naprawdę daje tyle próbek na sekundę, ile zgłosiło. Gdy zgłoszona
/// częstotliwość jest zła (np. macOS: tap 48 kHz, wyjście 16 kHz), resampler gubi część dźwięku,
/// a wypełnianie luk wstawia co chwilę ciszę — mowa robi się nierozpoznawalna. Czysta logika:
/// czas podaje wołający.
pub struct RateWatch {
    declared: u32,
    since: Option<f64>,
    last: f64,
    frames: u64,
}

impl RateWatch {
    /// Okno pomiaru (s).
    const WINDOW: f64 = 3.0;
    /// Przerwa w dostawach dłuższa niż to zaczyna pomiar od nowa (WASAPI loopback w ciszy nie
    /// wysyła nic — to nie jest zła częstotliwość).
    const STALL: f64 = 0.25;
    const TOLERANCE: f64 = 0.08;
    const STANDARD: [u32; 9] = [8_000, 11_025, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 96_000];

    pub fn new(declared: u32) -> Self {
        Self { declared, since: None, last: 0.0, frames: 0 }
    }

    /// `frames` próbek (mono, przed resamplingiem) przyszło w chwili `now` (s). Zwraca nową
    /// częstotliwość, jeśli zmierzona wyraźnie różni się od zgłoszonej.
    pub fn push(&mut self, frames: usize, now: f64) -> Option<u32> {
        let since = match self.since {
            Some(s) if (0.0..=Self::STALL).contains(&(now - self.last)) => s,
            _ => {
                // Pierwsza porcja (albo po przerwie) wyznacza początek okna — jej próbek nie liczymy.
                self.since = Some(now);
                self.last = now;
                self.frames = 0;
                return None;
            }
        };
        self.last = now;
        self.frames += frames as u64;
        let elapsed = now - since;
        if elapsed < Self::WINDOW {
            return None;
        }
        let measured = self.frames as f64 / elapsed;
        self.since = Some(now);
        self.frames = 0;
        let declared = self.declared as f64;
        if (measured - declared).abs() / declared <= Self::TOLERANCE {
            return None;
        }
        let snapped = Self::STANDARD.into_iter().min_by(|a, b| (*a as f64 - measured).abs().total_cmp(&(*b as f64 - measured).abs()))?;
        if (snapped as f64 - measured).abs() / measured > Self::TOLERANCE || snapped == self.declared {
            return None;
        }
        self.declared = snapped;
        Some(snapped)
    }
}

struct Track {
    writer: Option<SegmentedWriter>,
    resampler: Option<StreamResampler>,
    started: Instant,
    error: Option<String>,
    live: Option<(Arc<Live>, transcript::Track)>,
    watch: Option<RateWatch>,
    /// Szczytowy RMS od ostatniego odczytu (bity f32) — wskaźnik „sygnał jest” w oknie na żywo.
    peak: Arc<AtomicU32>,
}

impl Track {
    fn new(writer: SegmentedWriter, started: Instant, live: Option<(Arc<Live>, transcript::Track)>) -> Self {
        Self { writer: Some(writer), resampler: None, started, error: None, live, watch: None, peak: Arc::new(AtomicU32::new(0)) }
    }

    fn set_rate(&mut self, rate: u32) {
        self.resampler = StreamResampler::new(rate).map_err(|e| self.error = Some(e.to_string())).ok();
        self.watch = Some(RateWatch::new(rate));
    }

    fn push(&mut self, samples: &[f32]) {
        let now = self.started.elapsed().as_secs_f64();
        if let Some(rate) = self.watch.as_mut().and_then(|w| w.push(samples.len(), now)) {
            log::warn!("Źródło daje {rate} Hz zamiast zgłoszonych — przestawiam przeliczanie");
            let watch = self.watch.take();
            self.set_rate(rate);
            self.watch = watch;
        }
        let (Some(r), Some(w)) = (self.resampler.as_mut(), self.writer.as_mut()) else { return };
        let out = r.push(samples);
        if let Some((live, kind)) = &self.live {
            live.feed(*kind, &out);
        }
        let rms = crate::audio::rms(&out);
        let _ = self.peak.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| (rms > f32::from_bits(old)).then(|| rms.to_bits()));
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
    /// Szczytowy poziom (RMS) mikrofonu i dźwięku systemowego od poprzedniego odczytu.
    pub mic_level: f32,
    pub system_level: f32,
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
    live: Option<Arc<Live>>,
}

/// Odbiorca zdarzeń transkrypcji na żywo: (id spotkania, zdarzenie).
pub type LiveListener = Box<dyn Fn(&str, live::Event) + Send + Sync + 'static>;

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
            Some(a) => {
                let peak = |t: &Arc<Mutex<Track>>| f32::from_bits(t.lock().unwrap().peak.swap(0, Ordering::Relaxed));
                RecordingStatus {
                    recording: true,
                    meeting_id: Some(a.meeting.id.clone()),
                    seconds: a.started.elapsed().as_secs_f64(),
                    has_system_audio: a.system.lock().unwrap().is_some(),
                    warning: a.warning.lock().unwrap().clone(),
                    mic_level: peak(&a.mic_track),
                    system_level: peak(&a.sys_track),
                }
            }
            None => RecordingStatus {
                recording: false,
                meeting_id: None,
                seconds: 0.0,
                has_system_audio: false,
                warning: None,
                mic_level: 0.0,
                system_level: 0.0,
            },
        }
    }

    pub fn is_recording(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    /// Wypowiedzi przepisane dotąd na żywo w trwającym nagraniu (dla okna otwartego w trakcie).
    pub fn live_utterances(&self) -> Vec<Utterance> {
        let guard = self.active.lock().unwrap();
        let mut all = guard.as_ref().and_then(|a| a.live.as_ref()).map(|l| l.utterances()).unwrap_or_default();
        all.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        all
    }

    /// `live`: konfiguracja transkrypcji na żywo i odbiorca jej zdarzeń; `None` = wyłączona.
    pub fn start(&self, store: &Store, input_device: Option<&str>, live: Option<(live::Config, LiveListener)>) -> Result<Meeting> {
        let mut guard = self.active.lock().unwrap();
        if guard.is_some() {
            return Err(anyhow!("Nagrywanie już trwa"));
        }
        let system_available = system::availability();
        let meeting = store.create(Local::now(), system_available.is_ok())?;
        let audio = store.audio_folder(&meeting.id);
        let started = Instant::now();
        // Transkrypcja na żywo: brak modelu nie blokuje nagrania — tylko tekst się nie pojawi.
        let live = live.and_then(|(config, listener)| {
            let listener = Arc::new(listener);
            let (id, l) = (meeting.id.clone(), listener.clone());
            match Live::start(config, Box::new(move |e| l(&id, e))) {
                Ok(l) => Some(Arc::new(l)),
                Err(e) => {
                    log::warn!("transkrypcja na żywo wyłączona: {e}");
                    listener(&meeting.id, live::Event::Error(e.to_string()));
                    None
                }
            }
        });
        let tee = |kind| live.as_ref().map(|l| (l.clone(), kind));
        let mic_track = Arc::new(Mutex::new(Track::new(SegmentedWriter::new(&audio, MIC)?, started, tee(transcript::Track::Mic))));
        let sys_track = Arc::new(Mutex::new(Track::new(SegmentedWriter::new(&audio, SYSTEM)?, started, tee(transcript::Track::System))));
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
            live,
        });
        Ok(meeting)
    }

    /// Kończy nagranie, wyrównuje długości ścieżek i zapisuje `meeting.json` (stan `recorded`).
    /// Tekst z transkrypcji na żywo zostaje zapisany jako transkrypt tymczasowy (bez mówców);
    /// pełna transkrypcja po nagraniu go nadpisze.
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
        let live_engine = a.live.take().and_then(|l| {
            let utterances = l.finish();
            if utterances.is_empty() {
                return None;
            }
            let engine = format!("{} (na żywo)", l.engine_title);
            match save_live_transcript(store, &a.meeting, &engine, seconds, &utterances) {
                Ok(()) => Some(engine),
                Err(e) => {
                    log::error!("zapis transkryptu na żywo: {e}");
                    None
                }
            }
        });
        let meeting = store.update(&a.meeting.id, |m| {
            m.state = State::Recorded;
            m.ended_at = Some(Local::now());
            m.duration_seconds = seconds;
            m.has_system_audio = had_system;
            m.last_error = (!errors.is_empty()).then(|| errors.join("; "));
            if live_engine.is_some() {
                m.transcript_engine = live_engine.clone();
            }
        })?;
        log::info!("Koniec nagrania {} ({:.0} s)", meeting.id, seconds);
        Ok(meeting)
    }
}

#[cfg(test)]
mod tests {
    use super::RateWatch;

    /// Porcje co 10 ms przez `secs` sekund, `rate` próbek/s; zwraca pierwszą zgłoszoną zmianę.
    fn feed(w: &mut RateWatch, rate: f64, from: f64, secs: f64, jitter: bool) -> Option<u32> {
        let mut t = from;
        let mut i = 0;
        while t < from + secs {
            let n = (rate * 0.01 * if jitter && i % 2 == 0 { 1.03 } else if jitter { 0.97 } else { 1.0 }) as usize;
            if let Some(r) = w.push(n, t) {
                return Some(r);
            }
            t += 0.01;
            i += 1;
        }
        None
    }

    #[test]
    fn detects_16k_declared_as_48k() {
        let mut w = RateWatch::new(48_000);
        assert_eq!(feed(&mut w, 16_000.0, 0.0, 4.0, false), Some(16_000));
        assert_eq!(feed(&mut w, 16_000.0, 4.0, 8.0, false), None, "po przestawieniu bez kolejnych zmian");
    }

    #[test]
    fn jitter_is_not_a_rate_change() {
        let mut w = RateWatch::new(48_000);
        assert_eq!(feed(&mut w, 48_000.0, 0.0, 10.0, true), None);
    }

    #[test]
    fn snaps_to_44100() {
        let mut w = RateWatch::new(48_000);
        assert_eq!(feed(&mut w, 44_100.0, 0.0, 4.0, false), Some(44_100));
    }

    #[test]
    fn stalls_restart_the_window() {
        // Dostawy 0,1 s co 0,5 s (WASAPI w ciszy) — średnio mało próbek, ale to nie zła częstotliwość.
        let mut w = RateWatch::new(48_000);
        let mut t = 0.0;
        for _ in 0..40 {
            for k in 0..10 {
                assert_eq!(w.push(480, t + k as f64 * 0.01), None);
            }
            t += 0.5;
        }
    }
}

/// Transkrypt tymczasowy z wypowiedzi na żywo: te same pliki co po pełnej transkrypcji
/// (`transcript.json`, `transcript.md`), bez rozpoznawania mówców.
fn save_live_transcript(store: &Store, meeting: &Meeting, engine: &str, seconds: f64, utterances: &[Utterance]) -> Result<()> {
    let (mic, system): (Vec<Utterance>, Vec<Utterance>) = utterances.iter().cloned().partition(|u| u.track == transcript::Track::Mic);
    let doc = TranscriptDocument {
        meeting_id: meeting.id.clone(),
        engine: engine.to_string(),
        created_at: Local::now(),
        duration_seconds: seconds,
        utterances: transcript::build(&mic, &system, None),
    };
    std::fs::write(store.transcript_json(&meeting.id), serde_json::to_vec_pretty(&doc)?)?;
    std::fs::write(store.transcript_md(&meeting.id), transcript::markdown(&doc, meeting.started_at))?;
    Ok(())
}
