//! Nagrywanie spotkania: mikrofon (cpal) + dźwięk systemowy, każda ścieżka przez resampler do
//! 16 kHz i zapis na dysk (nic nie rośnie w RAM). Ścieżki trzymamy wyrównane względem zegara
//! nagrania: gdy któraś zaczyna później albo ma przerwę (np. tap utworzony na nowo po zgodzie
//! użytkownika), lukę wypełniamy ciszą — znaczniki czasu obu ścieżek się zgadzają.
//! Opcjonalnie próbki 16 kHz obu ścieżek idą też do transkrypcji na żywo (`live`).
//!
//! Wątki audio tylko kopiują próbki do kolejki (z chwilą odbioru) — przeliczanie, zapis na dysk
//! i transkrypcję na żywo robi osobny wątek, więc chwilowa zadyszka dysku nie gubi dźwięku.
//! Nadzór pilnuje obu źródeł: gdy dostawy ustaną, strumień zgłosi błąd albo zmieni się
//! urządzenie (np. słuchawki Bluetooth), przechwytywanie jest tworzone od nowa. Przerwy,
//! których nie dało się uniknąć, trafiają do logu i do `Meeting::audio_gaps`.
use anyhow::{anyhow, Result};
use chrono::Local;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::live::{self, Live};
use super::store::{AudioGap, Meeting, State, Store, MIC, SYSTEM};
use super::system::{self, SystemCapture};
use super::transcript::{self, TranscriptDocument, Utterance};
use super::writer::{SegmentedWriter, RATE};
use crate::audio::capture::{self, InputCapture};
use crate::audio::resample::StreamResampler;

/// Luka większa niż to (w próbkach 16 kHz) jest wypełniana ciszą.
const GAP_TOLERANCE: u64 = RATE as u64 / 2;
/// Tak długa przerwa w dostawach (ms) znaczy, że źródło się urwało.
const STALL_MS: u64 = 3_000;
/// Najkrótszy odstęp (ms) między kolejnymi próbami odtworzenia tego samego źródła.
const RETRY_MS: u64 = 3_000;
const MAX_RETRIES: u32 = 30;
/// Mikrofon dający same zera tak długo (ms) jest najpewniej wyciszony w systemie.
const SILENT_MIC_MS: u64 = 10_000;
/// Przerwy krótsze niż to (s) nie trafiają do `Meeting::audio_gaps`.
const REPORTED_GAP_S: f64 = 2.0;

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
    /// „mikrofon” / „rozmówcy” — do logów.
    name: &'static str,
    writer: Option<SegmentedWriter>,
    resampler: Option<StreamResampler>,
    error: Option<String>,
    live: Option<(Arc<Live>, transcript::Track)>,
    watch: Option<RateWatch>,
    /// Szczytowy RMS od ostatniego odczytu (bity f32) — wskaźnik „sygnał jest” w oknie na żywo.
    peak: Arc<AtomicU32>,
    /// Czy luki to zgubiony dźwięk (WASAPI loopback w ciszy nie wysyła nic — tam luka to cisza).
    report_gaps: bool,
    /// Przerwy w środku nagrania: (początek s, długość s).
    gaps: Vec<(f64, f64)>,
}

impl Track {
    fn new(name: &'static str, writer: SegmentedWriter, live: Option<(Arc<Live>, transcript::Track)>, report_gaps: bool) -> Self {
        Self { name, writer: Some(writer), resampler: None, error: None, live, watch: None, peak: Arc::new(AtomicU32::new(0)), report_gaps, gaps: Vec::new() }
    }

    fn set_rate(&mut self, rate: u32) {
        self.resampler = StreamResampler::new(rate).map_err(|e| self.error = Some(e.to_string())).ok();
        self.watch = Some(RateWatch::new(rate));
    }

