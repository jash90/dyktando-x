//! Meeting recording: microphone (cpal) + system audio, each track through a resampler to
//! 16 kHz and written to disk (nothing grows in RAM). Tracks are kept aligned to the recording
//! clock: when one starts later or has a gap (e.g. a tap recreated after the user grants
//! permission), the gap is filled with silence — the timestamps of both tracks match.
//! Optionally the 16 kHz samples of both tracks also go to live transcription (`live`).
//!
//! Audio threads only copy samples to a queue (with the time of receipt) — resampling, writing to
//! disk and live transcription are done by a separate thread, so a momentary disk hiccup loses no audio.
//! A supervisor watches both sources: when deliveries stop, the stream reports an error or the
//! device changes (e.g. Bluetooth headphones), the capture is recreated. Gaps that couldn't be
//! avoided go to the log and to `Meeting::audio_gaps`.
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
use crate::i18n;

/// A gap larger than this (in 16 kHz samples) is filled with silence.
const GAP_TOLERANCE: u64 = RATE as u64 / 2;
/// A pause in deliveries this long (ms) means the source has dropped out.
const STALL_MS: u64 = 3_000;
/// Minimum interval (ms) between successive attempts to recreate the same source.
const RETRY_MS: u64 = 3_000;
const MAX_RETRIES: u32 = 30;
/// A microphone producing only zeros for this long (ms) is most likely muted in the system.
const SILENT_MIC_MS: u64 = 10_000;
/// Gaps shorter than this (s) don't go to `Meeting::audio_gaps`.
const REPORTED_GAP_S: f64 = 2.0;

/// Checks whether the source really delivers as many samples per second as it reported. When the
/// reported rate is wrong (e.g. macOS: 48 kHz tap, 16 kHz output), the resampler loses part of the
/// audio and gap filling keeps inserting silence — speech becomes unrecognisable. Pure logic:
/// the caller supplies the time.
pub struct RateWatch {
    declared: u32,
    since: Option<f64>,
    last: f64,
    frames: u64,
}

impl RateWatch {
    /// Measurement window (s).
    const WINDOW: f64 = 3.0;
    /// A pause in deliveries longer than this restarts the measurement (WASAPI loopback sends
    /// nothing in silence — that's not a wrong rate).
    const STALL: f64 = 0.25;
    const TOLERANCE: f64 = 0.08;
    const STANDARD: [u32; 9] = [8_000, 11_025, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 96_000];

    pub fn new(declared: u32) -> Self {
        Self { declared, since: None, last: 0.0, frames: 0 }
    }

