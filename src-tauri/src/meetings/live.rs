//! Live transcription while a meeting is being recorded. 16 kHz samples from both tracks go
//! through a channel to a separate thread, which splits them into utterances with Silero VAD and
//! transcribes each finished one right away with the engine. We don't wait for pauses, though: an
//! ongoing utterance is transcribed every ~1.5 s as draft text (`Event::Partial`), and cut at the
//! quietest spot after 8 s at the latest. Utterances go to the UI (the `meeting-live` event), and
//! after recording stops the whole thing serves as a temporary transcript — until the full
//! transcription with speaker recognition.
use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::transcript::{Track, Utterance, ME, OTHERS};
use super::vad::{Silero, FRAME, FRAME_SECONDS};
use crate::engine::Engine;
use crate::i18n;
use crate::models::{self, AssetId, EngineId};
use crate::settings::Language;

/// Live splitting parameters. Shorter silence than in post-recording transcription (0.45 s instead
/// of 0.5 s) and a much shorter maximum: in a fast conversation there are almost no pauses, so an
/// utterance is closed after 8 s at the latest — at the quietest spot of the last 2 s, not mid-word.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub threshold: f32,
    pub min_speech: f64,
    pub min_silence: f64,
    pub pad: f64,
    pub max_segment: f64,
    /// Within how many final seconds of the window we look for a cut point.
    pub cut_search: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self { threshold: 0.5, min_speech: 0.25, min_silence: 0.45, pad: 0.2, max_segment: 8.0, cut_search: 2.0 }
    }
}

/// Pure logic: successive frame probabilities → utterance boundaries (in frames, end
/// exclusive), returned at the moment an utterance ends.
pub struct Cutter {
    p: Params,
    frame: usize,
    start: Option<usize>,
    /// Frame probabilities of the ongoing utterance (from `start`) — for choosing the cut point.
    probs: Vec<f32>,
    silence: usize,
    min_silence: usize,
    max_frames: usize,
    search_frames: usize,
}

impl Cutter {
    pub fn new(p: Params) -> Self {
        let max_frames = (p.max_segment / FRAME_SECONDS).max(2.0) as usize;
        Self {
            p,
            frame: 0,
            start: None,
            probs: Vec::with_capacity(max_frames + 64),
            silence: 0,
            min_silence: (p.min_silence / FRAME_SECONDS).round().max(1.0) as usize,
            max_frames,
            search_frames: ((p.cut_search / FRAME_SECONDS) as usize).clamp(1, max_frames / 2),
        }
    }

    /// Start of the ongoing utterance (frame), if one is in progress.
    pub fn pending_start(&self) -> Option<usize> {
        self.start
    }

    fn accept(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        ((end - start) as f64 * FRAME_SECONDS >= self.p.min_speech).then_some((start, end))
    }

    pub fn push(&mut self, prob: f32) -> Option<(usize, usize)> {
        let i = self.frame;
        self.frame += 1;
        if prob >= self.p.threshold {
            let start = *self.start.get_or_insert(i);
            self.silence = 0;
            self.probs.push(prob);
            if i + 1 - start >= self.max_frames {
                // The quietest frame at the end of the window (on a tie — the latest one); the rest after
                // the cut starts the next utterance, so nothing is lost.
                let from = self.probs.len() - self.search_frames;
                let mut cut = self.probs.len() - 1;
                for k in from..self.probs.len() {
                    if self.probs[k] <= self.probs[cut] {
                        cut = k;
                    }
                }
                let cut = cut.max(1);
                self.probs.drain(..cut);
                self.start = Some(start + cut);
                return self.accept(start, start + cut);
            }
            return None;
        }
        let start = self.start?;
        self.silence += 1;
        self.probs.push(prob);
        if self.silence < self.min_silence {
            return None;
        }
        let end = i + 1 - self.silence;
        self.start = None;
        self.silence = 0;
        self.probs.clear();
        self.accept(start, end)
    }

    /// End of recording: closes the ongoing utterance.
    pub fn flush(&mut self) -> Option<(usize, usize)> {
        let start = self.start.take()?;
        let end = self.frame - self.silence;
        self.silence = 0;
        self.probs.clear();
        self.accept(start, end)
    }
}

pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub samples: Vec<f32>,
}

