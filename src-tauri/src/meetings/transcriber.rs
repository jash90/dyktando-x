//! Transcription of a recorded meeting: VAD on both tracks → each segment through a fresh
//! engine instance (not the dictation one) → voice embeddings on the "system" track → speaker
//! clustering → `TranscriptBuilder` → `transcript.md` + `transcript.json`.
use anyhow::{anyhow, Result};
use chrono::Local;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

use super::diarize::{self, Embedder};
use super::store::{Store, MIC, SYSTEM};
use super::transcript::{self, SpeakerSegment, Track, TranscriptDocument, Utterance};
use super::vad;
use crate::engine::Engine;
use crate::i18n;
use crate::models::{self, AssetId, EngineId};

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    /// Step description in Polish (for the progress bar).
    pub step: String,
    /// 0..=1
    pub fraction: f32,
}

/// Utterance with no content: only fillers ("uh", "mm-hmm", "yyy") or a typical engine
/// hallucination on short wordless audio (a breath, a cough, a nod). On call recordings each such
/// utterance was a separate "participant" or an interjection in a foreign language (see `eval.rs`).
pub fn is_noise(text: &str, seconds: f64) -> bool {
    const FILLERS: &[&str] = &["uh", "um", "umm", "uhm", "hm", "hmm", "mm", "mmm", "mhm", "hmhm", "yhm", "yyy", "yy", "y", "eee", "ee", "e", "ah", "eh", "oh"];
    const HALLUCINATIONS: &[&str] = &[
        "zobaczmy", "dziękuję", "dziękuję bardzo", "dziękuję za uwagę", "dziękuję za obejrzenie", "wszelkie prawa zastrzeżone",
        "napisy", "subskrybuj", "thank you", "thanks", "you", "bye",
    ];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() || words.iter().all(|w| FILLERS.contains(w)) {
        return true;
    }
    seconds < 2.0 && (HALLUCINATIONS.contains(&words.join(" ").as_str()) || words.iter().all(|w| w.chars().all(|c| c.is_ascii_digit())))
}

pub struct Options {
    pub engine: EngineId,
    /// Language codes (whisper.cpp): empty = the engine detects it, several = mixed-language call.
    pub languages: Vec<String>,
    /// Vocabulary of names and terms (a hint for Whisper); empty = none.
    pub vocabulary: String,
    pub diarize: bool,
    pub tuning: Tuning,
}

/// Processing parameters. Tested on call recordings (`eval.rs`): longer segments, a bigger
/// margin and loudness normalisation didn't reduce errors — the defaults stay.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tuning {
    pub vad: vad::Params,
}

