//! Evaluation of meeting transcription on your own recordings (locally, the recordings aren't in the repo).
//! Does the same as the app after a file import: decoding → "system" track → `transcriber`.
//!
//! ```text
//! DX_EVAL_FILE=~/Downloads/rozmowa.mp3      audio file (required)
//! DX_EVAL_FROM=600 DX_EVAL_SECONDS=180      excerpt (s), whole file by default
//! DX_EVAL_ENGINE=parakeet|canary|whisper_turbo|whisper_large   default parakeet
//! DX_EVAL_LANG=auto|pl|pl,en                 default auto; several, comma-separated = mixed-language call
//! DX_EVAL_VOCAB="NPaw, Tizen"              vocabulary of names for Whisper
//! DX_EVAL_REF=wzorzec.txt                    reference text → WER
//! DX_EVAL_OUT=katalog                        where to save the transcript (.md and .txt)
//! cargo test --release --lib -- --ignored eval_meeting --nocapture
//! ```
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use super::import;
use super::store::{State, Store, SYSTEM};
use super::transcriber::{self, Options};
use super::writer::{SegmentedWriter, RATE};
use crate::audio::resample::StreamResampler;
use crate::models::EngineId;

/// Words for WER: lowercase, no punctuation (digits and letters stay).
pub fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Word error rate (substitutions + deletions + insertions) / reference length.
pub fn wer(reference: &str, hypothesis: &str) -> f64 {
    let (r, h) = (words(reference), words(hypothesis));
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    let mut cur = vec![0; h.len() + 1];
    for (i, rw) in r.iter().enumerate() {
        cur[0] = i + 1;
        for (j, hw) in h.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(rw != hw)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[h.len()] as f64 / r.len() as f64
}

/// An utterance that most likely came out in a language other than Polish (Parakeet guesses the
/// language on its own for each segment): at least 4 words, no Polish letters and almost no
/// Polish function words.
fn looks_foreign(text: &str) -> bool {
    const FUNCTION_WORDS: &[&str] = &[
        "i", "w", "z", "na", "nie", "to", "się", "że", "jest", "co", "jak", "ale", "do", "tak", "no", "ja", "ty", "on", "ona", "my",
        "czy", "o", "a", "po", "za", "od", "dla", "tego", "bo", "już", "też", "mnie", "mi", "by", "ten", "ta", "tu", "tam", "są", "był",
        "jeszcze", "tylko", "bardzo", "mam", "ma", "może", "więc", "jakby", "znaczy", "wiesz", "właśnie",
    ];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).collect();
    if words.len() < 4 || lower.chars().any(|c| "ąćęłńóśźż".contains(c)) {
        return false;
    }
    let polish = words.iter().filter(|w| FUNCTION_WORDS.contains(w)).count();
    (polish as f64) < 0.15 * words.len() as f64
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[test]
fn foreign_language_heuristic() {
    assert!(looks_foreign("Show them what you have done today"));
    assert!(!looks_foreign("No i to jest dobre pytanie"));
    assert!(!looks_foreign("Okay."), "krótkie wtrącenia zostają");
}

#[test]
fn wer_counts_edits() {
    assert_eq!(wer("ala ma kota", "ala ma kota"), 0.0);
    assert!((wer("ala ma kota", "Ala, ma psa!") - 1.0 / 3.0).abs() < 1e-9);
    assert!((wer("ala ma kota", "ala kota") - 1.0 / 3.0).abs() < 1e-9);
    assert!((wer("ala ma kota", "ala ma tego kota") - 1.0 / 3.0).abs() < 1e-9);
}

#[test]
#[ignore]
fn eval_meeting() {
    let file = PathBuf::from(env("DX_EVAL_FILE").expect("DX_EVAL_FILE"));
    let from = env("DX_EVAL_FROM").map(|v| v.parse::<f64>().unwrap()).unwrap_or(0.0);
    let seconds = env("DX_EVAL_SECONDS").map(|v| v.parse::<f64>().unwrap());
    let engine = match env("DX_EVAL_ENGINE").as_deref().unwrap_or("parakeet") {
        "canary" => EngineId::CanaryV2,
        "whisper_turbo" => EngineId::WhisperTurbo,
        "whisper_large" => EngineId::WhisperLargeV3,
        _ => EngineId::ParakeetV3,
    };
    let languages: Vec<String> = match env("DX_EVAL_LANG") {
        Some(v) if v != "auto" => v.split(',').map(|c| c.trim().to_string()).collect(),
        _ => Vec::new(),
    };

    // Excerpt of the file as the "system" track of a temporary meeting.
    let root = std::env::temp_dir().join(format!("dx-eval-{}", std::process::id()));
    let store = Store { root: root.clone() };
    let meeting = import::create(&store, &file).unwrap();
    let (skip, keep) = ((from * RATE as f64) as u64, seconds.map(|s| (s * RATE as f64) as u64));
    let mut writer = SegmentedWriter::new(&store.audio_folder(&meeting.id), SYSTEM).unwrap();
    let mut resampler: Option<StreamResampler> = None;
    let mut pos = 0u64;
    let t = std::time::Instant::now();
    let _ = import::decode(&file, &AtomicBool::new(false), |rate, mono, _| {
        let r = resampler.get_or_insert_with(|| StreamResampler::new(rate).unwrap());
        let out = r.push(mono);
        let (a, b) = (pos, pos + out.len() as u64);
        pos = b;
        let lo = skip.max(a);
        let hi = keep.map_or(b, |k| (skip + k).min(b));
        if hi > lo {
            writer.append(&out[(lo - a) as usize..(hi - a) as usize])?;
        }
        if keep.is_some_and(|k| pos >= skip + k) {
            anyhow::bail!("koniec wycinka");
        }
        Ok(())
    });
    let samples = writer.finish().unwrap();
    store.update(&meeting.id, |m| {
        m.state = State::Recorded;
        m.duration_seconds = samples as f64 / RATE as f64;
    })
    .unwrap();
    let decode_s = t.elapsed().as_secs_f64();

    let t = std::time::Instant::now();
    let mut tuning = transcriber::Tuning::default();
    let num = |n: &str| env(n).map(|v| v.parse::<f64>().unwrap());
    if let Some(v) = num("DX_EVAL_MIN_SILENCE") {
        tuning.vad.min_silence = v;
    }
    if let Some(v) = num("DX_EVAL_MAX_SEGMENT") {
        tuning.vad.max_segment = v;
    }
    if let Some(v) = num("DX_EVAL_PAD") {
        tuning.vad.pad = v;
    }
    if let Some(v) = num("DX_EVAL_THRESHOLD") {
        tuning.vad.threshold = v as f32;
    }
    println!("{tuning:?}");
    let doc = transcriber::transcribe(&store, &meeting.id, &Options { engine, languages: languages.clone(), vocabulary: env("DX_EVAL_VOCAB").unwrap_or_default(), diarize: true, tuning }, &AtomicBool::new(false), |_| {}).unwrap();
    let asr_s = t.elapsed().as_secs_f64();

    let mut per_speaker: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    for u in &doc.utterances {
        let e = per_speaker.entry(u.speaker.clone()).or_default();
        e.0 += 1;
        e.1 += u.end - u.start;
    }
    let text: String = doc.utterances.iter().map(|u| u.text.as_str()).collect::<Vec<_>>().join(" ");
    let foreign: Vec<&str> = doc.utterances.iter().filter(|u| looks_foreign(&u.text)).map(|u| u.text.as_str()).collect();

    println!("== {} [{from:.0} s +{:.0} s] {:?} {:?}", file.display(), samples as f64 / RATE as f64, engine, languages);
    println!("dekodowanie {decode_s:.1} s, transkrypcja {asr_s:.1} s ({:.1}× czasu rzeczywistego)", samples as f64 / RATE as f64 / asr_s);
    println!("mówcy ({}):", per_speaker.len());
    for (name, (n, secs)) in &per_speaker {
        println!("  {name}: {n} wypowiedzi, {secs:.0} s");
    }
    println!("wyglądające na obcy język: {}", foreign.len());
    for f in foreign.iter().take(10) {
        println!("  · {f}");
    }
    if let Some(reference) = env("DX_EVAL_REF") {
        let r = std::fs::read_to_string(&reference).unwrap();
        println!("WER {:.2}% ({} słów wzorca)", 100.0 * wer(&r, &text), words(&r).len());
    }
    if let Some(out) = env("DX_EVAL_OUT") {
        let stem = format!("{}_{from:.0}_{}_{:?}", file.file_stem().unwrap().to_string_lossy(), seconds.map_or("all".into(), |s| format!("{s:.0}")), engine);
        let stem = env("DX_EVAL_TAG").map_or(stem.clone(), |t| format!("{stem}{t}"));
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(format!("{stem}.md")), std::fs::read_to_string(store.transcript_md(&meeting.id)).unwrap()).unwrap();
        std::fs::write(out.join(format!("{stem}.txt")), &text).unwrap();
        println!("zapisano {}", out.join(&stem).display());
    }
    std::fs::remove_dir_all(root).ok();
}