/// After this many consecutive silent frames (~1 s) we reset the LSTM state — as in post-recording transcription.
const RESET_AFTER_SILENT_FRAMES: usize = 31;

/// Live splitting of a single track: VAD frame by frame, a sample buffer only from the start of
/// the ongoing utterance (with a margin), so memory doesn't grow with the meeting length.
pub struct Segmenter {
    vad: Silero,
    cutter: Cutter,
    pad_frames: usize,
    carry: Vec<f32>,
    buf: Vec<f32>,
    buf_frame0: usize,
    frames: usize,
    silent: usize,
    /// Frame of the last cut forced by length — there adjacent utterances touch without a
    /// margin (with a margin the words at the seam would be transcribed twice).
    joined: Option<usize>,
}

impl Segmenter {
    pub fn new(vad_model: &std::path::Path, p: Params) -> Result<Self> {
        Ok(Self {
            vad: Silero::load(vad_model)?,
            cutter: Cutter::new(p),
            pad_frames: (p.pad / FRAME_SECONDS).round() as usize,
            carry: Vec::with_capacity(FRAME * 2),
            buf: Vec::new(),
            buf_frame0: 0,
            frames: 0,
            silent: 0,
            joined: None,
        })
    }

    fn cut(&self, start: usize, end: usize) -> Segment {
        let left = if self.joined == Some(start) { 0 } else { self.pad_frames };
        let right = if self.cutter.pending_start() == Some(end) { 0 } else { self.pad_frames };
        let s = start.saturating_sub(left).max(self.buf_frame0);
        let e = (end + right).min(self.frames);
        let samples = self.buf[(s - self.buf_frame0) * FRAME..(e - self.buf_frame0) * FRAME].to_vec();
        Segment { start: s as f64 * FRAME_SECONDS, end: e as f64 * FRAME_SECONDS, samples }
    }