    /// `frames` samples (mono, before resampling) arrived at time `now` (s). Returns the new rate
    /// if the measured one clearly differs from the reported one.
    pub fn push(&mut self, frames: usize, now: f64) -> Option<u32> {
        let since = match self.since {
            Some(s) if (0.0..=Self::STALL).contains(&(now - self.last)) => s,
            _ => {
                // The first chunk (or the one after a pause) marks the start of the window — its samples aren't counted.
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
    /// "microphone" / "participants" — for logs.
    name: &'static str,
    writer: Option<SegmentedWriter>,
    resampler: Option<StreamResampler>,
    error: Option<String>,
    live: Option<(Arc<Live>, transcript::Track)>,
    watch: Option<RateWatch>,
    /// Peak RMS since the last read (f32 bits) — the "signal present" indicator in the live window.
    peak: Arc<AtomicU32>,
    /// Whether gaps are lost audio (WASAPI loopback sends nothing in silence — there a gap is silence).
    report_gaps: bool,
    /// Gaps in the middle of the recording: (start s, length s).
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

    /// `now`: time the chunk was received in the audio thread (s from the start of recording), not the write time.
    fn push(&mut self, now: f64, samples: &[f32]) {
        if let Some(rate) = self.watch.as_mut().and_then(|w| w.push(samples.len(), now)) {
            log::warn!("The source delivers {rate} Hz instead of the declared rate — switching the resampling");
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
                log::warn!("Gap in audio delivery ({}): {len:.1} s from {start:.1} s", self.name);
            }
            self.gaps.push((start, len));
        }
        let result = if expected > after + GAP_TOLERANCE { w.append_silence(expected - after) } else { Ok(()) }.and_then(|_| w.append(&out));
        if let Err(e) = result {
            // Disk full etc. — remember the first error, we'll show it on stop.
            if self.error.is_none() {
                log::error!("writing the track: {e}");
                self.error = Some(e.to_string());
            }
        }
    }

    /// Pads the track with silence to the recording length. When a lot is missing, the source
    /// dropped out and never came back — that's a gap too (the worst kind: until the very end).
    fn pad_to(&mut self, expected: u64) {
        let Some(w) = self.writer.as_mut() else { return };
        let have = w.samples_written();
        if expected <= have {
            return;
        }
        if self.report_gaps && have > 0 && expected > have + GAP_TOLERANCE {
            let (start, len) = (have as f64 / RATE as f64, (expected - have) as f64 / RATE as f64);
            log::warn!("No audio until the end of the recording ({}): {len:.1} s from {start:.1} s", self.name);
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
    /// Peak level (RMS) of the microphone and system audio since the previous read.
    pub mic_level: f32,
    pub system_level: f32,
}

/// Message from the audio thread to the writer thread: a track chunk with its receipt time (s from start).
enum Msg {
    Audio(transcript::Track, f64, Vec<f32>),
    Stop,
}

/// What the audio threads know about the source's deliveries — read by the supervisor (atomically, lock-free).
struct Flow {
    /// ms from the start of recording at the last buffer; 0 = nothing yet.
    last_ms: AtomicU64,
    /// Since when (ms) only zeros have been arriving; `u64::MAX` = the last buffer wasn't silent.
    zero_since_ms: AtomicU64,
}

impl Flow {
    fn new() -> Arc<Self> {
        Arc::new(Self { last_ms: AtomicU64::new(0), zero_since_ms: AtomicU64::new(u64::MAX) })
    }
}

/// Sample receiver for the audio thread: just a copy to the queue and a delivery timestamp.
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

/// State the supervisor reports in the recording window.
#[derive(Default)]
struct Warnings {
    /// App audio can't be recorded (system, permission) — a persistent message.
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
            parts.push(i18n::t("recorder.system_lost"));
        }
        if self.mic_lost {
            parts.push(i18n::t("recorder.mic_lost"));
        }
        if self.mic_silent {
            parts.push(i18n::t("recorder.mic_silent"));
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

/// Audio sources and what's needed to recreate them after a failure.
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

/// Watches a single source: when it was last (re)created and how many times in a row it failed.
struct Watch {
    since_ms: u64,
    last_try_ms: u64,
    retries: u32,
}

impl Watch {
    fn new(now_ms: u64) -> Self {
        Self { since_ms: now_ms, last_try_ms: 0, retries: 0 }
    }

    /// How many ms without deliveries (counting from the last buffer or from the source's (re)creation).
    fn idle_ms(&self, flow: &Flow, now_ms: u64) -> u64 {
        now_ms.saturating_sub(flow.last_ms.load(Ordering::Relaxed).max(self.since_ms))
    }

    fn may_retry(&self, now_ms: u64) -> bool {
        self.retries < MAX_RETRIES && now_ms.saturating_sub(self.last_try_ms) >= RETRY_MS
    }
}

/// Reason for recreating a source.
struct Problem {
    why: &'static str,
    /// Audio was flowing and cut off (warning in the recording window). A source that hasn't
    /// delivered anything yet (macOS waiting for permission) isn't "cut off".
    dropout: bool,
}

/// Why the source has to be recreated (`None` = it's working).
fn system_problem(src: &Sources, watch: &Watch, now_ms: u64) -> Option<Problem> {
    let guard = src.system.lock().unwrap();
    let Some(c) = guard.as_ref() else { return Some(Problem { why: "capture isn't running", dropout: true }) };
    // Where silence = no buffers (Windows), a lack of deliveries tells us nothing.
    if !system::DELIVERS_IN_SILENCE {
        return None;
    }
    if c.buffers_received() == 0 {
        // A tap created before the user granted permission delivers nothing (macOS).
        let waited = now_ms.saturating_sub(watch.since_ms) > 4_000;
        return waited.then_some(Problem { why: "delivers no audio", dropout: false });
    }
    if watch.idle_ms(&src.sys_flow, now_ms) > STALL_MS {
        return Some(Problem { why: "deliveries stopped", dropout: true });
    }
    c.device_changed().then_some(Problem { why: "the audio output changed", dropout: true })
}

fn mic_problem(src: &Sources, watch: &Watch, now_ms: u64, check_default: bool) -> Option<&'static str> {
    let guard = src.mic.lock().unwrap();
    let Some(m) = guard.as_ref() else { return Some("capture isn't running") };
    if m.failed() {
        return Some("the stream reported an error");
    }
    if watch.idle_ms(&src.mic_flow, now_ms) > STALL_MS {
        return Some("deliveries stopped");
    }
    // When recording from the default microphone (no other one chosen, or the chosen one is missing),
    // we follow changes of the default — e.g. after headphones are plugged in.
    let follows_default = src.input_device.as_deref().is_none_or(|d| d != m.device_name);
    if check_default && follows_default && capture::default_input_name().is_some_and(|d| d != m.device_name) {
        return Some("the default microphone changed");
    }
    None
}

/// Source supervisor (every 0.5 s): recreates the ones that dropped out and sets warnings.
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
                    log::warn!("Participants' audio: {why} — recreating the capture (attempt {})", sys.retries);
                    if dropout {
                        src.warnings.lock().unwrap().system_lost = true;
                    }
                    src.stop_system();
                    match src.start_system() {
                        Ok(()) => sys.since_ms = src.now_ms(),
                        Err(e) => log::error!("Participants' audio: {e}"),
                    }
                }
                Some(_) => {}
                None if sys.retries > 0 && sys.idle_ms(&src.sys_flow, now) < 1_000 => {
                    log::info!("Participants' audio is back");
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
                log::warn!("Microphone: {why} — recreating the capture (attempt {})", mic.retries);
                src.warnings.lock().unwrap().mic_lost = true;
                src.stop_mic();
                match src.start_mic() {
                    Ok(name) => {
                        mic.since_ms = src.now_ms();
                        log::info!("Microphone: {name}");
                    }
                    Err(e) => log::error!("Microphone: {e}"),
                }
            }
            Some(_) => {}
            None if mic.retries > 0 && mic.idle_ms(&src.mic_flow, now) < 1_000 => {
                log::info!("Microphone is back");
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
                log::warn!("Microphone delivers only zeros since {} s", zero_since / 1000);
            }
            w.mic_silent = silent;
        }
    }
}

