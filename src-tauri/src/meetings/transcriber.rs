//! Transkrypcja nagranego spotkania: VAD na obu ścieżkach → każdy fragment przez świeżą
//! instancję silnika (nie tę od dyktowania) → wektory głosu na ścieżce „system” → grupowanie
//! mówców → `TranscriptBuilder` → `transcript.md` + `transcript.json`.
use anyhow::{anyhow, Result};
use chrono::Local;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

use super::diarize::{self, Embedder};
use super::store::{Store, MIC, SYSTEM};
use super::transcript::{self, SpeakerSegment, Track, TranscriptDocument, Utterance};
use super::vad;
use crate::engine::Engine;
use crate::models::{self, AssetId, EngineId};
use crate::settings::Language;

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    /// Opis kroku po polsku (do paska postępu).
    pub step: String,
    /// 0..=1
    pub fraction: f32,
}

pub struct Options {
    pub engine: EngineId,
    pub language: Language,
    pub diarize: bool,
}

pub fn transcribe(store: &Store, id: &str, opts: &Options, cancel: &AtomicBool, mut progress: impl FnMut(Progress)) -> Result<TranscriptDocument> {
    let meeting = store.load(id).ok_or_else(|| anyhow!("Brak spotkania {id}"))?;
    if meeting.audio_deleted {
        return Err(anyhow!("Nagranie tego spotkania zostało już usunięte"));
    }
    let audio = store.audio_folder(id);
    let vad_model = models::asset(AssetId::SileroVad);
    for needed in [vad_model, opts.engine.asset()] {
        if !needed.is_installed() {
            return Err(anyhow!("Brak modelu {} — pobierz go w Ustawieniach → Modele", needed.title));
        }
    }
    let speaker_model = models::asset(AssetId::SpeakerModel);
    let diarize = opts.diarize && meeting.has_system_audio && speaker_model.is_installed();
    let is_cancelled = || cancel.load(Ordering::Relaxed);
    let check = || if cancel.load(Ordering::Relaxed) { Err(anyhow!("Przerwano")) } else { Ok(()) };

    // 1) Wykrywanie mowy (szybkie: ~1–2 % czasu nagrania).
    progress(Progress { step: "Wykrywanie mowy (mikrofon)".into(), fraction: 0.0 });
    let mic_segs = vad::segments(&vad::track_probabilities(&vad_model.file_path(0), &audio, MIC, &is_cancelled)?, vad::Params::default());
    let sys_segs = if meeting.has_system_audio {
        progress(Progress { step: "Wykrywanie mowy (rozmówcy)".into(), fraction: 0.03 });
        vad::segments(&vad::track_probabilities(&vad_model.file_path(0), &audio, SYSTEM, &is_cancelled)?, vad::Params::default())
    } else {
        Vec::new()
    };
    check()?;
    let total = (mic_segs.len() + sys_segs.len()).max(1);
    log::info!("Spotkanie {id}: {} fragmentów mikrofonu, {} rozmówców", mic_segs.len(), sys_segs.len());

    // 2) Transkrypcja fragmentów.
    progress(Progress { step: "Wczytywanie modelu".into(), fraction: 0.05 });
    let mut engine = Engine::load(opts.engine)?;
    let mut embedder = if diarize { Some(Embedder::load(&speaker_model.file_path(0))?) } else { None };
    let mut done = 0usize;
    let step = |label: &str, done: usize| Progress {
        step: format!("{label} ({done}/{total})"),
        fraction: 0.05 + 0.93 * done as f32 / total as f32,
    };

    let mut mic: Vec<Utterance> = Vec::new();
    vad::for_each_segment(&audio, MIC, &mic_segs, |i, samples| {
        check()?;
        let text = engine.transcribe(samples, opts.language)?;
        if !text.is_empty() {
            mic.push(Utterance { start: mic_segs[i].0, end: mic_segs[i].1, track: Track::Mic, text, speaker: String::new() });
        }
        done += 1;
        progress(step("Przepisywanie", done));
        Ok(())
    })?;

    let mut system: Vec<Utterance> = Vec::new();
    let mut voices: Vec<(f64, Option<Vec<f32>>)> = Vec::new();
    vad::for_each_segment(&audio, SYSTEM, &sys_segs, |i, samples| {
        check()?;
        let text = engine.transcribe(samples, opts.language)?;
        if !text.is_empty() {
            let (start, end) = sys_segs[i];
            system.push(Utterance { start, end, track: Track::System, text, speaker: String::new() });
            if let Some(e) = embedder.as_mut() {
                voices.push((end - start, e.embed(samples).unwrap_or_else(|err| {
                    log::warn!("wektor głosu: {err}");
                    None
                })));
            }
        }
        done += 1;
        progress(step("Przepisywanie", done));
        Ok(())
    })?;
    drop(engine);

    // 3) Mówcy.
    let speakers: Option<Vec<SpeakerSegment>> = if diarize && !system.is_empty() {
        progress(Progress { step: "Rozpoznawanie mówców".into(), fraction: 0.98 });
        let groups = diarize::cluster(&voices, diarize::SAME_SPEAKER);
        Some(
            system
                .iter()
                .zip(groups)
                .filter_map(|(u, g)| g.map(|g| SpeakerSegment { speaker_id: format!("S{g}"), start: u.start, end: u.end }))
                .collect(),
        )
    } else {
        None
    };

    let duration = super::writer::track_duration_samples(&audio, MIC).max(super::writer::track_duration_samples(&audio, SYSTEM)) as f64 / 16_000.0;
    let doc = TranscriptDocument {
        meeting_id: id.to_string(),
        engine: opts.engine.asset().title.to_string(),
        created_at: Local::now(),
        duration_seconds: if duration > 0.0 { duration } else { meeting.duration_seconds },
        utterances: transcript::build(&mic, &system, speakers.as_deref()),
    };
    std::fs::write(store.transcript_json(id), serde_json::to_vec_pretty(&doc)?)?;
    std::fs::write(store.transcript_md(id), transcript::markdown(&doc, meeting.started_at))?;
    progress(Progress { step: "Gotowe".into(), fraction: 1.0 });
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meetings::store::State;
    use crate::meetings::writer::SegmentedWriter;

    fn load(name: &str) -> Vec<f32> {
        let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let mut r = hound::WavReader::open(p).unwrap();
        r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
    }

    /// Syntetyczne spotkanie: „system” = kobieta, mężczyzna, kobieta, mężczyzna (FLEURS),
    /// „mikrofon” = moja wypowiedź w środku. Wymaga modeli: `cargo test -- --ignored synthetic_meeting`.
    #[test]
    #[ignore]
    fn synthetic_meeting_two_remote_speakers_and_me() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for a in [AssetId::SileroVad, AssetId::SpeakerModel, AssetId::Engine(EngineId::ParakeetV3)] {
            let asset = models::asset(a);
            if !asset.is_installed() {
                rt.block_on(asset.download(&AtomicBool::new(false), |_, _| {})).unwrap();
            }
        }
        let (w, m, me) = (load("fleurs_kobieta.wav"), load("fleurs_mezczyzna.wav"), load("ja_zosia.wav"));
        let room = |n: usize| vec![0.00001f32; n]; // „cisza w pokoju”, nie cyfrowe zero
        let gap = 32_000;
        let (mut sys, mut mic) = (Vec::new(), Vec::new());
        for (i, voice) in [&w, &m, &w, &m].into_iter().enumerate() {
            sys.extend_from_slice(voice);
            sys.extend(room(gap));
            mic.extend(room(voice.len() + gap));
            if i == 1 {
                sys.extend(room(me.len() + gap));
                mic.extend_from_slice(&me);
                mic.extend(room(gap));
            }
        }
        let root = std::env::temp_dir().join(format!("dx-e2e-{}", std::process::id()));
        let store = Store { root: root.clone() };
        let meeting = store.create(Local::now(), true).unwrap();
        let audio = store.audio_folder(&meeting.id);
        let mut ws = SegmentedWriter::new(&audio, SYSTEM).unwrap();
        ws.append(&sys).unwrap();
        ws.finish().unwrap();
        let mut wm = SegmentedWriter::new(&audio, MIC).unwrap();
        wm.append(&mic).unwrap();
        wm.finish().unwrap();
        store.update(&meeting.id, |x| x.state = State::Recorded).unwrap();

        let t = std::time::Instant::now();
        let opts = Options { engine: EngineId::ParakeetV3, language: Language::Pl, diarize: true };
        let doc = transcribe(&store, &meeting.id, &opts, &AtomicBool::new(false), |_| {}).unwrap();
        let md = std::fs::read_to_string(store.transcript_md(&meeting.id)).unwrap();
        println!("{:.1} s nagrania w {:.1} s\n{md}", sys.len() as f64 / 16_000.0, t.elapsed().as_secs_f64());
        let speakers: Vec<&str> = doc.utterances.iter().map(|u| u.speaker.as_str()).collect();
        assert_eq!(speakers, ["Rozmówca 1", "Rozmówca 2", "Ja", "Rozmówca 1", "Rozmówca 2"], "{md}");
        assert!(md.contains("czwartek"), "{md}");
        std::fs::remove_dir_all(root).ok();
    }
}
