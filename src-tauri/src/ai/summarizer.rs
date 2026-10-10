//! Transcript summary via the chosen provider. Short transcript → a single request;
//! longer than the model fits → notes from successive parts (with timestamps) and combining them.
use chrono::{DateTime, Datelike, Local};
use std::path::{Path, PathBuf};

use super::provider::{LlmConfig, LlmError};
use crate::settings::ProviderId;

pub const MAX_TOKENS: u32 = 16_000;

pub const DEFAULT_PROMPT: &str = "Jesteś asystentem, który przygotowuje notatki ze spotkań. Na podstawie transkryptu przygotuj \
podsumowanie po polsku w Markdown z sekcjami:\n\
\n\
## Streszczenie\n\
3–6 zdań: o czym było spotkanie i z jakim wynikiem.\n\
\n\
## Decyzje\n\
Lista podjętych decyzji (kto zdecydował, jeśli wiadomo).\n\
\n\
## Zadania\n\
Lista w formie „- [ ] Kto — co — termin”. Brak osoby lub terminu oznacz jako „nie ustalono”.\n\
\n\
## Otwarte pytania\n\
Sprawy bez odpowiedzi lub do wyjaśnienia.\n\
\n\
## Ryzyka\n\
Problemy, zagrożenia i zależności, o których mówiono.\n\
\n\
Zasady: opieraj się wyłącznie na transkrypcie, niczego nie dopowiadaj. Mówców nazywaj tak jak \
w transkrypcie („Ja”, „Rozmówca 1”…), chyba że z rozmowy wynika ich imię. Przy kluczowych \
ustaleniach podawaj znacznik czasu z transkryptu, np. [00:12:34]. Pustą sekcję oznacz „brak”. \
Transkrypt pochodzi z automatycznego rozpoznawania mowy i może zawierać błędy — popraw oczywiste \
przekręcenia słów, ale nie zmieniaj sensu.";

pub fn partial_instruction(part: usize, total: usize) -> String {
    format!(
        "To jest część {part} z {total} długiego transkryptu. Zrób szczegółowe notatki tej części (decyzje, zadania \
z osobami i terminami, otwarte pytania, ryzyka, ważne liczby) ze znacznikami czasu. To nie jest \
jeszcze końcowe podsumowanie — nic nie pomijaj."
    )
}

pub const FINAL_INSTRUCTION: &str = "Poniżej są notatki z kolejnych części jednego spotkania (w kolejności). Złóż z nich jedno końcowe \
podsumowanie całego spotkania w wymaganym formacie, łącząc powtórzenia.";

/// Prompt from settings, or the default when empty.
pub fn effective_prompt(custom: Option<&str>) -> &str {
    match custom {
        Some(p) if !p.trim().is_empty() => p,
        _ => DEFAULT_PROMPT,
    }
}

