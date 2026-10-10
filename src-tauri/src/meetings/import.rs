//! Importing a recorded call from an audio file (MP3, M4A/AAC, WAV, FLAC, OGG Vorbis/Opus, AIFF, CAF…).
//! The file is decoded as a stream (nothing grows in RAM), downmixed to mono, resampled to
//! 16 kHz and saved as the meeting's "system" track — in a plain recording you can't separate
//! your own voice, so everyone is a participant, and speaker recognition gives them the labels
//! "Rozmówca 1, 2…" ("Participant 1, 2…"). From there the regular meeting transcription runs.
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use once_cell::sync::Lazy;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use super::store::{Meeting, State, Store, SYSTEM};
use super::writer::{SegmentedWriter, RATE};
use crate::audio::resample::{downmix, StreamResampler};
use crate::i18n;

/// Extensions for the file picker filter.
pub const EXTENSIONS: &[&str] = &["mp3", "m4a", "mp4", "aac", "wav", "flac", "ogg", "oga", "aif", "aiff", "caf", "mka", "webm", "opus"];

/// Symphonia codecs + Opus via libopus (Symphonia has no Opus decoder of its own, and that's the
/// format of WhatsApp and Telegram voice notes).
static CODECS: Lazy<CodecRegistry> = Lazy::new(|| {
    let mut registry = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut registry);
    registry.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
    registry
});

/// Decodes the file and yields successive mono chunks at the source rate: `(rate, samples,
/// progress 0..=1 if the duration is known)`.
pub fn decode(path: &Path, cancel: &AtomicBool, mut on_chunk: impl FnMut(u32, &[f32], Option<f32>) -> Result<()>) -> Result<()> {
    let file = std::fs::File::open(path).with_context(|| i18n::t_with("import.opening", &[("path", &path.display())]))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| anyhow!(i18n::t_with("import.unsupported_format", &[("error", &e)])))?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| anyhow!(i18n::t("import.no_audio_track")))?;
    let track_id = track.id;
    let total_frames = track.num_frames;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or_else(|| anyhow!(i18n::t("import.no_track_params")))?;
    let mut decoder = CODECS
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| anyhow!(i18n::t_with("import.unsupported_codec", &[("error", &e)])))?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut frames_done: u64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!(super::CANCELLED));
        }
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) | Err(DecodeError::ResetRequired) => break,
            Err(DecodeError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(anyhow!(i18n::t_with("import.read_error", &[("error", &e)]))),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // Corrupted packet — skip it, the rest of the recording is more valuable.
            Err(DecodeError::DecodeError(e)) => {
                log::warn!("import: skipped packet ({e})");
                continue;
            }
            Err(DecodeError::IoError(_)) => continue,
            Err(e) => return Err(anyhow!(i18n::t_with("import.decode_error", &[("error", &e)]))),
        };
        let rate = decoded.spec().rate();
        let channels = decoded.spec().channels().count().max(1);
        decoded.copy_to_vec_interleaved(&mut interleaved);
        let mono = downmix(&interleaved, channels);
        frames_done += mono.len() as u64;
        let fraction = total_frames.filter(|t| *t > 0).map(|t| (frames_done as f64 / t as f64).min(1.0) as f32);
        on_chunk(rate, &mono, fraction)?;
    }
    if frames_done == 0 {
        return Err(anyhow!(i18n::t("import.no_audio")));
    }
    Ok(())
}

/// Creates a meeting for the imported file (state `importing`, date = file modification date,
/// title = file name). The audio is appended by `fill`.
pub fn create(store: &Store, path: &Path) -> Result<Meeting> {
    if !path.is_file() {
        return Err(anyhow!(i18n::t_with("import.no_such_file", &[("path", &path.display())])));
    }
    let started: DateTime<Local> = std::fs::metadata(path).and_then(|m| m.modified()).map(DateTime::from).unwrap_or_else(|_| Local::now());
    let meeting = store.create(started, true)?;
    let title = path.file_stem().map(|s| s.to_string_lossy().trim().to_string()).filter(|t| !t.is_empty());
    store.update(&meeting.id, |m| {
        m.state = State::Importing;
        m.title = title;
    })
}

