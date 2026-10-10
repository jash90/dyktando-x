//! Speech recognition engines: Parakeet v3 and Canary v2 (ONNX) via transcribe-rs, Whisper
//! (whisper.cpp) via `crate::whisper`.
//! Input is always 16 kHz mono f32. An instance is never shared between threads at the same time.
use anyhow::{anyhow, bail, Result};
use transcribe_rs::onnx::canary::{CanaryModel, CanaryParams};
use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams, TimestampGranularity};
use transcribe_rs::onnx::Quantization;

use crate::languages;
use crate::models::EngineId;
use crate::settings::Language;
use crate::whisper::Whisper;

pub const SAMPLE_RATE: u32 = 16_000;

pub enum Engine {
    Parakeet(ParakeetModel),
    Canary(CanaryModel),
    Whisper(Whisper),
}

impl Engine {
    pub fn load(id: EngineId) -> Result<Self> {
        let asset = id.asset();
        if !asset.is_installed() {
            return Err(anyhow!("Model {} nie jest pobrany", asset.title));
        }
        Ok(match id {
            EngineId::ParakeetV3 => Engine::Parakeet(
                ParakeetModel::load(&asset.dir_path(), &Quantization::Int8)
                    .map_err(|e| anyhow!("Nie udało się wczytać Parakeeta: {e}"))?,
            ),
            EngineId::CanaryV2 => Engine::Canary(
                CanaryModel::load(&asset.dir_path(), &Quantization::Int8).map_err(|e| anyhow!("Nie udało się wczytać Canary: {e}"))?,
            ),
            EngineId::WhisperTurbo | EngineId::WhisperLargeV3 => Engine::Whisper(
                Whisper::load(&asset.file_path(0)).map_err(|e| anyhow!("Nie udało się wczytać Whispera: {e}"))?,
            ),
        })
    }

    /// Dictionary of names and terms as a hint for Whisper (Parakeet and Canary don't support it).
    pub fn set_vocabulary(&mut self, vocabulary: &str) {
        if let Engine::Whisper(w) = self {
            w.set_vocabulary(vocabulary);
        }
    }

    /// Whether the model can transcribe these languages (whisper.cpp codes; empty = detects the language
    /// itself, several = mixed-language conversation). Checked before loading the model.
    pub fn check_languages(id: EngineId, languages: &[String]) -> Result<()> {
        if let Some(code) = languages.iter().find(|c| languages::name(c).is_none()) {
            bail!("Nieznany język „{code}”");
        }
        let model = id.asset().title;
        match id {
            EngineId::WhisperTurbo | EngineId::WhisperLargeV3 => {}
            EngineId::CanaryV2 if languages.len() > 1 => {
                bail!("{model} nie rozpoznaje języka sam — rozmowę w kilku językach przepisz Whisperem albo Parakeetem")
            }
            EngineId::ParakeetV3 | EngineId::CanaryV2 => {
                if let Some(code) = languages.iter().find(|c| !languages::EUROPEAN.contains(&c.as_str())) {
                    bail!("{model} nie zna języka: {} — wybierz Whispera", languages::name(code).unwrap_or(code));
                }
            }
        }
        Ok(())
    }

    /// Whether the engine can translate to `target` (Canary: between English and the rest of its
    /// languages; Whisper: only to English; Parakeet: not at all).
    pub fn can_translate(&self, language: Language, target: &str) -> bool {
        match self {
            Engine::Canary(_) => {
                let source = language.code().unwrap_or("pl");
                source != target && (source == "en" || target == "en")
            }
            Engine::Whisper(_) => target == "en" && language.code() != Some("en"),
            Engine::Parakeet(_) => false,
        }
    }

    /// Returns just the text (no postprocessing). Parakeet v3 detects the language itself;
    /// Whisper gets a language code or auto-detection.
    pub fn transcribe(&mut self, samples: &[f32], language: Language) -> Result<String> {
        self.transcribe_in(samples, language.code().as_slice())
    }

    /// Like `transcribe`, but with a language list (see `check_languages`). With several, Whisper
    /// first detects which of them the utterance is in, and transcribes in that language.
    pub fn transcribe_in(&mut self, samples: &[f32], languages: &[&str]) -> Result<String> {
        self.run(samples, languages, None)
    }

    /// Speech translation to `target` (language code, e.g. "en") — see `can_translate`.
    pub fn translate(&mut self, samples: &[f32], language: Language, target: &str) -> Result<String> {
        if !self.can_translate(language, target) {
            return Err(anyhow!("Ten model nie tłumaczy z {} na {target}", language.code().unwrap_or("auto")));
        }
        self.run(samples, language.code().as_slice(), Some(target))
    }

