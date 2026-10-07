//! Transkrypcja na żywo w trakcie nagrywania spotkania. Próbki 16 kHz z obu ścieżek trafiają
//! kanałem do osobnego wątku, który dzieli je Silero VAD na wypowiedzi i każdą skończoną od razu
//! przepisuje silnikiem. Wypowiedzi idą do interfejsu (zdarzenie `meeting-live`), a po
//! zatrzymaniu nagrania całość służy jako transkrypt tymczasowy — do czasu pełnej transkrypcji
//! z rozpoznawaniem mówców.
use anyhow::{anyhow, Result};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::transcript::{Track, Utterance, ME, OTHERS};
use super::vad::{Silero, FRAME, FRAME_SECONDS};
use crate::engine::Engine;
use crate::models::{self, AssetId, EngineId};
use crate::settings::Language;

/// Parametry dzielenia na żywo. Krótsza cisza niż w transkrypcji po nagraniu (0,45 s zamiast
/// 0,5 s) i krótsze maksimum: tekst pojawia się szybciej, kosztem rzadszych cięć w pół zdania.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub threshold: f32,
    pub min_speech: f64,
    pub min_silence: f64,
    pub pad: f64,
    pub max_segment: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self { threshold: 0.5, min_speech: 0.25, min_silence: 0.45, pad: 0.2, max_segment: 12.0 }
    }
}

/// Czysta logika: kolejne prawdopodobieństwa ramek → granice wypowiedzi (w ramkach, koniec
/// wyłączny), zwracane w chwili, gdy wypowiedź się kończy.
pub struct Cutter {
    p: Params,
    frame: usize,
    start: Option<usize>,
    silence: usize,
    min_silence: usize,
    max_frames: usize,
}

impl Cutter {
    pub fn new(p: Params) -> Self {
        Self {
            p,
            frame: 0,
            start: None,
            silence: 0,
            min_silence: (p.min_silence / FRAME_SECONDS).round().max(1.0) as usize,
            max_frames: (p.max_segment / FRAME_SECONDS).max(1.0) as usize,
        }
    }

    /// Początek trwającej wypowiedzi (ramka), jeśli jakaś trwa.
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
            if i + 1 - start >= self.max_frames {
                self.start = None;
                return self.accept(start, i + 1);
            }
            return None;
        }
        let start = self.start?;
        self.silence += 1;
        if self.silence < self.min_silence {
            return None;
        }
        let end = i + 1 - self.silence;
        self.start = None;
        self.silence = 0;
        self.accept(start, end)
    }

    /// Koniec nagrania: domyka trwającą wypowiedź.
    pub fn flush(&mut self) -> Option<(usize, usize)> {
        let start = self.start.take()?;
        let end = self.frame - self.silence;
        self.silence = 0;
        self.accept(start, end)
    }
}

pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub samples: Vec<f32>,
}

/// Po tylu ramkach ciszy z rzędu (~1 s) zerujemy stan LSTM — jak w transkrypcji po nagraniu.
const RESET_AFTER_SILENT_FRAMES: usize = 31;

/// Dzielenie jednej ścieżki na żywo: VAD ramka po ramce, bufor próbek tylko od początku
/// trwającej wypowiedzi (z marginesem), więc pamięć nie rośnie z długością spotkania.
pub struct Segmenter {
    vad: Silero,
    cutter: Cutter,
    pad_frames: usize,
    carry: Vec<f32>,
    buf: Vec<f32>,
    buf_frame0: usize,
    frames: usize,
    silent: usize,
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
        })
    }

    fn cut(&self, start: usize, end: usize) -> Segment {
        let s = start.saturating_sub(self.pad_frames).max(self.buf_frame0);
        let e = (end + self.pad_frames).min(self.frames);
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
            }
            self.trim();
        }
        carry.drain(..whole);
        self.carry = carry;
        Ok(out)
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
    /// Kod języka tłumaczenia (np. „en”); `None` = bez tłumaczenia. Tłumaczy zawsze Canary
    /// (jedyny model z tłumaczeniem w obie strony), niezależnie od `engine`.
    pub translate_to: Option<String>,
}