pub fn transcribe(store: &Store, id: &str, opts: &Options, cancel: &AtomicBool, mut progress: impl FnMut(Progress)) -> Result<TranscriptDocument> {
    let meeting = store.load(id).ok_or_else(|| anyhow!(i18n::t_with("meeting.not_found_id", &[("id", &id)])))?;
    if meeting.audio_deleted {
        return Err(anyhow!(i18n::t("meeting.audio_deleted")));
    }
    let audio = store.audio_folder(id);
    let vad_model = models::asset(AssetId::SileroVad);
    for needed in [vad_model, opts.engine.asset()] {
        if !needed.is_installed() {
            return Err(anyhow!(i18n::t_with("model.missing", &[("model", &needed.label())])));
        }
    }
    Engine::check_languages(opts.engine, &opts.languages)?;
    let languages: Vec<&str> = opts.languages.iter().map(String::as_str).collect();
    let speaker_model = models::asset(AssetId::SpeakerModel);
    let diarize = opts.diarize && meeting.has_system_audio && speaker_model.is_installed();
    let is_cancelled = || cancel.load(Ordering::Relaxed);
    let check = || if cancel.load(Ordering::Relaxed) { Err(anyhow!(super::CANCELLED)) } else { Ok(()) };

    // 1) Speech detection (fast: ~1–2 % of the recording time).
    progress(Progress { step: i18n::t("job.detecting_speech_mic"), fraction: 0.0 });
    let mic_segs = vad::segments(&vad::track_probabilities(&vad_model.file_path(0), &audio, MIC, &is_cancelled)?, opts.tuning.vad);
    let sys_segs = if meeting.has_system_audio {
        progress(Progress { step: i18n::t("job.detecting_speech_system"), fraction: 0.03 });
        vad::segments(&vad::track_probabilities(&vad_model.file_path(0), &audio, SYSTEM, &is_cancelled)?, opts.tuning.vad)
    } else {
        Vec::new()
    };
    check()?;
    let total = (mic_segs.len() + sys_segs.len()).max(1);
    log::info!("Meeting {id}: {} microphone segments, {} participant segments", mic_segs.len(), sys_segs.len());

    // 2) Transcription of segments.
    progress(Progress { step: i18n::t("job.loading_model"), fraction: 0.05 });
    let mut engine = Engine::load(opts.engine)?;
    engine.set_vocabulary(&opts.vocabulary);
    let mut embedder = if diarize { Some(Embedder::load(&speaker_model.file_path(0))?) } else { None };
    let mut done = 0usize;
    let transcribing = i18n::t("job.transcribing");
    let step = |label: &str, done: usize| Progress {
        step: format!("{label} ({done}/{total})"),
        fraction: 0.05 + 0.93 * done as f32 / total as f32,
    };

    let mut mic: Vec<Utterance> = Vec::new();
    vad::for_each_segment(&audio, MIC, &mic_segs, |i, samples| {
        check()?;
        let text = engine.transcribe_in(samples, &languages)?;
        if !is_noise(&text, mic_segs[i].1 - mic_segs[i].0) {
            mic.push(Utterance { start: mic_segs[i].0, end: mic_segs[i].1, track: Track::Mic, text, speaker: String::new(), translation: None });
        }
        done += 1;
        progress(step(&transcribing, done));
        Ok(())
    })?;

    let mut system: Vec<Utterance> = Vec::new();
    let mut voices: Vec<(f64, Option<Vec<f32>>)> = Vec::new();
    vad::for_each_segment(&audio, SYSTEM, &sys_segs, |i, samples| {
        check()?;
        let text = engine.transcribe_in(samples, &languages)?;
        let (start, end) = sys_segs[i];
        if !is_noise(&text, end - start) {
            system.push(Utterance { start, end, track: Track::System, text, speaker: String::new(), translation: None });
            if let Some(e) = embedder.as_mut() {
                voices.push((end - start, e.embed(samples).unwrap_or_else(|err| {
                    log::warn!("voice embedding: {err}");
                    None
                })));
            }
        }
        done += 1;
        progress(step(&transcribing, done));
        Ok(())
    })?;
    drop(engine);

    // 3) Speakers.
    let speakers: Option<Vec<SpeakerSegment>> = if diarize && !system.is_empty() {
        progress(Progress { step: i18n::t("job.recognizing_speakers"), fraction: 0.98 });
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
    progress(Progress { step: i18n::t("job.done"), fraction: 1.0 });
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meetings::store::State;
    use crate::meetings::writer::SegmentedWriter;

    #[test]
    fn noise_utterances() {
        for t in ["Uh", "Um...", "Mm-hmm.", "Hmm", "Yyy...", ""] {
            assert!(is_noise(t, 1.0), "{t:?}");
        }
        assert!(is_noise("Mhm, mhm.", 5.0), "same wypełniacze niezależnie od długości");
        assert!(is_noise("Zobaczmy.", 0.8));
        assert!(is_noise("Dziękuję.", 1.2));
        assert!(is_noise("2", 0.6));
        assert!(!is_noise("Dziękuję.", 3.5), "dłuższe „dziękuję” jest prawdziwe");
        assert!(!is_noise("Okay.", 0.7), "potaknięcie słowem zostaje");
        assert!(!is_noise("Tak, tak.", 0.9));
        assert!(!is_noise("Zobaczmy, co tu mamy.", 1.5));
        assert!(!is_noise("2009 rok", 1.0));
    }

    fn load(name: &str) -> Vec<f32> {
        let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let mut r = hound::WavReader::open(p).unwrap();
        r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
    }

    /// Synthetic meeting: "system" = woman, man, woman, man (FLEURS), "microphone" = my
    /// utterance in the middle. Requires models: `cargo test -- --ignored synthetic_meeting`.
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
        let room = |n: usize| vec![0.00001f32; n]; // "room silence", not digital zero
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
        let opts = Options { engine: EngineId::ParakeetV3, languages: vec!["pl".into()], vocabulary: String::new(), diarize: true, tuning: Tuning::default() };
        let doc = transcribe(&store, &meeting.id, &opts, &AtomicBool::new(false), |_| {}).unwrap();
        let md = std::fs::read_to_string(store.transcript_md(&meeting.id)).unwrap();
        println!("{:.1} s nagrania w {:.1} s\n{md}", sys.len() as f64 / 16_000.0, t.elapsed().as_secs_f64());
        let speakers: Vec<&str> = doc.utterances.iter().map(|u| u.speaker.as_str()).collect();
        assert_eq!(speakers, ["Rozmówca 1", "Rozmówca 2", "Ja", "Rozmówca 1", "Rozmówca 2"], "{md}");
        assert!(md.contains("czwartek"), "{md}");
        std::fs::remove_dir_all(root).ok();
    }
}