/// Writer thread: resamples and writes chunks in order of receipt, until `Msg::Stop`.
fn write_loop(rx: mpsc::Receiver<Msg>, started: Instant, mic_track: Arc<Mutex<Track>>, sys_track: Arc<Mutex<Track>>) {
    let mut lagging = false;
    for msg in rx {
        let Msg::Audio(kind, at, samples) = msg else { break };
        let lag = started.elapsed().as_secs_f64() - at;
        if lag > 2.0 && !lagging {
            log::warn!("Recording writes can't keep up (lag {lag:.1} s) — audio waits in memory");
        }
        // We warn after exceeding 2 s, and again only after dropping below 0.5 s.
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

/// Receiver of live transcription events: (meeting id, event).
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

    /// Utterances transcribed live so far in the ongoing recording (for a window opened mid-recording).
    pub fn live_utterances(&self) -> Vec<Utterance> {
        let guard = self.active.lock().unwrap();
        let mut all = guard.as_ref().and_then(|a| a.live.as_ref()).map(|l| l.utterances()).unwrap_or_default();
        all.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        all
    }

    /// `live`: live transcription config and the receiver of its events; `None` = disabled.
    pub fn start(&self, store: &Store, input_device: Option<&str>, live: Option<(live::Config, LiveListener)>) -> Result<Meeting> {
        let mut guard = self.active.lock().unwrap();
        if guard.is_some() {
            return Err(anyhow!(i18n::t("recorder.already_recording")));
        }
        let system_available = system::availability();
        let meeting = store.create(Local::now(), system_available.is_ok())?;
        let audio = store.audio_folder(&meeting.id);
        let started = Instant::now();
        // Live transcription: a missing model doesn't block recording — the text just won't appear.
        let live = live.and_then(|(config, listener)| {
            let listener = Arc::new(listener);
            let (id, l) = (meeting.id.clone(), listener.clone());
            match Live::start(config, Box::new(move |e| l(&id, e))) {
                Ok(l) => Some(Arc::new(l)),
                Err(e) => {
                    log::warn!("live transcription disabled: {e}");
                    listener(&meeting.id, live::Event::Error(e.to_string()));
                    None
                }
            }
        });
        let tee = |kind| live.as_ref().map(|l| (l.clone(), kind));
        let mic_track = Arc::new(Mutex::new(Track::new("microphone", SegmentedWriter::new(&audio, MIC)?, tee(transcript::Track::Mic), true)));
        let sys_track = Arc::new(Mutex::new(Track::new(
            "participants",
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
                return Err(anyhow!(i18n::t_with("mic.error", &[("error", &e)])));
            }
        };
        let mut system_wanted = false;
        if system_available.is_ok() {
            match sources.start_system() {
                Ok(()) => system_wanted = true,
                Err(e) => {
                    log::warn!("{e}");
                    sources.warnings.lock().unwrap().system_unavailable = Some(i18n::t_with("recorder.mic_only", &[("error", &e)]));
                }
            }
        }

        let stop_monitor = Arc::new(AtomicBool::new(false));
        let monitor = {
            let (src, stop) = (sources.clone(), stop_monitor.clone());
            std::thread::Builder::new().name("meeting-monitor".into()).spawn(move || supervise(src, system_wanted, stop))?
        };

        log::info!("Recording meeting {} (microphone: {mic_name})", meeting.id);
        *guard = Some(Active { meeting: meeting.clone(), sources, stop_monitor, monitor: Some(monitor), writer: Some(writer), live });
        Ok(meeting)
    }

    /// Ends the recording, aligns track lengths and writes `meeting.json` (state `recorded`).
    /// Text from live transcription is saved as a temporary transcript (without speakers);
    /// the full transcription after recording will overwrite it.
    pub fn stop(&self, store: &Store) -> Result<Meeting> {
        let mut a = self.active.lock().unwrap().take().ok_or_else(|| anyhow!(i18n::t("recorder.not_recording")))?;
        a.stop_monitor.store(true, Ordering::Relaxed);
        if let Some(m) = a.monitor.take() {
            let _ = m.join();
        }
        let src = a.sources.clone();
        src.stop_mic();
        let had_system = src.system.lock().unwrap().is_some();
        src.stop_system();
        let seconds = src.started.elapsed().as_secs_f64();
        // Sources no longer send anything — the writer thread appends what's waiting in the queue and exits.
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
            log::warn!("Meeting {}: {} gap(s) in the recording, {:.0} s in total", a.meeting.id, gaps.len(), gaps.iter().map(|g| g.seconds).sum::<f64>());
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
                    log::error!("saving the live transcript: {e}");
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
        log::info!("Recording {} ended ({:.0} s)", meeting.id, seconds);
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

    /// Chunks of 10 ms from `from` to `to` (s), as from the audio thread.
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
        feed_track(&mut t, 6.0, 8.0); // 4 s without deliveries
        assert_eq!(t.gaps.len(), 1, "{:?}", t.gaps);
        let (start, len) = t.gaps[0];
        assert!((start - 2.0).abs() < 0.1 && (len - 4.0).abs() < 0.1, "{start} {len}");
        // The track still follows the recording clock.
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
        // A normal tail (last buffers in flight) is not a gap.
        let mut t = track(&dir, true);
        feed_track(&mut t, 0.0, 2.0);
        t.pad_to(2 * RATE as u64 + RATE as u64 / 10);
        assert!(t.gaps.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn silence_without_deliveries_is_not_a_gap_where_that_is_normal() {
        let dir = tmp();
        let mut t = track(&dir, false); // WASAPI loopback sends nothing in silence
        feed_track(&mut t, 0.0, 1.0);
        feed_track(&mut t, 5.0, 6.0);
        assert!(t.gaps.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn late_first_samples_are_not_a_gap() {
        let dir = tmp();
        let mut t = track(&dir, true);
        feed_track(&mut t, 3.0, 4.0); // the source started after 3 s (e.g. tap after permission)
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
        // Everything lands in the queue before the writer thread even starts (the disk "stalled").
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

    /// Real microphone and system tap: mid-recording both sources "go silent" (we swap them for
    /// a capture whose audio goes nowhere), and the supervisor has to recreate them on its own.
    /// Plays speech quietly with `say`. `cargo test --lib -- --ignored recovers_from --nocapture`.
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
            // Swap under a single lock — the supervisor must not see a moment without a source.
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
        // After recreation the tap picks up the speech from `say` again.
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
        assert!(text.contains("participants'") && text.contains("muted"), "{text}");
    }

    /// Chunks every 10 ms for `secs` seconds, `rate` samples/s; returns the first reported change.
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
        // Deliveries of 0.1 s every 0.5 s (WASAPI in silence) — few samples on average, but not a wrong rate.
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

/// Temporary transcript from live utterances: the same files as after full transcription
/// (`transcript.json`, `transcript.md`), without speaker recognition.
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
