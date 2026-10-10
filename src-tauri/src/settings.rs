//! App settings in `settings.json` (the counterpart of `Preferences` in the Swift version).
//! Missing fields get default values, so old files always load.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::models::EngineId;
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Pl,
    En,
    /// The engine detects the language itself (Polish with English insertions, etc.).
    Auto,
}

impl Language {
    pub fn code(self) -> Option<&'static str> {
        match self {
            Language::Pl => Some("pl"),
            Language::En => Some("en"),
            Language::Auto => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PasteMode {
    /// Paste unless the focus is definitely not in a text field.
    #[default]
    Auto,
    Always,
    ClipboardOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Openai,
    Openrouter,
    Anthropic,
    Zai,
}

impl ProviderId {
    pub const ALL: [ProviderId; 4] = [Self::Anthropic, Self::Openai, Self::Openrouter, Self::Zai];

    pub fn key(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
            Self::Anthropic => "anthropic",
            Self::Zai => "zai",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Openai => "OpenAI",
            Self::Openrouter => "OpenRouter",
            Self::Anthropic => "Anthropic",
            Self::Zai => "Z.AI (GLM)",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Openai => "https://api.openai.com/v1",
            Self::Openrouter => "https://openrouter.ai/api/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::Zai => "https://api.z.ai/api/paas/v4",
        }
    }

    /// A default model only where it's certain; for the others the user picks from the list
    /// fetched via "Testuj połączenie" (Test connection); these providers' model names change often.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-opus-5-5",
            _ => "",
        }
    }

    pub fn model_placeholder(self) -> String {
        let example = |model: &str| crate::i18n::t_with("ai.model_placeholder_example", &[("model", &model)]);
        match self {
            Self::Anthropic => "claude-opus-5-5".into(),
            Self::Openai => crate::i18n::t("ai.openai_model_placeholder"),
            Self::Openrouter => example("anthropic/claude-opus-5-5"),
            Self::Zai => example("glm-5.3"),
        }
    }

    /// How many transcript characters fit in one request (Polish ≈ 3 characters/token).
    pub fn chunk_characters(self) -> usize {
        if self == Self::Anthropic { 1_200_000 } else { 180_000 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderConfig {
    /// Empty = the provider's default model.
    pub model: String,
    /// Empty = the default API URL.
    pub base_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub engine: EngineId,
    /// Dictation language.
    pub language: Language,
    /// Input device name; `None` = the system default.
    pub input_device: Option<String>,
    /// "Hold to talk" shortcut (handy-keys format, e.g. `F5`, `Ctrl+Alt+Space`).
    pub shortcut_push_to_talk: String,
    /// "Press to start / stop" shortcut; empty = disabled.
    pub shortcut_toggle: String,
    /// A lone modifier as push-to-talk, e.g. `CmdRight`, `CtrlRight`; empty = disabled.
    pub modifier_push_to_talk: String,
    pub paste_mode: PasteMode,
    pub hud_enabled: bool,
    /// Save every dictation (text and recording) to history — locally, like meetings.
    pub dictation_history: bool,

    /// Model for transcription after recording (and for imports).
    pub meeting_engine: EngineId,
    /// Live transcription model — speed matters here; after recording, quality does.
    pub meeting_live_engine: EngineId,
    /// Meeting language (transcription after recording, live, import; source of live translation).
    pub meeting_language: Language,
    /// Transcribe utterances during recording (text appears shortly after each pause).
    pub meeting_live_transcription: bool,
    /// Language code for live translation (e.g. "en"); empty = no translation. Requires Canary.
    pub meeting_live_translate_to: String,
    /// Small always-on-top window with a timer, levels and live text during recording.
    pub meeting_live_window: bool,
    pub meeting_diarization: bool,
    pub meeting_auto_transcribe: bool,
    pub meeting_auto_summarize: bool,
    pub meeting_audio_retention_days: u32,
    pub meeting_detection_prompt: bool,
    pub meeting_consent_reminder: bool,
    pub shortcut_meeting: String,

    /// Dictionary of names and terms (e.g. "NPaw, Hisense, Tizen, CI/CD") — a hint for Whisper
    /// in dictation and meetings.
    pub vocabulary: String,

    pub ai_provider: ProviderId,
    pub ai_providers: BTreeMap<ProviderId, ProviderConfig>,
    /// Empty = the default summary prompt.
    pub ai_prompt: String,

    /// UI language: `"system"` (follows the system language), `"en"` or `"pl"` — see `i18n::resolve`.
    /// Separate from the dictation language (`language`).
    pub ui_language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            engine: EngineId::ParakeetV3,
            language: Language::Pl,
            input_device: None,
            shortcut_push_to_talk: "F5".into(),
            shortcut_toggle: String::new(),
            modifier_push_to_talk: String::new(),
            paste_mode: PasteMode::Auto,
            hud_enabled: true,
            dictation_history: true,
            // Whisper turbo makes clearly fewer errors in conversations (compared on a daily recording:
            // 12.8% vs 19.6% word errors for Parakeet); after recording, time doesn't matter.
            meeting_engine: EngineId::WhisperTurbo,
            meeting_live_engine: EngineId::ParakeetV3,
            meeting_language: Language::Pl,
            meeting_live_transcription: true,
            meeting_live_translate_to: String::new(),
            meeting_live_window: true,
            meeting_diarization: true,
            meeting_auto_transcribe: true,
            meeting_auto_summarize: false,
            meeting_audio_retention_days: 30,
            meeting_detection_prompt: true,
            meeting_consent_reminder: true,
            shortcut_meeting: "Ctrl+Alt+R".into(),
            vocabulary: String::new(),
            ai_provider: ProviderId::Anthropic,
            ai_providers: BTreeMap::new(),
            ai_prompt: String::new(),
            ui_language: "system".into(),
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(paths::settings_file()).ok().and_then(|b| Self::parse(&b)).unwrap_or_default()
    }

    /// Old files behave after an update the same as before: from before the separate meeting
    /// language — meetings get the previous shared language; from before the separate live model —
    /// live keeps the meeting model.
    fn parse(bytes: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let has_meeting_language = value.get("meeting_language").is_some();
        let has_live_engine = value.get("meeting_live_engine").is_some();
        let mut s: Self = serde_json::from_value(value).ok()?;
        if !has_meeting_language {
            s.meeting_language = s.language;
        }
        if !has_live_engine {
            s.meeting_live_engine = s.meeting_engine;
        }
        Some(s)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = paths::settings_file();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn provider(&self, id: ProviderId) -> (String, String) {
        let cfg = self.ai_providers.get(&id).cloned().unwrap_or_default();
        let model = if cfg.model.trim().is_empty() { id.default_model().to_string() } else { cfg.model };
        let url = if cfg.base_url.trim().is_empty() { id.default_base_url().to_string() } else { cfg.base_url };
        (model, url.trim_end_matches('/').to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings = serde_json::from_str(r#"{"language":"auto"}"#).unwrap();
        assert_eq!(s.language, Language::Auto);
        assert_eq!(s.shortcut_push_to_talk, "F5");
        assert_eq!(s.meeting_audio_retention_days, 30);
        assert_eq!(s.ui_language, "system");
    }

    #[test]
    fn old_file_gives_meetings_the_shared_language() {
        let s = Settings::parse(br#"{"language":"auto"}"#).unwrap();
        assert_eq!((s.language, s.meeting_language), (Language::Auto, Language::Auto));
        let s = Settings::parse(br#"{"language":"en","meeting_language":"pl"}"#).unwrap();
        assert_eq!((s.language, s.meeting_language), (Language::En, Language::Pl));
    }

    #[test]
    fn old_file_keeps_its_meeting_model_for_live_transcription() {
        let s = Settings::parse(br#"{"meeting_engine":"canary_v2"}"#).unwrap();
        assert_eq!((s.meeting_engine, s.meeting_live_engine), (EngineId::CanaryV2, EngineId::CanaryV2));
        let s = Settings::parse(br#"{"meeting_engine":"whisper_turbo","meeting_live_engine":"parakeet_v3"}"#).unwrap();
        assert_eq!((s.meeting_engine, s.meeting_live_engine), (EngineId::WhisperTurbo, EngineId::ParakeetV3));
        // Fresh install: Whisper after recording, Parakeet live.
        let d = Settings::default();
        assert_eq!((d.meeting_engine, d.meeting_live_engine), (EngineId::WhisperTurbo, EngineId::ParakeetV3));
    }

    #[test]
    fn roundtrip() {
        let mut s = Settings::default();
        s.ai_providers.insert(ProviderId::Zai, ProviderConfig { model: "glm-5".into(), base_url: String::new() });
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.provider(ProviderId::Zai), ("glm-5".into(), "https://api.z.ai/api/paas/v4".into()));
    }
}