/// Decodes the file into the meeting's "system" track and marks it as recorded. On error or
/// cancellation it deletes the meeting (the original file is left untouched).
pub fn fill(store: &Store, meeting: &Meeting, path: &Path, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<Meeting> {
    let result = (|| -> Result<u64> {
        let mut writer = SegmentedWriter::new(&store.audio_folder(&meeting.id), SYSTEM)?;
        let mut resampler: Option<(u32, StreamResampler)> = None;
        decode(path, cancel, |rate, mono, fraction| {
            if resampler.as_ref().is_none_or(|(r, _)| *r != rate) {
                // First chunk, or a sample rate change mid-file (rare, but it happens).
                if let Some((_, mut old)) = resampler.take() {
                    writer.append(&old.flush())?;
                }
                resampler = Some((rate, StreamResampler::new(rate)?));
            }
            let (_, r) = resampler.as_mut().expect("resampler");
            writer.append(&r.push(mono))?;
            if let Some(f) = fraction {
                progress(f);
            }
            Ok(())
        })?;
        if let Some((_, mut r)) = resampler {
            writer.append(&r.flush())?;
        }
        writer.finish()
    })();
    match result {
        Ok(samples) => {
            let seconds = samples as f64 / RATE as f64;
            store.update(&meeting.id, |m| {
                m.state = State::Recorded;
                m.duration_seconds = seconds;
                m.ended_at = Some(m.started_at + chrono::Duration::milliseconds((seconds * 1000.0) as i64));
            })
        }
        Err(e) => {
            let _ = store.delete(&meeting.id);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meetings::writer;

    fn store() -> Store {
        let root = std::env::temp_dir().join(format!(
            "dx-import-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        Store { root }
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    #[test]
    fn decodes_16k_mono_wav_sample_exact() {
        let path = fixture("ja_zosia.wav");
        let expected = hound::WavReader::open(&path).unwrap().duration() as usize;
        let mut got = 0;
        decode(&path, &AtomicBool::new(false), |rate, s, _| {
            assert_eq!(rate, 16_000);
            got += s.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(got, expected);
    }

    /// Stereo 44.1 kHz (like a typical voice recorder export) → a meeting with one 16 kHz track of
    /// the same length; speech only in the right channel must not disappear when mixing down.
    #[test]
    fn imports_stereo_44k_file_as_system_track() {
        let s = store();
        std::fs::create_dir_all(&s.root).unwrap();
        let path = s.root.join("Rozmowa z klientem.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..44_100 * 3 {
            let tone = ((i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 12_000.0) as i16;
            w.write_sample(0i16).unwrap(); // left: silence
            w.write_sample(tone).unwrap(); // right: signal
        }
        w.finalize().unwrap();

        let m = create(&s, &path).unwrap();
        assert_eq!(m.state, State::Importing);
        assert_eq!(m.title.as_deref(), Some("Rozmowa z klientem"));
        let mut last = 0.0;
        let m = fill(&s, &m, &path, &AtomicBool::new(false), |f| last = f).unwrap();
        assert_eq!(m.state, State::Recorded);
        assert!(m.has_system_audio);
        assert!((m.duration_seconds - 3.0).abs() < 0.05, "{}", m.duration_seconds);
        assert!((last - 1.0).abs() < 1e-3, "postęp kończy się na 1: {last}");
        let audio = s.audio_folder(&m.id);
        assert_eq!(writer::track_duration_samples(&audio, "mic"), 0, "bez ścieżki mikrofonu");
        let mut peak = 0.0f32;
        writer::read_track(&audio, SYSTEM, 30, |_, c| {
            peak = c.iter().fold(peak, |p, x| p.max(x.abs()));
            Ok(())
        })
        .unwrap();
        assert!(peak > 0.1, "sygnał z prawego kanału przetrwał: {peak}");
        std::fs::remove_dir_all(&s.root).ok();
    }

    #[test]
    fn unsupported_file_leaves_no_meeting() {
        let s = store();
        std::fs::create_dir_all(&s.root).unwrap();
        let path = s.root.join("notatki.mp3");
        std::fs::write(&path, b"to nie jest dzwiek, tylko tekst udajacy mp3").unwrap();
        let m = create(&s, &path).unwrap();
        let err = fill(&s, &m, &path, &AtomicBool::new(false), |_| {}).unwrap_err().to_string();
        assert!(err.contains("format") || err.contains("dźwięk"), "{err}");
        assert!(s.load(&m.id).is_none(), "nieudany import nie zostawia spotkania");
        std::fs::remove_dir_all(&s.root).ok();
    }

    /// Compressed formats, as from a phone/voice recorder/messenger (requires `ffmpeg` in PATH):
    /// `cargo test --lib -- --ignored import_compressed --nocapture`.
    #[test]
    #[ignore]
    fn import_compressed_formats() {
        let s = store();
        std::fs::create_dir_all(&s.root).unwrap();
        let src = fixture("fleurs_kobieta.wav");
        let expected = hound::WavReader::open(&src).unwrap().duration() as f64 / 16_000.0;
        for (ext, args) in [("mp3", vec!["-ar", "44100", "-ac", "2"]), ("m4a", vec!["-ar", "48000", "-c:a", "aac"]), ("flac", vec![]), ("ogg", vec!["-c:a", "vorbis", "-strict", "-2", "-ac", "2"]), ("opus", vec!["-c:a", "libopus"])] {
            let path = s.root.join(format!("rozmowa.{ext}"));
            let ok = std::process::Command::new("ffmpeg").args(["-loglevel", "error", "-y", "-i"]).arg(&src).args(&args).arg(&path).status().map(|st| st.success());
            assert!(matches!(ok, Ok(true)), "ffmpeg nie utworzył {ext}");
            let m = create(&s, &path).unwrap();
            let m = fill(&s, &m, &path, &AtomicBool::new(false), |_| {}).unwrap();
            println!("{ext}: {:.2} s (źródło {expected:.2} s)", m.duration_seconds);
            assert!((m.duration_seconds - expected).abs() < 0.15, "{ext}: {} vs {expected}", m.duration_seconds);
        }
        std::fs::remove_dir_all(&s.root).ok();
    }

    /// The whole flow as in the app: a two-person call in MP3 → import → transcription with speaker
    /// recognition. Requires models and `ffmpeg`: `cargo test --lib -- --ignored import_then --nocapture`.
    #[test]
    #[ignore]
    fn import_then_transcribe_two_speakers() {
        use crate::meetings::transcriber::{self, Options};
        let s = store();
        std::fs::create_dir_all(&s.root).unwrap();
        let path = s.root.join("rozmowa.mp3");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-i"])
            .arg(fixture("fleurs_kobieta.wav"))
            .arg("-i")
            .arg(fixture("fleurs_mezczyzna.wav"))
            .args(["-filter_complex", "[0][1]concat=n=2:v=0:a=1,apad=pad_dur=0.8", "-ar", "44100", "-ac", "2"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(ok.success());
        let m = create(&s, &path).unwrap();
        let m = fill(&s, &m, &path, &AtomicBool::new(false), |_| {}).unwrap();
        let doc = transcriber::transcribe(
            &s,
            &m.id,
            &Options { engine: crate::models::EngineId::ParakeetV3, languages: vec!["pl".into()], vocabulary: String::new(), diarize: true, tuning: Default::default() },
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        for u in &doc.utterances {
            println!("[{:.1}] {}: {}", u.start, u.speaker, u.text);
        }
        assert!(doc.utterances.iter().all(|u| u.speaker != crate::meetings::transcript::ME), "w imporcie nie ma „Ja”");
        let text = doc.utterances.iter().map(|u| u.text.to_lowercase()).collect::<Vec<_>>().join(" ");
        assert!(text.contains("wizy"), "{text}");
        assert_eq!(doc.speakers(), vec!["Rozmówca 1".to_string(), "Rozmówca 2".to_string()]);
        std::fs::remove_dir_all(&s.root).ok();
    }

    #[test]
    fn cancel_removes_partial_meeting() {
        let s = store();
        let path = fixture("fleurs_kobieta.wav");
        let m = create(&s, &path).unwrap();
        let err = fill(&s, &m, &path, &AtomicBool::new(true), |_| {}).unwrap_err().to_string();
        assert_eq!(err, super::super::CANCELLED);
        assert!(s.load(&m.id).is_none());
        std::fs::remove_dir_all(&s.root).ok();
    }
}
