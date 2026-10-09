//! Whisper (whisper.cpp) wprost przez whisper-rs. transcribe-rs nie daje dostępu do stanu
//! modelu, a ten jest potrzebny, żeby rozpoznać język wypowiedzi spośród wybranych.
//! Parametry dekodowania są te same co w transcribe-rs (beam search 3, bez kontekstu).
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use transcribe_rs::whisper_cpp::gpu::auto_select_gpu_device;
use transcribe_rs::{get_whisper_accelerator, get_whisper_gpu_device, GPU_DEVICE_AUTO};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

pub struct Whisper {
    // Kolejność ma znaczenie: stan zwalniamy przed kontekstem.
    state: WhisperState,
    _context: WhisperContext,
    /// Podpowiedź na start (słownik nazw i terminów) — Whisper chętniej pisze je poprawnie.
    prompt: Option<String>,
}

/// Dłuższy słownik i tak by się nie zmieścił (Whisper bierze ok. 220 tokenów podpowiedzi).
const MAX_PROMPT_CHARS: usize = 600;

/// Słowa do porównań: małe litery, bez interpunkcji.
fn plain(text: &str) -> String {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Na dźwięku bez słów Whisper potrafi „przeczytać” samą podpowiedź (albo jej kawałek). Jedno
/// słowo zostawiamy — to częściej prawdziwe zawołanie po imieniu („Borys?”) niż echo; całe
/// słowa porównujemy, więc „ci” nie pasuje do „CI/CD”, a „se” do „Hisense”.
fn is_prompt_echo(text: &str, prompt: &str) -> bool {
    let t = plain(text);
    t.contains(' ') && format!(" {} ", plain(prompt)).contains(&format!(" {t} "))
}

/// Wątki dla rozpoznania języka (whisper.cpp przy dekodowaniu sam bierze min(4, rdzenie)).
fn threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(4))
}

impl Whisper {
    pub fn load(path: &Path) -> Result<Self> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu = get_whisper_accelerator().use_gpu();
        params.flash_attn = true;
        params.gpu_device = match get_whisper_gpu_device() {
            _ if !params.use_gpu => 0,
            GPU_DEVICE_AUTO => auto_select_gpu_device(),
            device => device,
        };
        let path = path.to_str().context("Ścieżka modelu nie jest UTF-8")?;
        let context = WhisperContext::new_with_params(path, params).map_err(|e| anyhow!("{e}"))?;
        let state = context.create_state().map_err(|e| anyhow!("{e}"))?;
        Ok(Self { state, _context: context, prompt: None })
    }

    /// Słownik nazw i terminów (np. „NPaw, Hisense, Tizen, CI/CD”); pusty = bez podpowiedzi.
    pub fn set_vocabulary(&mut self, vocabulary: &str) {
        let v = vocabulary.split_whitespace().collect::<Vec<_>>().join(" ");
        self.prompt = (!v.is_empty()).then(|| v.chars().take(MAX_PROMPT_CHARS).collect());
    }

    /// Najbardziej prawdopodobny z `candidates` (kody whisper.cpp) język wypowiedzi.
    pub fn detect_language<'a>(&mut self, samples: &[f32], candidates: &[&'a str]) -> Result<&'a str> {
        self.state.pcm_to_mel(samples, threads()).map_err(|e| anyhow!("{e}"))?;
        let (_, probs) = self.state.lang_detect(0, threads()).map_err(|e| anyhow!("{e}"))?;
        candidates
            .iter()
            .filter_map(|&code| Some((code, *probs.get(whisper_rs::get_lang_id(code)? as usize)?)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(code, _)| code)
            .ok_or_else(|| anyhow!("Whisper nie zna żadnego z wybranych języków"))
    }

    /// `language: None` = whisper.cpp sam rozpoznaje język (spośród wszystkich).
    pub fn transcribe(&mut self, samples: &[f32], language: Option<&str>, translate: bool) -> Result<String> {
        let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 3, patience: -1.0 });
        params.set_language(language);
        params.set_translate(translate);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_no_speech_thold(0.2);
        params.set_no_context(true);
        if let Some(prompt) = self.prompt.as_deref() {
            params.set_initial_prompt(prompt);
        }
        self.state.full(params, samples).map_err(|e| anyhow!("{e}"))?;
        let mut text = String::new();
        for i in 0..self.state.full_n_segments() {
            let segment = self.state.get_segment(i).ok_or_else(|| anyhow!("brak segmentu {i}"))?;
            text.push_str(segment.to_str().map_err(|e| anyhow!("{e}"))?);
        }
        if self.prompt.as_deref().is_some_and(|p| is_prompt_echo(&text, p)) {
            return Ok(String::new());
        }
        Ok(text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_out_prompt_is_not_an_utterance() {
        let prompt = "NPaw, Hisense, Tizen, CI/CD, Klaudiusz";
        assert!(is_prompt_echo("NPaw, Hisense, Tizen, CI/CD, Klaudiusz.", prompt));
        assert!(is_prompt_echo(" Hisense, Tizen", prompt));
        assert!(!is_prompt_echo("Wczoraj na Hisense coś tam patrzyłem", prompt));
        assert!(!is_prompt_echo("...", prompt));
        // Jedno słowo (nawet ze słownika) to prawdziwa wypowiedź.
        assert!(!is_prompt_echo("Klaudiusz?", prompt));
        assert!(!is_prompt_echo("Ci", prompt));
        // Tylko całe słowa: „sense tizen” to nie kawałek „Hisense, Tizen”.
        assert!(!is_prompt_echo("sense Tizen", prompt));
    }
}