    /// `now`: chwila odbioru porcji w wątku audio (s od startu nagrania), nie chwila zapisu.
    fn push(&mut self, now: f64, samples: &[f32]) {
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
        let expected = (now * RATE as f64) as u64;
        let after = w.samples_written() + out.len() as u64;
        if expected > after + GAP_TOLERANCE && w.samples_written() > 0 && self.report_gaps {
            let (start, len) = (w.samples_written() as f64 / RATE as f64, (expected - after) as f64 / RATE as f64);
            if len >= 1.0 {
                log::warn!("Przerwa w dostawie dźwięku ({}): {len:.1} s od {start:.1} s", self.name);
            }
            self.gaps.push((start, len));
        }
        let result = if expected > after + GAP_TOLERANCE { w.append_silence(expected - after) } else { Ok(()) }.and_then(|_| w.append(&out));
        if let Err(e) = result {
            // Pełny dysk itp. — zapamiętaj pierwszy błąd, pokażemy go przy zatrzymaniu.
            if self.error.is_none() {
                log::error!("zapis ścieżki: {e}");
                self.error = Some(e.to_string());
            }
        }
    }

    /// Dopełnia ścieżkę ciszą do długości nagrania. Gdy brakuje dużo, źródło urwało się
    /// i już nie wróciło — to też przerwa (najgorsza: aż do końca).
    fn pad_to(&mut self, expected: u64) {
        let Some(w) = self.writer.as_mut() else { return };
        let have = w.samples_written();
        if expected <= have {
            return;
        }
        if self.report_gaps && have > 0 && expected > have + GAP_TOLERANCE {
            let (start, len) = (have as f64 / RATE as f64, (expected - have) as f64 / RATE as f64);
            log::warn!("Brak dźwięku do końca nagrania ({}): {len:.1} s od {start:.1} s", self.name);
            self.gaps.push((start, len));
        }
        let _ = w.append_silence(expected - have);
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

/// Wiadomość z wątku audio do wątku zapisu: porcja ścieżki z chwilą odbioru (s od startu).
enum Msg {
    Audio(transcript::Track, f64, Vec<f32>),
    Stop,
}

/// Co wątki audio wiedzą o dostawach źródła — czyta nadzór (atomowo, bez blokad).
struct Flow {
    /// ms od startu nagrania przy ostatnim buforze; 0 = jeszcze nic.
    last_ms: AtomicU64,
    /// Od kiedy (ms) przychodzą same zera; `u64::MAX` = ostatni bufor nie był cichy.
    zero_since_ms: AtomicU64,
}

impl Flow {
    fn new() -> Arc<Self> {
        Arc::new(Self { last_ms: AtomicU64::new(0), zero_since_ms: AtomicU64::new(u64::MAX) })
    }
}

/// Odbiorca próbek dla wątku audio: tylko kopia do kolejki i znacznik dostawy.
fn audio_sink(kind: transcript::Track, started: Instant, tx: Sender<Msg>, flow: Arc<Flow>) -> impl FnMut(&[f32]) + Send + 'static {
    move |samples: &[f32]| {
        if samples.is_empty() {
            return;
        }
        let now = started.elapsed();
        let ms = (now.as_millis() as u64).max(1);
        flow.last_ms.store(ms, Ordering::Relaxed);
        if samples.iter().all(|&s| s == 0.0) {
            let _ = flow.zero_since_ms.compare_exchange(u64::MAX, ms, Ordering::Relaxed, Ordering::Relaxed);
        } else {
            flow.zero_since_ms.store(u64::MAX, Ordering::Relaxed);
        }
        let _ = tx.send(Msg::Audio(kind, now.as_secs_f64(), samples.to_vec()));
    }
}

/// Stan, który nadzór zgłasza w oknie nagrywania.
#[derive(Default)]
struct Warnings {
    /// Dźwięku aplikacji nie da się nagrywać (system, uprawnienie) — stały komunikat.
    system_unavailable: Option<String>,
    system_lost: bool,
    mic_lost: bool,
    mic_silent: bool,
}

impl Warnings {
    fn text(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(self.system_unavailable.clone());
        if self.system_lost {
            parts.push("Dźwięk rozmówców się urwał — wznawiam nagrywanie…".into());
        }
        if self.mic_lost {
            parts.push("Mikrofon się urwał — wznawiam nagrywanie…".into());
        }
        if self.mic_silent {
            parts.push("Mikrofon nagrywa samą ciszę — sprawdź, czy nie jest wyciszony.".into());
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

/// Źródła dźwięku i to, czego potrzeba, żeby je odtworzyć po awarii.
struct Sources {
    started: Instant,
    tx: Sender<Msg>,
    input_device: Option<String>,
    mic: Mutex<Option<InputCapture>>,
    system: Mutex<Option<SystemCapture>>,
    mic_flow: Arc<Flow>,
    sys_flow: Arc<Flow>,
    mic_track: Arc<Mutex<Track>>,
    sys_track: Arc<Mutex<Track>>,
    warnings: Mutex<Warnings>,
}

impl Sources {
    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn start_mic(&self) -> Result<String> {
        let sink = audio_sink(transcript::Track::Mic, self.started, self.tx.clone(), self.mic_flow.clone());
        let capture = InputCapture::start(self.input_device.as_deref(), Box::new(sink))?;
        self.mic_track.lock().unwrap().set_rate(capture.sample_rate);
        let name = capture.device_name.clone();
        *self.mic.lock().unwrap() = Some(capture);
        Ok(name)
    }

    fn start_system(&self) -> Result<()> {
        let sink = audio_sink(transcript::Track::System, self.started, self.tx.clone(), self.sys_flow.clone());
        let capture = SystemCapture::start(Box::new(sink))?;
        self.sys_track.lock().unwrap().set_rate(capture.sample_rate);
        *self.system.lock().unwrap() = Some(capture);
        Ok(())
    }

    fn stop_mic(&self) {
        if let Some(m) = self.mic.lock().unwrap().take() {
            m.stop();
        }
    }

    fn stop_system(&self) {
        if let Some(c) = self.system.lock().unwrap().take() {
            c.stop();
        }
    }
}

/// Pilnuje jednego źródła: kiedy ostatnio je (od)tworzono i ile razy z rzędu się nie udało.
struct Watch {
    since_ms: u64,
    last_try_ms: u64,
    retries: u32,
}

impl Watch {
    fn new(now_ms: u64) -> Self {
        Self { since_ms: now_ms, last_try_ms: 0, retries: 0 }
    }

    /// Ile ms bez dostaw (licząc od ostatniego bufora albo od (od)tworzenia źródła).
    fn idle_ms(&self, flow: &Flow, now_ms: u64) -> u64 {
        now_ms.saturating_sub(flow.last_ms.load(Ordering::Relaxed).max(self.since_ms))
    }

    fn may_retry(&self, now_ms: u64) -> bool {
        self.retries < MAX_RETRIES && now_ms.saturating_sub(self.last_try_ms) >= RETRY_MS
    }
}

/// Powód odtworzenia źródła.
struct Problem {
    why: &'static str,
    /// Dźwięk płynął i się urwał (ostrzeżenie w oknie nagrywania). Źródło, które jeszcze nic nie
    /// dało (macOS czeka na zgodę), nie jest „urwane”.
    dropout: bool,
}

/// Dlaczego źródło trzeba utworzyć od nowa (`None` = działa).
fn system_problem(src: &Sources, watch: &Watch, now_ms: u64) -> Option<Problem> {
    let guard = src.system.lock().unwrap();
    let Some(c) = guard.as_ref() else { return Some(Problem { why: "przechwytywanie nie działa", dropout: true }) };
    // Gdzie cisza = brak buforów (Windows), brak dostaw niczego nie mówi.
    if !system::DELIVERS_IN_SILENCE {
        return None;
    }
    if c.buffers_received() == 0 {
        // Tap utworzony przed zgodą użytkownika nie oddaje nic (macOS).
        let waited = now_ms.saturating_sub(watch.since_ms) > 4_000;
        return waited.then_some(Problem { why: "nie oddaje dźwięku", dropout: false });
    }
    if watch.idle_ms(&src.sys_flow, now_ms) > STALL_MS {
        return Some(Problem { why: "dostawy ustały", dropout: true });
    }
    c.device_changed().then_some(Problem { why: "zmieniło się wyjście dźwięku", dropout: true })
}

fn mic_problem(src: &Sources, watch: &Watch, now_ms: u64, check_default: bool) -> Option<&'static str> {
    let guard = src.mic.lock().unwrap();
    let Some(m) = guard.as_ref() else { return Some("przechwytywanie nie działa") };
    if m.failed() {
        return Some("strumień zgłosił błąd");
    }
    if watch.idle_ms(&src.mic_flow, now_ms) > STALL_MS {
        return Some("dostawy ustały");
    }
    // Gdy nagrywamy z domyślnego mikrofonu (nie wybrano innego albo wybranego nie ma),
    // idziemy za zmianą domyślnego — np. po podłączeniu słuchawek.
    let follows_default = src.input_device.as_deref().is_none_or(|d| d != m.device_name);
    if check_default && follows_default && capture::default_input_name().is_some_and(|d| d != m.device_name) {
        return Some("zmienił się domyślny mikrofon");
    }
    None
}

/// Nadzór źródeł (co 0,5 s): odtwarza te, które się urwały, i ustawia ostrzeżenia.
fn supervise(src: Arc<Sources>, system_wanted: bool, stop: Arc<AtomicBool>) {
    let mut sys = Watch::new(0);
    let mut mic = Watch::new(0);
    let mut tick = 0u64;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(500));
        tick += 1;
        let now = src.now_ms();

        if system_wanted {
            match system_problem(&src, &sys, now) {
                Some(Problem { why, dropout }) if sys.may_retry(now) => {
                    sys.retries += 1;
                    sys.last_try_ms = now;
                    log::warn!("Dźwięk rozmówców: {why} — tworzę przechwytywanie od nowa (próba {})", sys.retries);
                    if dropout {
                        src.warnings.lock().unwrap().system_lost = true;
                    }
                    src.stop_system();
                    match src.start_system() {
                        Ok(()) => sys.since_ms = src.now_ms(),
                        Err(e) => log::error!("Dźwięk rozmówców: {e}"),
                    }
                }
                Some(_) => {}
                None if sys.retries > 0 && sys.idle_ms(&src.sys_flow, now) < 1_000 => {
                    log::info!("Dźwięk rozmówców wrócił");
                    sys.retries = 0;
                    src.warnings.lock().unwrap().system_lost = false;
                }
                None => {}
            }
        }

        match mic_problem(&src, &mic, now, tick.is_multiple_of(4)) {
            Some(why) if mic.may_retry(now) => {
                mic.retries += 1;
                mic.last_try_ms = now;
                log::warn!("Mikrofon: {why} — tworzę przechwytywanie od nowa (próba {})", mic.retries);
                src.warnings.lock().unwrap().mic_lost = true;
                src.stop_mic();
                match src.start_mic() {
                    Ok(name) => {
                        mic.since_ms = src.now_ms();
                        log::info!("Mikrofon: {name}");
                    }
                    Err(e) => log::error!("Mikrofon: {e}"),
                }
            }
            Some(_) => {}
            None if mic.retries > 0 && mic.idle_ms(&src.mic_flow, now) < 1_000 => {
                log::info!("Mikrofon wrócił");
                mic.retries = 0;
                src.warnings.lock().unwrap().mic_lost = false;
            }
            None => {}
        }

        let zero_since = src.mic_flow.zero_since_ms.load(Ordering::Relaxed);
        let silent = zero_since != u64::MAX && now.saturating_sub(zero_since) > SILENT_MIC_MS;
        let mut w = src.warnings.lock().unwrap();
        if silent != w.mic_silent {
            if silent {
                log::warn!("Mikrofon daje same zera od {} s", zero_since / 1000);
            }
            w.mic_silent = silent;
        }
    }
}

/// Wątek zapisu: przelicza i zapisuje porcje w kolejności odbioru, aż do `Msg::Stop`.
fn write_loop(rx: mpsc::Receiver<Msg>, started: Instant, mic_track: Arc<Mutex<Track>>, sys_track: Arc<Mutex<Track>>) {
    let mut lagging = false;
    for msg in rx {
        let Msg::Audio(kind, at, samples) = msg else { break };
        let lag = started.elapsed().as_secs_f64() - at;
        if lag > 2.0 && !lagging {
            log::warn!("Zapis nagrania nie nadąża (opóźnienie {lag:.1} s) — dźwięk czeka w pamięci");
        }
        // Ostrzegamy po przekroczeniu 2 s, ponownie dopiero po zejściu poniżej 0,5 s.
        lagging = if lagging { lag > 0.5 } else { lag > 2.0 };
        let track = if kind == transcript::Track::Mic { &mic_track } else { &sys_track };
        track.lock().unwrap().push(at, &samples);
    }
}

struct Active {
    meeting: Meeting,
    sources: Arc<Sources>,
    stop_monitor: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    live: Option<Arc<Live>>,
}

/// Odbiorca zdarzeń transkrypcji na żywo: (id spotkania, zdarzenie).
pub type LiveListener = Box<dyn Fn(&str, live::Event) + Send + Sync + 'static>;

#[derive(Default)]
pub struct Recorder {
    active: Mutex<Option<Active>>,
}

impl Recorder {
    pub fn status(&self) -> RecordingStatus {
        let guard = self.active.lock().unwrap();
        match guard.as_ref() {
            Some(a) => {
                let src = &a.sources;
                let peak = |t: &Arc<Mutex<Track>>| f32::from_bits(t.lock().unwrap().peak.swap(0, Ordering::Relaxed));
                RecordingStatus {
                    recording: true,
                    meeting_id: Some(a.meeting.id.clone()),
                    seconds: src.started.elapsed().as_secs_f64(),
                    has_system_audio: src.system.lock().unwrap().is_some(),
                    warning: src.warnings.lock().unwrap().text(),
                    mic_level: peak(&src.mic_track),
                    system_level: peak(&src.sys_track),
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
        let mic_track = Arc::new(Mutex::new(Track::new("mikrofon", SegmentedWriter::new(&audio, MIC)?, tee(transcript::Track::Mic), true)));
        let sys_track = Arc::new(Mutex::new(Track::new(
            "rozmówcy",
            SegmentedWriter::new(&audio, SYSTEM)?,
            tee(transcript::Track::System),
            system::DELIVERS_IN_SILENCE,
        )));

        let (tx, rx) = mpsc::channel();
        let writer = {
            let (m, s) = (mic_track.clone(), sys_track.clone());
            std::thread::Builder::new().name("meeting-writer".into()).spawn(move || write_loop(rx, started, m, s))?
        };
        let sources = Arc::new(Sources {
            started,
            tx,
            input_device: input_device.map(str::to_string),
            mic: Mutex::new(None),
            system: Mutex::new(None),
            mic_flow: Flow::new(),
            sys_flow: Flow::new(),
            mic_track,
            sys_track,
            warnings: Mutex::new(Warnings { system_unavailable: system_available.clone().err(), ..Default::default() }),
        });
        let stop_writer = |sources: &Sources, writer: JoinHandle<()>| {
            let _ = sources.tx.send(Msg::Stop);
            let _ = writer.join();
        };

        let mic_name = match sources.start_mic() {
            Ok(name) => name,
            Err(e) => {
                stop_writer(&sources, writer);
                let _ = store.delete(&meeting.id);
                return Err(anyhow!("Mikrofon: {e}"));
            }
        };
        let mut system_wanted = false;
        if system_available.is_ok() {
            match sources.start_system() {
                Ok(()) => system_wanted = true,
                Err(e) => {
                    log::warn!("{e}");
                    sources.warnings.lock().unwrap().system_unavailable = Some(format!("{e} Nagrywam tylko mikrofon."));
                }
            }
        }

        let stop_monitor = Arc::new(AtomicBool::new(false));
        let monitor = {
            let (src, stop) = (sources.clone(), stop_monitor.clone());
            std::thread::Builder::new().name("meeting-monitor".into()).spawn(move || supervise(src, system_wanted, stop))?
        };

        log::info!("Nagrywanie spotkania {} (mikrofon: {mic_name})", meeting.id);
        *guard = Some(Active { meeting: meeting.clone(), sources, stop_monitor, monitor: Some(monitor), writer: Some(writer), live });
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
        let src = a.sources.clone();
        src.stop_mic();
        let had_system = src.system.lock().unwrap().is_some();
        src.stop_system();
        let seconds = src.started.elapsed().as_secs_f64();
        // Źródła już nic nie wysyłają — wątek zapisu dopisuje to, co czeka w kolejce, i kończy.
        let _ = src.tx.send(Msg::Stop);
        if let Some(w) = a.writer.take() {
            let _ = w.join();
        }
        let expected = (seconds * RATE as f64) as u64;
        let mut errors = Vec::new();
        let mut gaps = Vec::new();
        for (track, kind) in [(&src.mic_track, transcript::Track::Mic), (&src.sys_track, transcript::Track::System)] {
            let mut t = track.lock().unwrap();
            t.pad_to(expected);
            if let Err(e) = t.finish() {
                errors.push(e.to_string());
            }
            if let Some(e) = t.error.take() {
                errors.push(e);
            }
            gaps.extend(t.gaps.iter().filter(|g| g.1 >= REPORTED_GAP_S).map(|&(start, seconds)| AudioGap { track: kind, start, seconds }));
        }
        if !gaps.is_empty() {
            log::warn!("Spotkanie {}: {} przerw(y) w nagraniu, razem {:.0} s", a.meeting.id, gaps.len(), gaps.iter().map(|g| g.seconds).sum::<f64>());
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
            m.audio_gaps = gaps;
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
    use super::*;

    fn tmp() -> std::path::PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("dx-rec-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn track(dir: &std::path::Path, report: bool) -> Track {
        let mut t = Track::new("mikrofon", SegmentedWriter::new(dir, MIC).unwrap(), None, report);
        t.set_rate(RATE);
        t
    }

    /// Porcje po 10 ms od `from` do `to` (s), jak z wątku audio.
    fn feed_track(t: &mut Track, from: f64, to: f64) {
        let mut at = from;
        while at < to {
            at += 0.01;
            t.push(at, &[0.1; 160]);
        }
    }

    #[test]
    fn a_gap_in_deliveries_is_padded_and_reported() {
        let dir = tmp();
        let mut t = track(&dir, true);
        feed_track(&mut t, 0.0, 2.0);
        feed_track(&mut t, 6.0, 8.0); // 4 s bez dostaw
        assert_eq!(t.gaps.len(), 1, "{:?}", t.gaps);
        let (start, len) = t.gaps[0];
        assert!((start - 2.0).abs() < 0.1 && (len - 4.0).abs() < 0.1, "{start} {len}");
        // Ścieżka dalej trzyma się zegara nagrania.
        let written = t.writer.as_ref().unwrap().samples_written() as f64 / RATE as f64;
        assert!((written - 8.0).abs() < 0.1, "{written}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_source_that_never_came_back_is_a_gap_until_the_end() {
        let dir = tmp();
        let mut t = track(&dir, true);
        feed_track(&mut t, 0.0, 2.0);
        t.pad_to(10 * RATE as u64);
        assert_eq!(t.gaps.len(), 1);
        let (start, len) = t.gaps[0];
        assert!((start - 2.0).abs() < 0.1 && (len - 8.0).abs() < 0.1, "{start} {len}");
        // Zwykła końcówka (ostatnie bufory w drodze) to nie przerwa.
        let mut t = track(&dir, true);
        feed_track(&mut t, 0.0, 2.0);
        t.pad_to(2 * RATE as u64 + RATE as u64 / 10);
        assert!(t.gaps.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn silence_without_deliveries_is_not_a_gap_where_that_is_normal() {
        let dir = tmp();
        let mut t = track(&dir, false); // WASAPI loopback w ciszy nie wysyła nic
        feed_track(&mut t, 0.0, 1.0);
        feed_track(&mut t, 5.0, 6.0);
        assert!(t.gaps.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn late_first_samples_are_not_a_gap() {
        let dir = tmp();
        let mut t = track(&dir, true);
        feed_track(&mut t, 3.0, 4.0); // źródło ruszyło po 3 s (np. tap po zgodzie)
        assert!(t.gaps.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn writer_thread_keeps_receipt_times_even_when_it_runs_late() {
        let dir = tmp();
        let mic = Arc::new(Mutex::new(track(&dir, true)));
        let sys = Arc::new(Mutex::new(Track::new("rozmówcy", SegmentedWriter::new(&dir, SYSTEM).unwrap(), None, true)));
        sys.lock().unwrap().set_rate(RATE);
        let (tx, rx) = mpsc::channel();
        // Wszystko trafia do kolejki, zanim wątek zapisu w ogóle ruszy (dysk „stał”).
        for i in 1..=300 {
            tx.send(Msg::Audio(transcript::Track::Mic, i as f64 * 0.01, vec![0.1; 160])).unwrap();
        }
        tx.send(Msg::Stop).unwrap();
        let (m, s2) = (mic.clone(), sys.clone());
        let started = Instant::now() - Duration::from_secs(10);
        std::thread::spawn(move || write_loop(rx, started, m, s2)).join().unwrap();
        let t = mic.lock().unwrap();
        assert!(t.gaps.is_empty(), "opóźniony zapis to nie przerwa: {:?}", t.gaps);
        assert!((t.writer.as_ref().unwrap().samples_written() as i64 - 48_000).abs() < 400);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn audio_sink_marks_deliveries_and_digital_silence() {
        let (tx, rx) = mpsc::channel();
        let flow = Flow::new();
        let mut sink = audio_sink(transcript::Track::Mic, Instant::now() - Duration::from_secs(5), tx, flow.clone());
        sink(&[0.0; 10]);
        let first_zero = flow.zero_since_ms.load(Ordering::Relaxed);
        assert!(first_zero >= 5_000 && first_zero != u64::MAX);
        sink(&[0.0; 10]);
        assert_eq!(flow.zero_since_ms.load(Ordering::Relaxed), first_zero, "początek ciszy się nie przesuwa");
        sink(&[0.2; 10]);
        assert_eq!(flow.zero_since_ms.load(Ordering::Relaxed), u64::MAX);
        assert!(flow.last_ms.load(Ordering::Relaxed) >= 5_000);
        assert_eq!(rx.try_iter().count(), 3);
    }

    #[test]
    fn watch_counts_idle_time_from_the_last_buffer_or_restart() {
        let flow = Flow::new();
        let mut w = Watch::new(1_000);
        assert_eq!(w.idle_ms(&flow, 1_500), 500, "bez buforów liczymy od (od)tworzenia");
        flow.last_ms.store(4_000, Ordering::Relaxed);
        assert_eq!(w.idle_ms(&flow, 8_000), 4_000);
        assert!(w.may_retry(8_000));
        w.last_try_ms = 8_000;
        assert!(!w.may_retry(9_000), "nie częściej niż co RETRY_MS");
        w.retries = MAX_RETRIES;
        assert!(!w.may_retry(20_000));
    }

    /// Prawdziwy mikrofon i tap systemowy: w trakcie nagrania oba źródła „milkną” (podmieniamy je
    /// na przechwytywanie, którego dźwięk nigdzie nie trafia), a nadzór ma je odtworzyć sam.
    /// Puszcza cicho mowę z `say`. `cargo test --lib -- --ignored recovers_from --nocapture`.
    #[test]
    #[ignore]
    fn real_recording_recovers_from_silent_sources() {
        let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
        let store = Store { root: tmp() };
        let rec = Recorder::default();
        let meeting = rec.start(&store, None, None).unwrap();
        let mut say = std::process::Command::new("say")
            .args(["-v", "Zosia", "[[volm 0.15]] Raz dwa trzy, próba nagrywania spotkania. Mówię dalej, żeby było co nagrać przez dłuższą chwilę, a potem jeszcze trochę."])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_secs(3));
        {
            let guard = rec.active.lock().unwrap();
            let src = &guard.as_ref().unwrap().sources;
            // Podmiana pod jedną blokadą — nadzór nie może zobaczyć chwili bez źródła.
            let fake = SystemCapture::start(Box::new(|_: &[f32]| {})).unwrap();
            let old = src.system.lock().unwrap().replace(fake);
            if let Some(old) = old {
                old.stop();
            }
            let fake = InputCapture::start(None, Box::new(|_: &[f32]| {})).unwrap();
            let old = src.mic.lock().unwrap().replace(fake);
            if let Some(old) = old {
                old.stop();
            }
        }
        std::thread::sleep(Duration::from_millis(1500));
        println!("ostrzeżenie w trakcie: {:?}", rec.status().warning);
        std::thread::sleep(Duration::from_secs(6));
        let status = rec.status();
        println!("ostrzeżenie po odtworzeniu: {:?}", status.warning);
        let m = rec.stop(&store).unwrap();
        let _ = say.kill();
        let _ = say.wait();
        println!("{:?}", m.audio_gaps);
        let mic_gap = m.audio_gaps.iter().find(|g| g.track == transcript::Track::Mic).expect("przerwa mikrofonu");
        let sys_gap = m.audio_gaps.iter().find(|g| g.track == transcript::Track::System).expect("przerwa rozmówców");
        for g in [mic_gap, sys_gap] {
            assert!((2.5..=5.0).contains(&g.start) && (2.0..=5.0).contains(&g.seconds), "{g:?}");
        }
        assert!(status.warning.is_none(), "po odtworzeniu ostrzeżenie znika: {:?}", status.warning);
        let dir = store.audio_folder(&meeting.id);
        let (mic, sys) = (super::super::writer::track_duration_samples(&dir, MIC), super::super::writer::track_duration_samples(&dir, SYSTEM));
        assert!((mic as i64 - sys as i64).abs() < RATE as i64 / 10, "ścieżki równej długości: {mic} {sys}");
        // Po odtworzeniu tap znowu łapie mowę z `say`.
        let mut tail = Vec::new();
        super::super::writer::read_track(&dir, SYSTEM, 1, |at, chunk| {
            if at as f64 / RATE as f64 >= 7.0 {
                tail.extend_from_slice(chunk);
            }
            Ok(())
        })
        .unwrap();
        let rms = crate::audio::rms(&tail);
        println!("RMS rozmówców po odtworzeniu: {rms:.4}");
        assert!(rms > 0.001, "tap po odtworzeniu nie łapie dźwięku");
        std::fs::remove_dir_all(&store.root).ok();
    }

    #[test]
    fn warnings_are_combined() {
        let w = Warnings::default();
        assert_eq!(w.text(), None);
        let w = Warnings { system_lost: true, mic_silent: true, ..Default::default() };
        let text = w.text().unwrap();
        assert!(text.contains("rozmówców") && text.contains("wyciszony"), "{text}");
    }

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
