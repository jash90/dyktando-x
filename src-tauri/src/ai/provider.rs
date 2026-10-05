//! Klient dostawców AI. OpenAI, OpenRouter i Z.AI mówią protokołem OpenAI Chat Completions;
//! Anthropic ma własne Messages API. Budowa zapytań i parsowanie odpowiedzi są czyste (testy bez sieci).
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

use crate::settings::{ProviderId, Settings};

pub const COMPLETION_TIMEOUT: Duration = Duration::from_secs(600);
pub const MODELS_TIMEOUT: Duration = Duration::from_secs(30);
/// Próby wysłania przy 429 / 5xx / 529 (przeciążenie) i błędach sieci.
pub const ATTEMPTS: u32 = 3;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Odmowa klasyfikatora bezpieczeństwa → API samo powtarza zapytanie na zalecanym modelu zastępczym.
pub const ANTHROPIC_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Modele z serwerowym `fallbacks: "default"` (rodzina Opus 5 / Opus 5.5 / Sonnet 5.5 / Fable 5.1).
pub const ANTHROPIC_FALLBACK_MODELS: [&str; 4] =
    ["claude-opus-5", "claude-opus-5-5", "claude-sonnet-5-5", "claude-fable-5-1"];
const ANTHROPIC_HOST: &str = "api.anthropic.com";

pub type Headers = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LlmError {
    pub message: String,
    /// Czy warto ponowić (429, 5xx, sieć).
    pub retryable: bool,
}