    fn run(&mut self, samples: &[f32], languages: &[&str], target: Option<&str>) -> Result<String> {
        if samples.len() < (SAMPLE_RATE as usize) / 10 {
            return Ok(String::new());
        }
        let text = match self {
            Engine::Parakeet(m) => {
                // transcribe_with pads 250 ms of silence at the start itself; we pad the end,
                // so the last word isn't cut off when the key is released quickly.
                let mut padded = samples.to_vec();
                padded.extend(std::iter::repeat(0.0).take(SAMPLE_RATE as usize / 4));
                m.transcribe_with(
                    &padded,
                    &ParakeetParams {
                        timestamp_granularity: Some(TimestampGranularity::Segment),
                        ..Default::default()
                    },
                )
                .map_err(|e| anyhow!("Parakeet: {e}"))?
                .text
            }
            Engine::Canary(m) => {
                // Canary doesn't detect the language itself — "automatic" is treated as Polish.
                let source = match languages {
                    [] => "pl",
                    [one] => one,
                    _ => bail!("Canary nie rozpoznaje języka sam — wybierz jeden język"),
                };
                let params = CanaryParams {
                    language: Some(source.to_string()),
                    target_language: target.map(str::to_string),
                    ..Default::default()
                };
                m.transcribe_with(samples, &params).map_err(|e| anyhow!("Canary: {e}"))?.text
            }
            Engine::Whisper(w) => {
                let language = match languages {
                    [] => None,
                    [one] => Some(*one),
                    many => Some(w.detect_language(samples, many).map_err(|e| anyhow!("Whisper: {e}"))?),
                };
                w.transcribe(samples, language, target == Some("en")).map_err(|e| anyhow!("Whisper: {e}"))?
            }
        };
        Ok(text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Asset;
    use std::sync::atomic::AtomicBool;

    fn fixture() -> Vec<f32> {
        fixture_named("fleurs_kobieta.wav")
    }

    fn fixture_named(name: &str) -> Vec<f32> {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let mut r = hound::WavReader::open(path).unwrap();
        r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
    }

    fn langs(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn language_choice_is_checked_per_model() {
        let whisper = EngineId::WhisperTurbo;
        assert!(Engine::check_languages(whisper, &langs(&["pl", "en", "ja"])).is_ok());
        assert!(Engine::check_languages(whisper, &[]).is_ok());
        assert!(Engine::check_languages(whisper, &langs(&["xx"])).is_err());
        // Canary: one language, and only a European one.
        assert!(Engine::check_languages(EngineId::CanaryV2, &langs(&["de"])).is_ok());
        assert!(Engine::check_languages(EngineId::CanaryV2, &langs(&["pl", "en"])).is_err());
        assert!(Engine::check_languages(EngineId::CanaryV2, &langs(&["ja"])).is_err());
        // Parakeet detects by itself, but only among European languages.
        assert!(Engine::check_languages(EngineId::ParakeetV3, &langs(&["pl", "en"])).is_ok());
        assert!(Engine::check_languages(EngineId::ParakeetV3, &langs(&["pl", "ja"])).is_err());
    }

    fn ensure(asset: &Asset) {
        if !asset.is_installed() {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(asset.download(&AtomicBool::new(false), |_, _| {}))
                .unwrap();
        }
    }

    fn words(s: &str) -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn check(id: EngineId) {
        ensure(id.asset());
        let t = std::time::Instant::now();
        let mut e = Engine::load(id).unwrap();
        let load = t.elapsed();
        let t = std::time::Instant::now();
        let text = e.transcribe(&fixture(), Language::Pl).unwrap();
        println!("{id:?}: wczytanie {load:?}, transkrypcja {:?}: {text}", t.elapsed());
        let expected = words("Jeżeli schodzisz na brzeg wyłącznie podczas wycieczek statkiem nie musisz mieć oddzielnej wizy");
        let got = words(&text);
        let hits = expected.iter().filter(|w| got.contains(w)).count();
        assert!(hits * 10 >= expected.len() * 8, "trafione {hits}/{}: {text}", expected.len());
    }

    /// Downloads the model (~670 MB) on first run: `cargo test -- --ignored parakeet`.
    #[test]
    #[ignore]
    fn parakeet_transcribes_polish() {
        check(EngineId::ParakeetV3);
    }

    /// ~1.6 GB: `cargo test -- --ignored whisper_turbo`.
    #[test]
    #[ignore]
    fn whisper_turbo_transcribes_polish() {
        check(EngineId::WhisperTurbo);
    }

    /// ~1 GB: `cargo test -- --ignored canary`.
    #[test]
    #[ignore]
    fn canary_transcribes_polish() {
        check(EngineId::CanaryV2);
    }

    /// pl→en translation via Canary: `cargo test -- --ignored canary_translates`.
    #[test]
    #[ignore]
    fn canary_translates_polish_to_english() {
        ensure(EngineId::CanaryV2.asset());
        let mut e = Engine::load(EngineId::CanaryV2).unwrap();
        assert!(e.can_translate(Language::Pl, "en"));
        assert!(!e.can_translate(Language::Pl, "de"));
        let t = std::time::Instant::now();
        let text = e.translate(&fixture(), Language::Pl, "en").unwrap();
        println!("Canary pl→en w {:?}: {text}", t.elapsed());
        let got = words(&text);
        assert!(got.contains(&"visa".to_string()), "{text}");
        assert!(!got.contains(&"wizy".to_string()), "{text}");
    }

    /// Mixed-language conversation: each utterance in a language picked from the selected ones.
    /// `cargo test -- --ignored whisper_turbo_picks`.
    #[test]
    #[ignore]
    fn whisper_turbo_picks_the_language_among_chosen() {
        ensure(EngineId::WhisperTurbo.asset());
        let Engine::Whisper(mut w) = Engine::load(EngineId::WhisperTurbo).unwrap() else { unreachable!() };
        let (pl, en) = (fixture(), fixture_named("en_samantha.wav"));
        assert_eq!(w.detect_language(&pl, &["en", "pl"]).unwrap(), "pl");
        assert_eq!(w.detect_language(&en, &["pl", "en"]).unwrap(), "en");
        let mut e = Engine::Whisper(w);
        let text = e.transcribe_in(&en, &["pl", "en"]).unwrap();
        println!("en: {text}");
        let got = words(&text);
        assert!(["quarterly", "report", "team", "friday"].iter().all(|w| got.contains(&w.to_string())), "{text}");
        let text = e.transcribe_in(&pl, &["pl", "en"]).unwrap();
        println!("pl: {text}");
        assert!(words(&text).contains(&"wizy".to_string()), "{text}");
    }

    #[test]
    #[ignore]
    fn whisper_large_transcribes_polish() {
        check(EngineId::WhisperLargeV3);
    }
}
