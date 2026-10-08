//! Ustawienia aplikacji w `settings.json` (odpowiednik `Preferences` ze Swifta).
//! Brakujące pola dostają wartości domyślne, więc stare pliki zawsze się wczytają.
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
    /// Silnik sam rozpoznaje język (polski z wtrąceniami angielskimi itp.).
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
    /// Wklejaj, chyba że fokus na pewno nie jest w polu tekstowym.
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

    /// Domyślny model tylko tam, gdzie jest pewny; u pozostałych użytkownik wybiera z listy
    /// pobranej przez „Testuj połączenie” (nazwy modeli tych dostawców często się zmieniają).
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-opus-5-5",
            _ => "",
        }
    }

    pub fn model_placeholder(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-opus-5-5",
            Self::Openai => "wybierz po „Testuj połączenie”",
            Self::Openrouter => "np. anthropic/claude-opus-5-5",
            Self::Zai => "np. glm-5.3",
        }
    }

    /// Ile znaków transkryptu mieści się w jednym zapytaniu (polski ≈ 3 znaki/token).
    pub fn chunk_characters(self) -> usize {
        if self == Self::Anthropic { 1_200_000 } else { 180_000 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderConfig {
    /// Puste = domyślny model dostawcy.
    pub model: String,
    /// Puste = domyślny adres API.
    pub base_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub engine: EngineId,
    /// Język dyktowania.
    pub language: Language,
    /// Nazwa urządzenia wejściowego; `None` = domyślne systemowe.
    pub input_device: Option<String>,
    /// Skrót „przytrzymaj, aby mówić” (format handy-keys, np. `F5`, `Ctrl+Alt+Space`).
    pub shortcut_push_to_talk: String,
    /// Skrót „naciśnij, aby zacząć / zakończyć”; pusty = wyłączony.
    pub shortcut_toggle: String,
    /// Sam modyfikator jako push-to-talk, np. `CmdRight`, `CtrlRight`; pusty = wyłączony.
    pub modifier_push_to_talk: String,
    pub paste_mode: PasteMode,
    pub hud_enabled: bool,

    pub meeting_engine: EngineId,
    /// Język spotkań (transkrypcja po nagraniu, na żywo, import; źródło tłumaczenia na żywo).
    pub meeting_language: Language,
    /// Przepisuj wypowiedzi w trakcie nagrania (tekst pojawia się chwilę po każdej pauzie).
    pub meeting_live_transcription: bool,
    /// Kod języka tłumaczenia na żywo (np. „en”); pusty = bez tłumaczenia. Wymaga Canary.
    pub meeting_live_translate_to: String,
    /// Małe okno zawsze na wierzchu z licznikiem, poziomami i tekstem na żywo w trakcie nagrania.
    pub meeting_live_window: bool,
    pub meeting_diarization: bool,
    pub meeting_auto_transcribe: bool,
    pub meeting_auto_summarize: bool,
    pub meeting_audio_retention_days: u32,
    pub meeting_detection_prompt: bool,
    pub meeting_consent_reminder: bool,
    pub shortcut_meeting: String,

    pub ai_provider: ProviderId,
    pub ai_providers: BTreeMap<ProviderId, ProviderConfig>,
    /// Pusty = domyślny prompt podsumowania.
    pub ai_prompt: String,
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
            meeting_engine: EngineId::ParakeetV3,
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
            ai_provider: ProviderId::Anthropic,
            ai_providers: BTreeMap::new(),
            ai_prompt: String::new(),
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(paths::settings_file()).ok().and_then(|b| Self::parse(&b)).unwrap_or_default()
    }

    /// Plik sprzed osobnego języka spotkań: spotkania dostają dotychczasowy wspólny język,
    /// żeby po aktualizacji działały tak samo jak wcześniej.
    fn parse(bytes: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let has_meeting_language = value.get("meeting_language").is_some();
        let mut s: Self = serde_json::from_value(value).ok()?;
        if !has_meeting_language {
            s.meeting_language = s.language;
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
    }

    #[test]
    fn old_file_gives_meetings_the_shared_language() {
        let s = Settings::parse(br#"{"language":"auto"}"#).unwrap();
        assert_eq!((s.language, s.meeting_language), (Language::Auto, Language::Auto));
        let s = Settings::parse(br#"{"language":"en","meeting_language":"pl"}"#).unwrap();
        assert_eq!((s.language, s.meeting_language), (Language::En, Language::Pl));
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