impl LlmError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: false }
    }

    fn retryable(message: impl Into<String>) -> Self {
        Self { message: message.into(), retryable: true }
    }

    fn network(e: impl std::fmt::Display) -> Self {
        Self::retryable(format!("Błąd sieci: {e}"))
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LlmError {}

/// `output_config.effort` — błąd 400 na Haiku 4.5, Sonnet 4.5 i starszych; obsługują go nowsze modele.
pub fn anthropic_supports_effort(model: &str) -> bool {
    const SUPPORTED: [&str; 9] = [
        "claude-fable-", "claude-mythos-", "claude-opus-5", "claude-sonnet-5",
        "claude-opus-4-5", "claude-opus-4-6", "claude-opus-4-7", "claude-opus-4-8", "claude-sonnet-4-6",
    ];
    SUPPORTED.iter().any(|p| model.starts_with(p))
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmConfig {
    pub provider: ProviderId,
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

impl LlmConfig {
    /// Pełna konfiguracja albo czytelny błąd, czego brakuje (`api_key` z `ai::keys::get`).
    pub fn from_settings(settings: &Settings, id: ProviderId, api_key: Option<String>) -> Result<Self, LlmError> {
        let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
            return Err(LlmError::new(format!(
                "Brak klucza API dla {} — dodaj go w Ustawieniach → AI.",
                id.display_name()
            )));
        };
        let (model, base_url) = settings.provider(id);
        if model.is_empty() {
            return Err(LlmError::new(format!("Wybierz model dla {} w Ustawieniach → AI.", id.display_name())));
        }
        Ok(Self { provider: id, api_key, model, base_url })
    }

    /// Jak `from_settings`, z kluczem z pęku kluczy.
    pub fn load(settings: &Settings, id: ProviderId) -> Result<Self, LlmError> {
        Self::from_settings(settings, id, super::keys::get(id))
    }

    /// Adres API bez białych znaków i ukośników na brzegach; niepoprawny → domyślny dostawcy.
    pub fn base(&self) -> String {
        let trimmed = self.base_url.trim().trim_matches('/');
        match reqwest::Url::parse(trimmed) {
            Ok(_) => trimmed.to_string(),
            Err(_) => self.provider.default_base_url().to_string(),
        }
    }

    fn host(&self) -> Option<String> {
        reqwest::Url::parse(&self.base()).ok()?.host_str().map(str::to_string)
    }

    fn is_anthropic(&self) -> bool {
        self.provider == ProviderId::Anthropic
    }

    /// Fallback tylko do oficjalnego API Anthropic (bramki/proxy pod innym adresem mogą go nie znać).
    pub fn uses_fallbacks(&self) -> bool {
        self.is_anthropic()
            && ANTHROPIC_FALLBACK_MODELS.contains(&self.model.as_str())
            && self.host().as_deref() == Some(ANTHROPIC_HOST)
    }

    fn auth_headers(&self) -> Headers {
        let mut h: Headers = Vec::new();
        if self.is_anthropic() {
            h.push(("x-api-key".into(), self.api_key.clone()));
            h.push(("anthropic-version".into(), ANTHROPIC_VERSION.into()));
        } else {
            h.push(("Authorization".into(), format!("Bearer {}", self.api_key)));
            if self.provider == ProviderId::Openrouter {
                // Atrybucja w panelu OpenRouter (opcjonalna).
                h.push(("X-Title".into(), "Dyktando X".into()));
            }
        }
        h
    }

    /// Zapytanie o odpowiedź: (URL, nagłówki, ciało JSON).
    pub fn build_request(&self, system: &str, user: &str, max_tokens: u32) -> (String, Headers, Value) {
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        headers.extend(self.auth_headers());
        if self.is_anthropic() {
            let mut body = json!({
                "model": self.model,
                "max_tokens": max_tokens,
                "system": system,
                "messages": [{"role": "user", "content": user}],
            });
            // Claude Opus 5.5 ma domyślnie effort „medium” — ustawiamy jawnie tam, gdzie model to obsługuje.
            if anthropic_supports_effort(&self.model) {
                body["output_config"] = json!({"effort": "medium"});
            }
            if self.uses_fallbacks() {
                body["fallbacks"] = json!("default");
                headers.push(("anthropic-beta".into(), ANTHROPIC_FALLBACK_BETA.into()));
            }
            (format!("{}/messages", self.base()), headers, body)
        } else {
            let mut body = json!({
                "model": self.model,
                "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            });
            // Nowsze modele OpenAI przyjmują tylko max_completion_tokens; OpenRouter i Z.AI — max_tokens.
            let field = if self.provider == ProviderId::Openai { "max_completion_tokens" } else { "max_tokens" };
            body[field] = json!(max_tokens);
            (format!("{}/chat/completions", self.base()), headers, body)
        }
    }

    /// Zapytanie o listę modeli: (URL, nagłówki). Anthropic bez nagłówka beta.
    pub fn build_models_request(&self) -> (String, Headers) {
        (format!("{}/models", self.base()), self.auth_headers())
    }

    pub async fn complete(&self, system: &str, user: &str, max_tokens: u32) -> Result<String, LlmError> {
        let (url, headers, body) = self.build_request(system, user, max_tokens);
        let json = send(&url, &headers, &body, retry_delay).await?;
        if self.is_anthropic() { parse_anthropic(&json) } else { parse_openai(&json) }
    }

    pub async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let (url, headers) = self.build_models_request();
        let client = client(MODELS_TIMEOUT)?;
        let resp = client.get(&url).headers(header_map(&headers)?).send().await.map_err(LlmError::network)?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(LlmError::network)?;
        let json = to_json(&bytes);
        if !(200..300).contains(&status) {
            return Err(http_error(status, &json, &bytes));
        }
        Ok(parse_models(self.provider, &json))
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client, LlmError> {
    reqwest::Client::builder().timeout(timeout).build().map_err(LlmError::network)
}

fn header_map(headers: &Headers) -> Result<reqwest::header::HeaderMap, LlmError> {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| LlmError::new(format!("Niepoprawny nagłówek: {k}")))?;
        // Bez wartości w komunikacie — to może być klucz API.
        let value = reqwest::header::HeaderValue::from_str(v)
            .map_err(|_| LlmError::new(format!("Niepoprawna wartość nagłówka {k} — sprawdź klucz API")))?;
        map.insert(name, value);
    }
    Ok(map)
}

/// Ciało odpowiedzi jako obiekt JSON; cokolwiek innego → `{}`.
fn to_json(bytes: &[u8]) -> Value {
    serde_json::from_slice::<Value>(bytes).ok().filter(Value::is_object).unwrap_or_else(|| json!({}))
}

/// Odstęp przed kolejną próbą: `retry-after` z serwera, inaczej 2^(próba+1) s.
fn retry_delay(attempt: u32, retry_after: Option<f64>) -> Duration {
    let secs = retry_after.unwrap_or_else(|| 2f64.powi(attempt as i32 + 1));
    Duration::from_secs_f64(if secs.is_finite() && secs > 0.0 { secs } else { 0.0 })
}

/// POST z ponawianiem dla błędów `retryable` i błędów sieci.
async fn send(
    url: &str,
    headers: &Headers,
    body: &Value,
    delay: impl Fn(u32, Option<f64>) -> Duration,
) -> Result<Value, LlmError> {
    let client = client(COMPLETION_TIMEOUT)?;
    let headers = header_map(headers)?;
    let payload = serde_json::to_vec(body).map_err(|e| LlmError::new(e.to_string()))?;
    let mut last = LlmError::new("Brak odpowiedzi");
    for attempt in 0..ATTEMPTS {
        let is_last = attempt + 1 == ATTEMPTS;
        let result = async {
            let resp = client.post(url).headers(headers.clone()).body(payload.clone()).send().await?;
            let status = resp.status().as_u16();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<f64>().ok());
            let bytes = resp.bytes().await?;
            Ok::<_, reqwest::Error>((status, retry_after, bytes))
        }
        .await;
        match result {
            Ok((status, retry_after, bytes)) => {
                let json = to_json(&bytes);
                if (200..300).contains(&status) {
                    return Ok(json);
                }
                let error = http_error(status, &json, &bytes);
                if !error.retryable || is_last {
                    return Err(error);
                }
                log::warn!("AI: {} — ponawiam (próba {}/{ATTEMPTS})", error.message, attempt + 2);
                last = error;
                tokio::time::sleep(delay(attempt, retry_after)).await;
            }
            Err(e) => {
                last = LlmError::network(e);
                if is_last {
                    return Err(last);
                }
                log::warn!("AI: {} — ponawiam (próba {}/{ATTEMPTS})", last.message, attempt + 2);
                tokio::time::sleep(delay(attempt, None)).await;
            }
        }
    }
    Err(last)
}

/// Czytelny błąd HTTP z treścią komunikatu dostawcy.
pub fn http_error(status: u16, json: &Value, body: &[u8]) -> LlmError {
    let detail = json["error"]["message"]
        .as_str()
        .or_else(|| json["error"].as_str())
        .or_else(|| json["message"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| String::from_utf8_lossy(&body[..body.len().min(300)]).into_owned());
    match status {
        401 | 403 => LlmError::new(format!("Klucz API odrzucony ({status}): {detail}")),
        402 => LlmError::new(format!("Brak środków na koncie dostawcy (402): {detail}")),
        404 => LlmError::new(format!("Nie znaleziono (404) — sprawdź model i adres API: {detail}")),
        429 => LlmError::retryable(format!("Limit zapytań (429): {detail}")),
        500..=599 => LlmError::retryable(format!("Błąd serwera dostawcy ({status}): {detail}")),
        _ => LlmError::new(format!("Błąd {status}: {detail}")),
    }
}

const LENGTH_LIMIT: &str = "Model skończył się na limicie długości — brak treści";

/// Odpowiedź Chat Completions (OpenAI / OpenRouter / Z.AI).
pub fn parse_openai(json: &Value) -> Result<String, LlmError> {
    let choice = &json["choices"][0];
    if !choice["message"].is_object() {
        return Err(LlmError::new("Nieoczekiwana odpowiedź dostawcy (brak choices)"));
    }
    let text = choice["message"]["content"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        let reason = choice["finish_reason"].as_str().unwrap_or("?");
        return Err(LlmError::new(if reason == "length" {
            LENGTH_LIMIT.to_string()
        } else {
            format!("Pusta odpowiedź modelu (finish_reason: {reason})")
        }));
    }
    Ok(text.to_string())
}

/// Odpowiedź Anthropic Messages API (bloki `text`; `thinking` i inne pomijane).
pub fn parse_anthropic(json: &Value) -> Result<String, LlmError> {
    let stop_reason = json["stop_reason"].as_str();
    // Odmowa przychodzi jako HTTP 200 — sprawdzamy stop_reason, zanim przeczytamy treść.
    if stop_reason == Some("refusal") {
        let category = json["stop_details"]["category"]
            .as_str()
            .map(|c| format!(" (kategoria: {c})"))
            .unwrap_or_default();
        return Err(LlmError::new(format!("Model odmówił odpowiedzi{category}")));
    }
    let text: String = json["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"].as_str() == Some("text"))
                .filter_map(|b| b["text"].as_str())
                .collect()
        })
        .unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return Err(LlmError::new(if stop_reason == Some("max_tokens") {
            LENGTH_LIMIT.to_string()
        } else {
            format!("Pusta odpowiedź modelu (stop_reason: {})", stop_reason.unwrap_or("?"))
        }));
    }
    Ok(text.to_string())
}