impl Config {
    /// Model, który naprawdę będzie użyty (z tłumaczeniem — Canary).
    pub fn effective_engine(&self) -> EngineId {
        if self.translate_to.is_some() { EngineId::CanaryV2 } else { self.engine }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Utterance(Utterance),
    Error(String),
}

pub type Listener = Box<dyn Fn(Event) + Send + 'static>;

enum Msg {
    Audio(Track, Vec<f32>),
    Stop,
}

pub struct Live {
    tx: Mutex<Option<Sender<Msg>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    utterances: Arc<Mutex<Vec<Utterance>>>,
    pub engine_title: &'static str,
}

impl Live {
    /// Sprawdza modele i startuje wątek (wczytanie modeli trwa parę sekund — w tle, nagranie
    /// nie czeka; próbki z tego czasu czekają w kanale).
    pub fn start(config: Config, listener: Listener) -> Result<Self> {
        let vad = models::asset(AssetId::SileroVad);
        let engine_id = config.effective_engine();
        for needed in [vad, engine_id.asset()] {
            if !needed.is_installed() {
                return Err(anyhow!("Brak modelu {} — pobierz go w Ustawieniach → Modele", needed.title));
            }
        }
        let translate_to = config.translate_to.clone().filter(|t| !t.is_empty());
        if let Some(t) = &translate_to {
            let source = config.language.code().unwrap_or("pl");
            if !(source != t && (source == "en" || t == "en")) {
                return Err(anyhow!("Canary tłumaczy tylko między angielskim a innymi językami (ustawiony język: {source}, docelowy: {t})"));
            }
        }
        let (tx, rx) = mpsc::channel::<Msg>();
        let utterances = Arc::new(Mutex::new(Vec::new()));
        let sink = utterances.clone();
        let vad_path = vad.file_path(0);
        let handle = std::thread::Builder::new().name("live-transcribe".into()).spawn(move || {
            let loaded = (|| -> Result<_> {
                let mic = Segmenter::new(&vad_path, Params::default())?;
                let system = Segmenter::new(&vad_path, Params::default())?;
                let engine = Engine::load(engine_id)?;
                Ok((mic, system, engine))
            })();
            let (mut mic, mut system, mut engine) = match loaded {
                Ok(x) => x,
                Err(e) => {
                    log::error!("transkrypcja na żywo: {e}");
                    listener(Event::Error(e.to_string()));
                    for _ in rx {} // nagranie trwa dalej, tylko bez tekstu na żywo
                    return;
                }
            };
            let mut emit = |seg: Segment, track: Track| match engine.transcribe(&seg.samples, config.language) {
                Ok(text) if !text.is_empty() => {
                    let speaker = if track == Track::Mic { ME } else { OTHERS };
                    // Tłumaczenie to drugi przebieg dekodera po tym samym audio; błąd nie
                    // zabiera oryginału.
                    let translation = translate_to.as_deref().and_then(|t| match engine.translate(&seg.samples, config.language, t) {
                        Ok(tr) if !tr.is_empty() => Some(tr),
                        Ok(_) => None,
                        Err(e) => {
                            log::warn!("tłumaczenie na żywo: {e}");
                            None
                        }
                    });
                    let u = Utterance { start: seg.start, end: seg.end, track, text, speaker: speaker.into(), translation };
                    sink.lock().unwrap().push(u.clone());
                    listener(Event::Utterance(u));
                }
                Ok(_) => {}
                Err(e) => log::warn!("transkrypcja na żywo: {e}"),
            };
            for msg in rx {
                match msg {
                    Msg::Audio(track, samples) => {
                        let seg = if track == Track::Mic { &mut mic } else { &mut system };
                        match seg.push(&samples) {
                            Ok(segments) => segments.into_iter().for_each(|s| emit(s, track)),
                            Err(e) => log::warn!("VAD na żywo: {e}"),
                        }
                    }
                    Msg::Stop => {
                        if let Some(s) = mic.flush() {
                            emit(s, Track::Mic);
                        }
                        if let Some(s) = system.flush() {
                            emit(s, Track::System);
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
            engine_title: engine_id.asset().title,
        })
    }

    /// Próbki 16 kHz mono danej ścieżki (wołane z wątków audio — tylko wysyła do kanału).
    pub fn feed(&self, track: Track, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(Msg::Audio(track, samples.to_vec()));
        }
    }

    pub fn utterances(&self) -> Vec<Utterance> {
        self.utterances.lock().unwrap().clone()
    }

    /// Domyka trwające wypowiedzi, czeka na ich przepisanie i oddaje wszystko po kolei.
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
        assert_eq!(out.len(), 3, "{out:?}");
        assert!(out.iter().all(|(s, e)| (e - s) as f64 * FRAME_SECONDS <= 12.0 + 1e-9));
        assert_eq!(out[0].1, out[1].0);
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

    /// Dwa zdania z przerwą podane „strumieniowo” (porcje po 20 ms) → dwie wypowiedzi na żywo,
    /// druga domknięta dopiero przez `finish`. Wymaga modeli: `cargo test -- --ignored live_`.
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
            Config { engine: EngineId::ParakeetV3, language: Language::Pl, translate_to: None },
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
        assert_eq!(got.lock().unwrap().len(), all.len());
    }

    /// Na żywo z tłumaczeniem pl→en (Canary): `cargo test -- --ignored live_translates`.
    #[test]
    #[ignore]
    fn live_translates_to_english_with_canary() {
        let audio = fixture("fleurs_kobieta.wav");
        let live = Live::start(
            Config { engine: EngineId::ParakeetV3, language: Language::Pl, translate_to: Some("en".into()) },
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
        let bad = Config { engine: EngineId::ParakeetV3, language: Language::Pl, translate_to: Some("de".into()) };
        let err = Live::start(bad, Box::new(|_| {})).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("angielskim") || err.contains("Brak modelu"), "{err}");
    }
}
