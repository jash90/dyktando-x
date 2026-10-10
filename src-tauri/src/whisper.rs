//! Whisper (whisper.cpp) directly via whisper-rs. transcribe-rs doesn't expose the model
//! state, which is needed to detect the utterance's language among the selected ones.
//! Decoding parameters are the same as in transcribe-rs (beam search 3, no context).
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use transcribe_rs::whisper_cpp::gpu::auto_select_gpu_device;
use transcribe_rs::{get_whisper_accelerator, get_whisper_gpu_device, GPU_DEVICE_AUTO};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

pub struct Whisper {
    // Order matters: the state is dropped before the context.
    state: WhisperState,
    _context: WhisperContext,
    /// Initial prompt (dictionary of names and terms) — Whisper then spells them right more often.
    prompt: Option<String>,
}

/// A longer dictionary wouldn't fit anyway (Whisper takes about 220 prompt tokens).
const MAX_PROMPT_CHARS: usize = 600;

/// Words for comparison: lowercase, no punctuation.
fn plain(text: &str) -> String {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// On audio without speech Whisper can "read out" the prompt itself (or a piece of it). A single
/// word is kept — it's more often a real call by name ("Borys?") than an echo; whole
/// words are compared, so "ci" doesn't match "CI/CD", nor "se" match "Hisense".
fn is_prompt_echo(text: &str, prompt: &str) -> bool {
    let t = plain(text);
    t.contains(' ') && format!(" {} ", plain(prompt)).contains(&format!(" {t} "))
}

/// Threads for language detection (whisper.cpp takes min(4, cores) itself when decoding).
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
        let path = path.to_str().with_context(|| crate::i18n::t("whisper.path_not_utf8"))?;
        let context = WhisperContext::new_with_params(path, params).map_err(|e| anyhow!("{e}"))?;
        let state = context.create_state().map_err(|e| anyhow!("{e}"))?;
        Ok(Self { state, _context: context, prompt: None })
    }

    /// Dictionary of names and terms (e.g. "NPaw, Hisense, Tizen, CI/CD"); empty = no prompt.
    pub fn set_vocabulary(&mut self, vocabulary: &str) {
        let v = vocabulary.split_whitespace().collect::<Vec<_>>().join(" ");
        self.prompt = (!v.is_empty()).then(|| v.chars().take(MAX_PROMPT_CHARS).collect());
    }

    /// The most likely language of the utterance among `candidates` (whisper.cpp codes).
    pub fn detect_language<'a>(&mut self, samples: &[f32], candidates: &[&'a str]) -> Result<&'a str> {
        self.state.pcm_to_mel(samples, threads()).map_err(|e| anyhow!("{e}"))?;
        let (_, probs) = self.state.lang_detect(0, threads()).map_err(|e| anyhow!("{e}"))?;
        candidates
            .iter()
            .filter_map(|&code| Some((code, *probs.get(whisper_rs::get_lang_id(code)? as usize)?)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(code, _)| code)
            .ok_or_else(|| anyhow!(crate::i18n::t("whisper.no_language")))
    }

    /// `language: None` = whisper.cpp detects the language itself (among all of them).
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
            let segment = self.state.get_segment(i).ok_or_else(|| anyhow!(crate::i18n::t_with("whisper.no_segment", &[("index", &i)])))?;
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
        // A single word (even from the dictionary) is a real utterance.
        assert!(!is_prompt_echo("Klaudiusz?", prompt));
        assert!(!is_prompt_echo("Ci", prompt));
        // Whole words only: "sense tizen" is not a piece of "Hisense, Tizen".
        assert!(!is_prompt_echo("sense Tizen", prompt));
    }
}