/// Identyfikatory z `data[].id`; u dostawców zgodnych z OpenAI posortowane (Anthropic — kolejność API).
pub fn parse_models(provider: ProviderId, json: &Value) -> Vec<String> {
    let mut ids: Vec<String> = json["data"]
        .as_array()
        .map(|items| items.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if provider != ProviderId::Anthropic {
        ids.sort();
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn config(p: ProviderId, model: &str) -> LlmConfig {
        LlmConfig { provider: p, api_key: "sk-test".into(), model: model.into(), base_url: p.default_base_url().into() }
    }

    fn header<'a>(h: &'a Headers, name: &str) -> Option<&'a str> {
        h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    // Budowa zapytań

    #[test]
    fn openai_request() {
        let (url, h, b) = config(ProviderId::Openai, "m-1").build_request("S", "U", 900);
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
        assert_eq!(header(&h, "Authorization"), Some("Bearer sk-test"));
        assert_eq!(header(&h, "X-Title"), None);
        assert_eq!(b["model"], "m-1");
        assert_eq!(b["max_completion_tokens"], 900);
        assert!(b.get("max_tokens").is_none());
        assert_eq!(b["messages"], json!([{"role": "system", "content": "S"}, {"role": "user", "content": "U"}]));
    }

    #[test]
    fn openrouter_and_zai_requests() {
        let (url, h, b) = config(ProviderId::Openrouter, "m-1").build_request("S", "U", 10);
        assert_eq!(url, "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(header(&h, "X-Title"), Some("Dyktando X"));
        assert_eq!(b["max_tokens"], 10);
        assert!(b.get("max_completion_tokens").is_none());

        let (url, h, b) = config(ProviderId::Zai, "m-1").build_request("S", "U", 10);
        assert_eq!(url, "https://api.z.ai/api/paas/v4/chat/completions");
        assert_eq!(header(&h, "X-Title"), None);
        assert_eq!(b["max_tokens"], 10);
    }

    #[test]
    fn custom_base_url_trailing_slash_ignored() {
        let mut c = config(ProviderId::Openai, "m-1");
        c.base_url = " http://localhost:1234/v1/ ".into();
        assert_eq!(c.build_request("", "", 1).0, "http://localhost:1234/v1/chat/completions");
        c.base_url = "nie adres".into();
        assert_eq!(c.base(), "https://api.openai.com/v1");
    }

    #[test]
    fn anthropic_request() {
        let (url, h, b) = config(ProviderId::Anthropic, "claude-opus-5-5").build_request("S", "U", 16_000);
        assert_eq!(url, "https://api.anthropic.com/v1/messages");
        assert_eq!(header(&h, "x-api-key"), Some("sk-test"));
        assert_eq!(header(&h, "anthropic-version"), Some("2023-06-01"));
        assert_eq!(header(&h, "anthropic-beta"), Some("server-side-fallback-2026-07-01"));
        assert_eq!(header(&h, "Authorization"), None);
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["system"], "S");
        assert_eq!(b["max_tokens"], 16_000);
        assert_eq!(b["messages"], json!([{"role": "user", "content": "U"}]));
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(b["output_config"]["effort"], "medium");
        assert!(b.get("thinking").is_none(), "Opus 5.5: thinking zostaje domyślne (adaptive)");
    }

    #[test]
    fn anthropic_haiku_no_effort_no_fallbacks() {
        let (_, h, b) = config(ProviderId::Anthropic, "claude-haiku-4-5-20251001").build_request("S", "U", 100);
        assert!(b.get("output_config").is_none(), "Haiku 4.5 zwraca 400 na effort");
        assert!(b.get("fallbacks").is_none());
        assert_eq!(header(&h, "anthropic-beta"), None);
    }

    #[test]
    fn anthropic_sonnet46_effort_but_no_fallbacks() {
        let (_, h, b) = config(ProviderId::Anthropic, "claude-sonnet-4-6").build_request("S", "U", 100);
        assert_eq!(b["output_config"]["effort"], "medium");
        assert!(b.get("fallbacks").is_none());
        assert_eq!(header(&h, "anthropic-beta"), None);
    }

    #[test]
    fn anthropic_custom_host_no_fallbacks() {
        let mut c = config(ProviderId::Anthropic, "claude-opus-5-5");
        c.base_url = "https://gateway.example.com/v1".into();
        let (url, h, b) = c.build_request("S", "U", 100);
        assert!(b.get("fallbacks").is_none());
        assert_eq!(header(&h, "anthropic-beta"), None);
        assert_eq!(url, "https://gateway.example.com/v1/messages");
    }

    #[test]
    fn models_requests() {
        let (url, h) = config(ProviderId::Zai, "").build_models_request();
        assert_eq!(url, "https://api.z.ai/api/paas/v4/models");
        assert_eq!(header(&h, "Authorization"), Some("Bearer sk-test"));
        let (url, h) = config(ProviderId::Anthropic, "claude-opus-5-5").build_models_request();
        assert_eq!(url, "https://api.anthropic.com/v1/models");
        assert_eq!(header(&h, "x-api-key"), Some("sk-test"));
        assert_eq!(header(&h, "anthropic-beta"), None);
    }

    #[test]
    fn config_from_settings() {
        let s = Settings::default();
        let c = LlmConfig::from_settings(&s, ProviderId::Anthropic, Some("k".into())).unwrap();
        assert_eq!((c.model.as_str(), c.base_url.as_str()), ("claude-opus-5-5", "https://api.anthropic.com/v1"));
        let e = LlmConfig::from_settings(&s, ProviderId::Anthropic, Some(String::new())).unwrap_err();
        assert_eq!(e.message, "Brak klucza API dla Anthropic — dodaj go w Ustawieniach → AI.");
        let e = LlmConfig::from_settings(&s, ProviderId::Zai, Some("k".into())).unwrap_err();
        assert_eq!(e.message, "Wybierz model dla Z.AI (GLM) w Ustawieniach → AI.");
    }

    // Odpowiedzi

    #[test]
    fn parse_anthropic_joins_text_blocks_skips_thinking() {
        let j = json!({"stop_reason": "end_turn", "content": [
            {"type": "thinking", "thinking": ""}, {"type": "text", "text": "## Streszczenie\n"}, {"type": "text", "text": "Ok."}]});
        assert_eq!(parse_anthropic(&j).unwrap(), "## Streszczenie\nOk.");
    }

    #[test]
    fn parse_anthropic_refusal_checked_first() {
        let j = json!({"stop_reason": "refusal", "content": [{"type": "text", "text": "x"}], "stop_details": {"category": "cyber"}});
        assert_eq!(parse_anthropic(&j).unwrap_err().message, "Model odmówił odpowiedzi (kategoria: cyber)");
        let j = json!({"stop_reason": "refusal", "content": []});
        assert_eq!(parse_anthropic(&j).unwrap_err().message, "Model odmówił odpowiedzi");
    }

    #[test]
    fn parse_anthropic_empty_and_length() {
        let j = json!({"stop_reason": "max_tokens", "content": [{"type": "text", "text": "  "}]});
        assert_eq!(parse_anthropic(&j).unwrap_err().message, LENGTH_LIMIT);
        let j = json!({"stop_reason": "end_turn", "content": []});
        assert_eq!(parse_anthropic(&j).unwrap_err().message, "Pusta odpowiedź modelu (stop_reason: end_turn)");
        assert_eq!(parse_anthropic(&json!({})).unwrap_err().message, "Pusta odpowiedź modelu (stop_reason: ?)");
    }

    #[test]
    fn parse_openai_cases() {
        let j = json!({"choices": [{"message": {"content": " Podsumowanie "}, "finish_reason": "stop"}]});
        assert_eq!(parse_openai(&j).unwrap(), "Podsumowanie");
        let j = json!({"choices": [{"message": {"content": ""}, "finish_reason": "length"}]});
        assert_eq!(parse_openai(&j).unwrap_err().message, LENGTH_LIMIT);
        let j = json!({"choices": [{"message": {"content": null}, "finish_reason": "content_filter"}]});
        assert_eq!(parse_openai(&j).unwrap_err().message, "Pusta odpowiedź modelu (finish_reason: content_filter)");
        assert_eq!(parse_openai(&json!({})).unwrap_err().message, "Nieoczekiwana odpowiedź dostawcy (brak choices)");
    }

    #[test]
    fn parse_models_sorting() {
        let j = json!({"data": [{"id": "b"}, {"id": "a"}, {"x": 1}]});
        assert_eq!(parse_models(ProviderId::Zai, &j), ["a", "b"]);
        assert_eq!(parse_models(ProviderId::Anthropic, &j), ["b", "a"]);
        assert!(parse_models(ProviderId::Openai, &json!({})).is_empty());
    }

    // Błędy HTTP

    #[test]
    fn http_error_mapping() {
        let j = json!({"error": {"message": "invalid x-api-key"}});
        let e = http_error(401, &j, b"");
        assert_eq!(e.message, "Klucz API odrzucony (401): invalid x-api-key");
        assert!(!e.retryable);
        assert!(http_error(403, &j, b"").message.starts_with("Klucz API odrzucony (403)"));
        let e = http_error(402, &json!({"error": "no money"}), b"");
        assert_eq!((e.message.as_str(), e.retryable), ("Brak środków na koncie dostawcy (402): no money", false));
        let e = http_error(404, &json!({"message": "nope"}), b"");
        assert_eq!(e.message, "Nie znaleziono (404) — sprawdź model i adres API: nope");
        let e = http_error(429, &j, b"");
        assert_eq!((e.message.as_str(), e.retryable), ("Limit zapytań (429): invalid x-api-key", true));
        let e = http_error(529, &json!({}), b"Overloaded");
        assert_eq!((e.message.as_str(), e.retryable), ("Błąd serwera dostawcy (529): Overloaded", true));
        let e = http_error(400, &json!({}), &[b'x'; 500]);
        assert_eq!(e.message, format!("Błąd 400: {}", "x".repeat(300)));
        assert!(!e.retryable);
    }

    #[test]
    fn retry_delay_honors_retry_after() {
        assert_eq!(retry_delay(0, Some(0.0)), Duration::ZERO);
        assert_eq!(retry_delay(0, Some(1.5)), Duration::from_millis(1500));
        assert_eq!(retry_delay(0, None), Duration::from_secs(2));
        assert_eq!(retry_delay(1, None), Duration::from_secs(4));
        assert_eq!(retry_delay(0, Some(-3.0)), Duration::ZERO);
    }

    // Lokalny serwer z podstawionymi odpowiedziami (127.0.0.1, żadnych prawdziwych dostawców).

    struct Stub {
        base: String,
        requests: Arc<Mutex<Vec<String>>>,
    }

    async fn stub(responses: Vec<(u16, &'static str, &'static str)>) -> Stub {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        tokio::spawn(async move {
            for (status, extra_headers, body) in responses {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Nagłówki + ciało wg Content-Length.
                loop {
                    let n = sock.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len || n == 0 {
                            log.lock().unwrap().push(text);
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                sock.shutdown().await.ok();
            }
        });
        Stub { base, requests }
    }

    fn stub_config(p: ProviderId, base: &str) -> LlmConfig {
        LlmConfig { base_url: base.into(), ..config(p, "claude-opus-5-5") }
    }

    #[tokio::test]
    async fn http401_readable_error_no_retry() {
        let s = stub(vec![(401, "", r#"{"error":{"message":"invalid x-api-key"}}"#)]).await;
        let e = stub_config(ProviderId::Anthropic, &s.base).complete("s", "u", 5).await.unwrap_err();
        assert!(e.message.contains("Klucz API odrzucony"), "{}", e.message);
        let reqs = s.requests.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].starts_with("POST /v1/messages "));
        assert!(!reqs[0].contains("anthropic-beta"), "lokalny host — bez fallbacku");
    }

    #[tokio::test]
    async fn http429_is_retried() {
        let s = stub(vec![
            (429, "retry-after: 0\r\n", r#"{"error":{"message":"slow down"}}"#),
            (200, "", r#"{"choices":[{"message":{"content":"Gotowe"},"finish_reason":"stop"}]}"#),
        ])
        .await;
        let text = stub_config(ProviderId::Openrouter, &s.base).complete("s", "u", 5).await.unwrap();
        assert_eq!(text, "Gotowe");
        let reqs = s.requests.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(reqs[1].to_ascii_lowercase().contains("x-title: dyktando x"));
    }

    #[tokio::test]
    async fn list_models_sorted() {
        let s = stub(vec![(200, "", r#"{"data":[{"id":"glm-b"},{"id":"glm-a"}]}"#)]).await;
        let models = stub_config(ProviderId::Zai, &s.base).list_models().await.unwrap();
        assert_eq!(models, ["glm-a", "glm-b"]);
        assert!(s.requests.lock().unwrap()[0].starts_with("GET /v1/models "));
    }
}
