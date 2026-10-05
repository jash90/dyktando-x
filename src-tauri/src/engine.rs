//! Silniki rozpoznawania mowy: Parakeet v3 (ONNX) i Whisper (whisper.cpp) przez transcribe-rs.
//! Wejście zawsze 16 kHz mono f32. Instancja nie jest współdzielona między wątkami naraz.
use anyhow::{anyhow, Result};
use transcribe_rs::onnx::canary::{CanaryModel, CanaryParams};
use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams, TimestampGranularity};
use transcribe_rs::onnx::Quantization;
use transcribe_rs::whisper_cpp::{WhisperEngine, WhisperInferenceParams};

use crate::models::EngineId;
use crate::settings::Language;

pub const SAMPLE_RATE: u32 = 16_000;

pub enum Engine {
    Parakeet(ParakeetModel),
    Canary(CanaryModel),
    Whisper(WhisperEngine),
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
                WhisperEngine::load(&asset.file_path(0))
                    .map_err(|e| anyhow!("Nie udało się wczytać Whispera: {e}"))?,
            ),
        })
    }

    /// Zwraca sam tekst (bez postprocessingu). Parakeet v3 sam rozpoznaje język;
    /// Whisper dostaje kod języka albo autodetekcję.
    pub fn transcribe(&mut self, samples: &[f32], language: Language) -> Result<String> {
        if samples.len() < (SAMPLE_RATE as usize) / 10 {
            return Ok(String::new());
        }
        let text = match self {
            Engine::Parakeet(m) => {
                // transcribe_with sam dokłada 250 ms ciszy na początku; na końcu dokładamy my,
                // żeby ostatnie słowo nie było ucięte przy szybkim puszczeniu klawisza.
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
                // Canary nie rozpoznaje języka sam — „automatycznie” traktujemy jak polski.
                let params = CanaryParams { language: Some(language.code().unwrap_or("pl").to_string()), ..Default::default() };
                m.transcribe_with(samples, &params).map_err(|e| anyhow!("Canary: {e}"))?.text
            }
            Engine::Whisper(w) => {
                let params = WhisperInferenceParams {
                    language: language.code().map(str::to_string),
                    ..Default::default()
                };
                w.transcribe_with(samples, &params).map_err(|e| anyhow!("Whisper: {e}"))?.text
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
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fleurs_kobieta.wav");
        let mut r = hound::WavReader::open(path).unwrap();
        r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
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

    /// Pobiera model (~670 MB) przy pierwszym uruchomieniu: `cargo test -- --ignored parakeet`.
    #[test]
    #[ignore]
    fn parakeet_transcribes_polish() {
        check(EngineId::ParakeetV3);
    }

    /// ~1,6 GB: `cargo test -- --ignored whisper_turbo`.
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

    #[test]
    #[ignore]
    fn whisper_large_transcribes_polish() {
        check(EngineId::WhisperLargeV3);
    }
}