/// Splits the transcript into parts ≤ `limit` characters, cutting only between lines (utterances).
/// A single line longer than the limit stays whole in a separate part.
pub fn chunks(transcript: &str, limit: usize) -> Vec<String> {
    if transcript.chars().count() <= limit {
        return vec![transcript.to_string()];
    }
    let mut parts = Vec::new();
    let (mut current, mut current_len) = (String::new(), 0usize);
    for line in transcript.split('\n') {
        let line_len = line.chars().count();
        if !current.is_empty() && current_len + line_len + 1 > limit {
            parts.push(std::mem::take(&mut current));
            current_len = 0;
        }
        if !current.is_empty() {
            current.push('\n');
            current_len += 1;
        }
        current.push_str(line);
        current_len += line_len;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Something that answers (system, user) — `LlmConfig` in the app, a mock in tests.
pub(crate) trait Completer {
    async fn complete(&self, system: &str, user: &str, max_tokens: u32) -> Result<String, LlmError>;
}

impl Completer for LlmConfig {
    async fn complete(&self, system: &str, user: &str, max_tokens: u32) -> Result<String, LlmError> {
        LlmConfig::complete(self, system, user, max_tokens).await
    }
}

/// Transcript summary. `progress(done, total)` counts model requests:
/// one for a short transcript, otherwise the parts + combining.
pub async fn summarize(
    config: &LlmConfig,
    transcript_markdown: &str,
    custom_prompt: Option<&str>,
    progress: impl FnMut(usize, usize),
) -> Result<String, LlmError> {
    summarize_with(config, config.provider, transcript_markdown, effective_prompt(custom_prompt), progress).await
}

pub(crate) async fn summarize_with<C: Completer>(
    llm: &C,
    provider: ProviderId,
    transcript: &str,
    prompt: &str,
    mut progress: impl FnMut(usize, usize),
) -> Result<String, LlmError> {
    let parts = chunks(transcript, provider.chunk_characters());
    if parts.len() == 1 {
        progress(0, 1);
        let out = llm.complete(prompt, &format!("Transkrypt spotkania:\n\n{transcript}"), MAX_TOKENS).await?;
        progress(1, 1);
        return Ok(out);
    }
    let total = parts.len() + 1;
    let mut notes = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        progress(i, total);
        let user = format!("{}\n\n{part}", partial_instruction(i + 1, parts.len()));
        notes.push(llm.complete(prompt, &user, MAX_TOKENS).await?);
    }
    progress(parts.len(), total);
    let joined = notes
        .iter()
        .enumerate()
        .map(|(i, n)| format!("### Część {}\n{n}", i + 1))
        .collect::<Vec<_>>()
        .join("\n\n");
    let out = llm.complete(prompt, &format!("{FINAL_INSTRUCTION}\n\n{joined}"), MAX_TOKENS).await?;
    progress(total, total);
    Ok(out)
}

/// Filename-safe model: everything outside `[A-Za-z0-9._-]` → `-`.
pub fn safe_model(model: &str) -> String {
    model
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect()
}

/// File name in `summaries/`: `<yyyy-mm-dd_hh-mm-ss>-<provider>-<model>.md` (history — we overwrite nothing).
pub fn summary_file_name(provider: ProviderId, model: &str, now: DateTime<Local>) -> String {
    format!("{}-{}-{}.md", now.format("%Y-%m-%d_%H-%M-%S"), provider.key(), safe_model(model))
}

const MONTHS_SHORT: [&str; 12] = ["sty", "lut", "mar", "kwi", "maj", "cze", "lip", "sie", "wrz", "paź", "lis", "gru"];

/// Short Polish date like `PolishDate.short`: "5 paź, 14:03", with the year if not the current one.
pub fn polish_short(date: DateTime<Local>, now: DateTime<Local>) -> String {
    let month = MONTHS_SHORT[date.month0() as usize];
    if date.year() == now.year() {
        format!("{} {month}, {}", date.day(), date.format("%H:%M"))
    } else {
        format!("{} {month} {}, {}", date.day(), date.year(), date.format("%H:%M"))
    }
}

/// File content: header with provider, model and date, then the summary.
pub fn summary_document(provider: ProviderId, model: &str, summary: &str, now: DateTime<Local>) -> String {
    format!(
        "> Podsumowanie wygenerowane przez {} ({model}) · {}\n\n{summary}\n",
        provider.display_name(),
        polish_short(now, now)
    )
}

/// Saves the summary to `folder` (e.g. `<meeting>/summaries`) and returns the file path.
pub fn save(summary: &str, folder: &Path, config: &LlmConfig, now: DateTime<Local>) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let path = folder.join(summary_file_name(config.provider, &config.model, now));
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, summary_document(config.provider, &config.model, summary, now))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Newest summary in the folder (files have the date in their name, so sorting is enough).
#[allow(dead_code)]
pub fn latest(folder: &Path) -> Option<PathBuf> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".md"))
        .collect();
    names.sort();
    names.pop().map(|n| folder.join(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::cell::RefCell;

    struct Fake {
        calls: RefCell<Vec<(String, String)>>,
    }

    impl Fake {
        fn new() -> Self {
            Self { calls: RefCell::new(Vec::new()) }
        }
    }

    impl Completer for Fake {
        async fn complete(&self, system: &str, user: &str, _max: u32) -> Result<String, LlmError> {
            let mut calls = self.calls.borrow_mut();
            calls.push((system.to_string(), user.to_string()));
            Ok(format!("notatka {}", calls.len()))
        }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    #[test]
    fn chunks_split_only_between_lines() {
        let transcript = (1..=100).map(|i| format!("[00:00:{i}] **Ja:** zdanie numer {i}")).collect::<Vec<_>>().join("\n");
        let parts = chunks(&transcript, 500);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.chars().count() <= 500));
        assert!(parts.iter().all(|p| p.split('\n').all(|l| l.starts_with("[00:00:"))), "linie całe");
        assert_eq!(parts.join("\n"), transcript, "nic nie ginie ani się nie dubluje");
    }

    #[test]
    fn chunks_short_is_single_and_counts_chars_not_bytes() {
        assert_eq!(chunks("krótko", 100), ["krótko"]);
        let s = "ż".repeat(10); // 20 bytes, 10 characters
        assert_eq!(chunks(&s, 10), [s.clone()]);
        // A line longer than the limit is not cut.
        assert_eq!(chunks(&format!("a\n{}\nb", "x".repeat(20)), 5), ["a", &"x".repeat(20), "b"]);
    }

    #[test]
    fn short_transcript_single_call() {
        let fake = Fake::new();
        let mut steps = Vec::new();
        let out = block_on(summarize_with(&fake, ProviderId::Anthropic, "krótko", "P", |d, t| steps.push((d, t)))).unwrap();
        assert_eq!(out, "notatka 1");
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], ("P".to_string(), "Transkrypt spotkania:\n\nkrótko".to_string()));
        assert_eq!(steps, [(0, 1), (1, 1)]);
    }

    #[test]
    fn long_transcript_map_reduce() {
        let fake = Fake::new();
        let transcript = (0..4_000)
            .map(|i| format!("[00:00:00] **Rozmówca 1:** {}{i}", "słowo ".repeat(10)))
            .collect::<Vec<_>>()
            .join("\n");
        let mut steps = Vec::new();
        block_on(summarize_with(&fake, ProviderId::Openai, &transcript, "P", |d, t| steps.push((d, t)))).unwrap();
        let parts = chunks(&transcript, ProviderId::Openai.chunk_characters()).len();
        assert!(parts > 1);
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), parts + 1, "części + złożenie");
        assert!(calls[0].1.starts_with(&format!("To jest część 1 z {parts} długiego transkryptu.")));
        let last = &calls.last().unwrap().1;
        assert!(last.starts_with(FINAL_INSTRUCTION));
        assert!(last.contains("### Część 1\nnotatka 1\n\n### Część 2\nnotatka 2"));
        assert_eq!(steps.first(), Some(&(0, parts + 1)));
        assert_eq!(steps.last(), Some(&(parts + 1, parts + 1)));
    }

    #[test]
    fn prompt_fallback() {
        assert_eq!(effective_prompt(None), DEFAULT_PROMPT);
        assert_eq!(effective_prompt(Some("  ")), DEFAULT_PROMPT);
        assert_eq!(effective_prompt(Some("Mój")), "Mój");
        assert!(DEFAULT_PROMPT.contains("przygotuj podsumowanie po polsku w Markdown z sekcjami:\n\n## Streszczenie\n3–6 zdań"));
        assert!(DEFAULT_PROMPT.ends_with("popraw oczywiste przekręcenia słów, ale nie zmieniaj sensu."));
        assert!(!DEFAULT_PROMPT.contains("  "), "kontynuacje linii bez podwójnych spacji");
    }

    #[test]
    fn file_naming() {
        let now = Local.with_ymd_and_hms(2026, 10, 5, 14, 3, 9).unwrap();
        assert_eq!(
            summary_file_name(ProviderId::Openrouter, "anthropic/claude opus:5", now),
            "2026-10-05_14-03-09-openrouter-anthropic-claude-opus-5.md"
        );
        assert_eq!(summary_file_name(ProviderId::Zai, "glm-5.3_x", now), "2026-10-05_14-03-09-zai-glm-5.3_x.md");
        assert_eq!(safe_model("żółw"), "---w");
    }

    #[test]
    fn document_header() {
        let now = Local.with_ymd_and_hms(2026, 10, 5, 14, 3, 9).unwrap();
        assert_eq!(
            summary_document(ProviderId::Zai, "glm-5", "## Streszczenie", now),
            "> Podsumowanie wygenerowane przez Z.AI (GLM) (glm-5) · 5 paź, 14:03\n\n## Streszczenie\n"
        );
        let old = Local.with_ymd_and_hms(2025, 1, 31, 9, 5, 0).unwrap();
        assert_eq!(polish_short(old, now), "31 sty 2025, 09:05");
    }

    #[test]
    fn save_and_latest() {
        let dir = std::env::temp_dir().join(format!("dyktandox-sum-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(latest(&dir), None);
        let c = LlmConfig { provider: ProviderId::Anthropic, api_key: String::new(), model: "claude-opus-5-5".into(), base_url: String::new() };
        let a = save("A", &dir, &c, Local.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap()).unwrap();
        let b = save("B", &dir, &c, Local.with_ymd_and_hms(2026, 2, 1, 10, 0, 0).unwrap()).unwrap();
        assert_ne!(a, b);
        assert_eq!(latest(&dir), Some(b.clone()));
        assert!(std::fs::read_to_string(&b).unwrap().ends_with("\n\nB\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