    fn trim(&mut self) {
        let keep_from = self.cutter.pending_start().unwrap_or(self.frames).saturating_sub(self.pad_frames).max(self.buf_frame0);
        self.buf.drain(..(keep_from - self.buf_frame0) * FRAME);
        self.buf_frame0 = keep_from;
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<Segment>> {
        let mut out = Vec::new();
        let mut carry = std::mem::take(&mut self.carry);
        carry.extend_from_slice(samples);
        let whole = carry.len() / FRAME * FRAME;
        for frame in carry[..whole].chunks_exact(FRAME) {
            self.buf.extend_from_slice(frame);
            self.frames += 1;
            let p = self.vad.probability(frame)?;
            self.silent = if p < 0.15 { self.silent + 1 } else { 0 };
            if self.silent == RESET_AFTER_SILENT_FRAMES {
                self.vad.reset();
            }
            if let Some((s, e)) = self.cutter.push(p) {
                out.push(self.cut(s, e));
                self.joined = (self.cutter.pending_start() == Some(e)).then_some(e);
            }
            self.trim();
        }
        carry.drain(..whole);
        self.carry = carry;
        Ok(out)
    }

    /// The ongoing (unclosed) utterance, if it's already at least `min_seconds` long — for live
    /// draft text.
    pub fn pending(&self, min_seconds: f64) -> Option<Segment> {
        let start = self.cutter.pending_start()?;
        ((self.frames - start) as f64 * FRAME_SECONDS >= min_seconds).then(|| self.cut(start, self.frames))
    }

    pub fn flush(&mut self) -> Option<Segment> {
        let (s, e) = self.cutter.flush()?;
        let seg = self.cut(s, e);
        self.trim();
        Some(seg)
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub engine: EngineId,
    pub language: Language,
    /// Translation language code (e.g. "en"); `None` = no translation. Canary always translates
    /// (the only model with translation both ways), regardless of `engine`.
    pub translate_to: Option<String>,
    /// Vocabulary of names and terms (a hint for Whisper).
    pub vocabulary: String,
}

impl Config {
    /// The model that will actually be used (with translation — Canary).
    pub fn effective_engine(&self) -> EngineId {
        if self.translate_to.is_some() { EngineId::CanaryV2 } else { self.engine }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    /// A closed utterance (pause or cut after `max_segment`) — goes into the transcript.
    Utterance(Utterance),
    /// Draft text of the ongoing utterance on a given track; replaces that track's previous draft.
    /// Empty text = remove the draft. Doesn't go into the transcript.
    Partial(Utterance),
    Error(String),
}

pub type Listener = Box<dyn Fn(Event) + Send + 'static>;

enum Msg {
    Audio(Track, Vec<f32>),
    Stop,
}

/// Draft text appears when the ongoing utterance is already this many seconds long…
const PARTIAL_MIN_SECONDS: f64 = 1.0;
/// …and refreshes no more often than this (or every 2× the last engine run time).
const PARTIAL_EVERY: Duration = Duration::from_millis(1500);
/// Backlog (samples in the channel) above which we skip draft text — only closed utterances
/// count, so the window doesn't drift away from reality.
const MAX_BACKLOG_FOR_PARTIAL: usize = 2 * 16_000;
/// Translating draft text only when the queue is almost empty (it's a second decoder pass).
const MAX_BACKLOG_FOR_PARTIAL_TRANSLATION: usize = 16_000 / 2;

#[derive(Default)]
struct Draft {
    start: Option<f64>,
    at: Option<Instant>,
    shown: bool,
}

/// A single engine run: text and (optionally) translation; records the run time.
fn run(engine: &mut Engine, samples: &[f32], language: Language, translate_to: Option<&str>, last_run: &mut Duration) -> (Option<String>, Option<String>) {
    let t = Instant::now();
    let text = match engine.transcribe(samples, language) {
        Ok(text) if !super::transcriber::is_noise(&text, samples.len() as f64 / 16_000.0) => text,
        Ok(_) => return (None, None),
        Err(e) => {
            log::warn!("live transcription: {e}");
            return (None, None);
        }
    };
    // Translation is a second decoder pass over the same audio; an error doesn't take away the original.
    let translation = translate_to.and_then(|to| match engine.translate(samples, language, to) {
        Ok(tr) if !tr.is_empty() => Some(tr),
        Ok(_) => None,
        Err(e) => {
            log::warn!("live translation: {e}");
            None
        }
    });
    *last_run = t.elapsed();
    (Some(text), translation)
}

pub struct Live {
    tx: Mutex<Option<Sender<Msg>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    utterances: Arc<Mutex<Vec<Utterance>>>,
    /// Samples sent to the thread but not yet processed.
    queued: Arc<AtomicUsize>,
    pub engine_title: &'static str,
}

impl Live {
    /// Checks the models and starts the thread (loading models takes a few seconds — in the background,
    /// recording doesn't wait; samples from that period wait in the channel).
    pub fn start(config: Config, listener: Listener) -> Result<Self> {
        let vad = models::asset(AssetId::SileroVad);
        let engine_id = config.effective_engine();
        for needed in [vad, engine_id.asset()] {
            if !needed.is_installed() {
                return Err(anyhow!(i18n::t_with("model.missing", &[("model", &needed.label())])));
            }
        }
        let translate_to = config.translate_to.clone().filter(|t| !t.is_empty());
        if let Some(t) = &translate_to {
            let source = config.language.code().unwrap_or("pl");
            if !(source != t && (source == "en" || t == "en")) {
                return Err(anyhow!(i18n::t_with("engine.canary_translation", &[("source", &source), ("target", t)])));
            }
        }
        let (tx, rx) = mpsc::channel::<Msg>();
        let utterances = Arc::new(Mutex::new(Vec::new()));
        let sink = utterances.clone();
        let queued = Arc::new(AtomicUsize::new(0));
        let backlog = queued.clone();
        let vad_path = vad.file_path(0);
        let handle = std::thread::Builder::new().name("live-transcribe".into()).spawn(move || {
            let loaded = (|| -> Result<_> {
                let mic = Segmenter::new(&vad_path, Params::default())?;
                let system = Segmenter::new(&vad_path, Params::default())?;
                let mut engine = Engine::load(engine_id)?;
                engine.set_vocabulary(&config.vocabulary);
                Ok((mic, system, engine))
            })();
            let (mut mic, mut system, mut engine) = match loaded {
                Ok(x) => x,
                Err(e) => {
                    log::error!("live transcription: {e}");
                    listener(Event::Error(e.to_string()));
                    for _ in rx {} // recording continues, just without live text
                    return;
                }
            };
            let language = config.language;
            // Last engine run time — the draft text pace adapts to the hardware.
            let mut last_run = Duration::ZERO;
            let translate_to = translate_to.as_deref();
            let utterance = |seg: &Segment, track: Track, text: String, translation: Option<String>| {
                let speaker = if track == Track::Mic { ME } else { OTHERS };
                Utterance { start: seg.start, end: seg.end, track, text, speaker: speaker.into(), translation }
            };
            let mut drafts = [Draft::default(), Draft::default()];
            let slot = |track: Track| if track == Track::Mic { 0 } else { 1 };
            let finalize = |seg: Segment, track: Track, engine: &mut Engine, drafts: &mut [Draft; 2], last_run: &mut Duration| {
                let d = &mut drafts[slot(track)];
                let had_draft = std::mem::take(d).shown;
                match run(engine, &seg.samples, language, translate_to, last_run) {
                    (Some(text), translation) => {
                        let u = utterance(&seg, track, text, translation);
                        sink.lock().unwrap().push(u.clone());
                        listener(Event::Utterance(u));
                    }
                    _ if had_draft => listener(Event::Partial(utterance(&seg, track, String::new(), None))),
                    _ => {}
                }
            };
            for msg in rx {
                match msg {
                    Msg::Audio(track, samples) => {
                        backlog.fetch_sub(samples.len(), Ordering::Relaxed);
                        let seg = if track == Track::Mic { &mut mic } else { &mut system };
                        match seg.push(&samples) {
                            Ok(segments) => segments.into_iter().for_each(|s| finalize(s, track, &mut engine, &mut drafts, &mut last_run)),
                            Err(e) => log::warn!("live VAD: {e}"),
                        }
                        // Draft text of the ongoing utterance — we don't wait for a pause.
                        let waiting = backlog.load(Ordering::Relaxed);
                        if waiting > MAX_BACKLOG_FOR_PARTIAL {
                            continue;
                        }
                        let Some(pending) = seg.pending(PARTIAL_MIN_SECONDS) else { continue };
                        let d = &mut drafts[slot(track)];
                        let every = PARTIAL_EVERY.max(last_run * 2);
                        let due = d.start != Some(pending.start) || d.at.is_none_or(|at| at.elapsed() >= every);
                        if !due {
                            continue;
                        }
                        d.start = Some(pending.start);
                        d.at = Some(Instant::now());
                        let to = translate_to.filter(|_| waiting <= MAX_BACKLOG_FOR_PARTIAL_TRANSLATION);
                        if let (Some(text), translation) = run(&mut engine, &pending.samples, language, to, &mut last_run) {
                            drafts[slot(track)].shown = true;
                            listener(Event::Partial(utterance(&pending, track, text, translation)));
                        }
                    }
                    Msg::Stop => {
                        if let Some(s) = mic.flush() {
                            finalize(s, Track::Mic, &mut engine, &mut drafts, &mut last_run);
                        }
                        if let Some(s) = system.flush() {
                            finalize(s, Track::System, &mut engine, &mut drafts, &mut last_run);
                        }
                        break;
                    }
                }
            }
        })?;
        Ok(Self {
            tx: Mutex::new(Some(tx)),
            handle: Mutex::new(Some(handle)),
            utterances,
            queued,
            engine_title: engine_id.asset().title,
        })
    }

    /// 16 kHz mono samples of a given track (called from audio threads — only sends to the channel).
    pub fn feed(&self, track: Track, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            self.queued.fetch_add(samples.len(), Ordering::Relaxed);
            if tx.send(Msg::Audio(track, samples.to_vec())).is_err() {
                self.queued.fetch_sub(samples.len(), Ordering::Relaxed);
            }
        }
    }

    pub fn utterances(&self) -> Vec<Utterance> {
        self.utterances.lock().unwrap().clone()
    }

    /// Closes ongoing utterances, waits for them to be transcribed and returns everything in order.
    pub fn finish(&self) -> Vec<Utterance> {
        if let Some(tx) = self.tx.lock().unwrap().take() {
            let _ = tx.send(Msg::Stop);
        }
        if let Some(h) = self.handle.lock().unwrap().take() {
            let _ = h.join();
        }
        let mut all = self.utterances();
        all.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(c: &mut Cutter, pattern: &[(f32, f64)]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (p, secs) in pattern {
            for _ in 0..(secs / FRAME_SECONDS).round() as usize {
                out.extend(c.push(*p));
            }
        }
        out
    }

    #[test]
    fn utterance_ends_after_min_silence_without_trailing_silence() {
        let mut c = Cutter::new(Params::default());
        let out = run(&mut c, &[(0.0, 1.0), (0.9, 2.0), (0.0, 0.45)]);
        assert_eq!(out.len(), 1, "{out:?}");
        let (s, e) = out[0];
        assert!((s as f64 * FRAME_SECONDS - 1.0).abs() < 0.05);
        assert!((e as f64 * FRAME_SECONDS - 3.0).abs() < 0.05, "{e}");
        assert!(c.flush().is_none());
    }

    #[test]
    fn short_pause_does_not_split() {
        let mut c = Cutter::new(Params::default());
        let out = run(&mut c, &[(0.9, 2.0), (0.1, 0.3), (0.9, 2.0), (0.0, 1.0)]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!((out[0].1 - out[0].0) as f64 * FRAME_SECONDS > 4.0);
    }

    #[test]
    fn clicks_are_dropped() {
        let mut c = Cutter::new(Params::default());
        let out = run(&mut c, &[(0.9, 0.1), (0.0, 1.0)]);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn long_speech_is_force_split_at_max() {
        let mut c = Cutter::new(Params::default());
        let out = run(&mut c, &[(0.9, 30.0), (0.0, 1.0)]);
        assert_eq!(out.len(), 4, "{out:?}");
        assert!(out.iter().all(|(s, e)| (e - s) as f64 * FRAME_SECONDS <= 8.0 + 1e-9));
        assert_eq!(out[0].1, out[1].0);
        assert_eq!(out[2].1, out[3].0);
    }

    #[test]
    fn forced_cut_lands_in_the_quietest_frame_of_the_last_two_seconds() {
        let mut c = Cutter::new(Params::default());
        // 6.5 s of speech, a short "dip" (still above the threshold), then speech with no pause.
        let out = run(&mut c, &[(0.95, 6.5), (0.55, 0.1), (0.95, 10.0), (0.0, 1.0)]);
        assert!(out.len() >= 2, "{out:?}");
        let cut = out[0].1 as f64 * FRAME_SECONDS;
        assert!((cut - 6.5).abs() < 0.15, "cięcie w {cut:.2} s: {out:?}");
        assert_eq!(out[0].1, out[1].0, "reszta zaczyna następną wypowiedź");
        assert!(out.iter().all(|(s, e)| (e - s) as f64 * FRAME_SECONDS <= 8.0 + 1e-9));
    }

    #[test]
    fn dips_below_threshold_inside_window_are_preferred() {
        let mut c = Cutter::new(Params::default());
        // A short pause (0.2 s < min_silence) doesn't close the utterance, but it's the best cut point.
        let out = run(&mut c, &[(0.9, 7.0), (0.1, 0.2), (0.9, 5.0), (0.0, 1.0)]);
        let cut = out[0].1 as f64 * FRAME_SECONDS;
        assert!((7.0..7.25).contains(&cut), "cięcie w {cut:.2} s: {out:?}");
    }

    #[test]
    fn flush_closes_pending_utterance_without_counted_silence() {
        let mut c = Cutter::new(Params::default());
        assert!(run(&mut c, &[(0.9, 1.0), (0.0, 0.2)]).is_empty());
        let (s, e) = c.flush().unwrap();
        assert_eq!(s, 0);
        assert!((e as f64 * FRAME_SECONDS - 1.0).abs() < 0.05, "{e}");
        assert!(c.flush().is_none());
    }

    fn fixture(name: &str) -> Vec<f32> {
        let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let mut r = hound::WavReader::open(p).unwrap();
        r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
    }

    /// Two sentences with a pause fed "as a stream" (20 ms chunks) → two live utterances,
    /// the second closed only by `finish`. Requires models: `cargo test -- --ignored live_`.
    #[test]
    #[ignore]
    fn live_streams_two_utterances_from_mic() {
        let (a, b) = (fixture("fleurs_kobieta.wav"), fixture("ja_zosia.wav"));
        let mut audio = a.clone();
        audio.extend(vec![0.00001f32; 16_000]);
        audio.extend_from_slice(&b);
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let live = Live::start(
            Config { engine: EngineId::ParakeetV3, language: Language::Pl, translate_to: None, vocabulary: String::new() },
            Box::new(move |e| g.lock().unwrap().push(e)),
        )
        .unwrap();
        let t = std::time::Instant::now();
        for chunk in audio.chunks(320) {
            live.feed(Track::Mic, chunk);
        }
        let all = live.finish();
        println!("{} wypowiedzi w {:.1} s: {all:#?}", all.len(), t.elapsed().as_secs_f64());
        assert!(all.len() >= 2, "{all:?}");
        assert!(all[0].text.to_lowercase().contains("wizy"), "{all:?}");
        assert!(all.last().unwrap().text.to_lowercase().contains("czwartek"), "{all:?}");
        assert!(all.iter().all(|u| u.speaker == ME));
        assert_eq!(got.lock().unwrap().iter().filter(|e| matches!(e, Event::Utterance(_))).count(), all.len());
    }

    /// Speech glued together with no pause at all (fast conversation) fed in real time: draft text
    /// must appear before the end, and closed utterances must not exceed 8 s.
    /// `cargo test -- --ignored live_`.
    #[test]
    #[ignore]
    fn live_shows_partials_without_any_pause() {
        let mut audio = fixture("fleurs_kobieta.wav");
        audio.extend(fixture("ja_zosia.wav"));
        audio.extend(fixture("fleurs_kobieta.wav"));
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let live = Live::start(
            Config { engine: EngineId::ParakeetV3, language: Language::Pl, translate_to: None, vocabulary: String::new() },
            Box::new(move |e| g.lock().unwrap().push((Instant::now(), e))),
        )
        .unwrap();
        let t = Instant::now();
        for chunk in audio.chunks(320) {
            live.feed(Track::System, chunk);
            std::thread::sleep(Duration::from_millis(20)); // real-time pace
        }
        let fed = Instant::now();
        let all = live.finish();
        let events = got.lock().unwrap();
        let partials: Vec<_> = events.iter().filter(|(at, e)| matches!(e, Event::Partial(u) if !u.text.is_empty()) && *at < fed).collect();
        println!("{:.1} s audio, {} roboczych, {} domkniętych w {:.1} s: {all:#?}", audio.len() as f64 / 16_000.0, partials.len(), all.len(), t.elapsed().as_secs_f64());
        assert!(!partials.is_empty(), "brak tekstu roboczego");
        assert!(all.iter().all(|u| u.end - u.start <= 8.0 + 0.2 + 0.05), "{all:?}");
        let text = all.iter().map(|u| u.text.to_lowercase()).collect::<Vec<_>>().join(" ");
        assert!(text.contains("wizy") && text.contains("czwartek"), "{text}");
    }

    /// Live with pl→en translation (Canary): `cargo test -- --ignored live_translates`.
    #[test]
    #[ignore]
    fn live_translates_to_english_with_canary() {
        let audio = fixture("fleurs_kobieta.wav");
        let live = Live::start(
            Config { engine: EngineId::ParakeetV3, language: Language::Pl, translate_to: Some("en".into()), vocabulary: String::new() },
            Box::new(|_| {}),
        )
        .unwrap();
        assert_eq!(live.engine_title, EngineId::CanaryV2.asset().title);
        let t = std::time::Instant::now();
        for chunk in audio.chunks(320) {
            live.feed(Track::System, chunk);
        }
        let all = live.finish();
        println!("{} wypowiedzi w {:.1} s: {all:#?}", all.len(), t.elapsed().as_secs_f64());
        assert!(!all.is_empty());
        assert!(all[0].text.to_lowercase().contains("wizy"), "{all:?}");
        let tr = all[0].translation.clone().unwrap_or_default().to_lowercase();
        assert!(tr.contains("visa"), "{all:?}");
        assert_eq!(all[0].speaker, OTHERS);
    }

    #[test]
    fn translation_requires_english_on_one_side() {
        let bad = Config { engine: EngineId::ParakeetV3, language: Language::Pl, translate_to: Some("de".into()), vocabulary: String::new() };
        let err = Live::start(bad, Box::new(|_| {})).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("English") || err.contains("is missing"), "{err}");
    }
}
