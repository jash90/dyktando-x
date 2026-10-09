//! Historia dyktowania: każde dyktowanie (tekst, surowy wynik modelu i nagranie 16 kHz) zapisane
//! lokalnie, tak jak spotkania — `<dane>/Dictations/<id>.json` i `<id>.wav`. Nagranie podlega tej
//! samej retencji co audio spotkań; tekst zostaje.
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

use crate::engine::Engine;
use crate::models::EngineId;
use crate::{paths, AppState};

const RATE: u32 = 16_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub created_at: DateTime<Local>,
    pub duration_seconds: f64,
    /// Tekst po poprawkach (interpunkcja ze słów, wielkie litery) — to, co trafiło do pola.
    pub text: String,
    /// Wynik modelu przed poprawkami.
    pub raw: String,
    /// Model, którym przepisano (nazwa jak w transkryptach spotkań).
    pub engine: String,
    /// Kody języków (puste = rozpoznany przez model).
    #[serde(default)]
    pub languages: Vec<String>,
    /// Wklejone do aktywnego pola (inaczej tylko do schowka).
    pub pasted: bool,
    #[serde(default)]
    pub audio_deleted: bool,
}

pub fn new_id(at: DateTime<Local>) -> String {
    at.format("%Y-%m-%d_%H-%M-%S-%3f").to_string()
}

pub struct History {
    pub root: PathBuf,
}

impl Default for History {
    fn default() -> Self {
        Self { root: paths::dictations() }
    }
}

fn spec() -> hound::WavSpec {
    hound::WavSpec { channels: 1, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }
}

impl History {
    fn json(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    pub fn wav(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.wav"))
    }

    /// `audio`: nagranie 16 kHz mono f32.
    pub fn save(&self, entry: &Entry, audio: &[f32]) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let mut w = hound::WavWriter::create(self.wav(&entry.id), spec())?;
        for s in audio {
            w.write_sample((s * 32768.0).round().clamp(-32768.0, 32767.0) as i16)?;
        }
        w.finalize()?;
        self.write(entry)
    }

    fn write(&self, entry: &Entry) -> Result<()> {
        std::fs::write(self.json(&entry.id), serde_json::to_vec_pretty(entry)?)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Option<Entry> {
        serde_json::from_slice(&std::fs::read(self.json(id)).ok()?).ok()
    }

    /// Od najnowszych.
    pub fn all(&self) -> Vec<Entry> {
        let mut v: Vec<Entry> = std::fs::read_dir(&self.root)
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|e: &Entry| std::cmp::Reverse(e.created_at));
        v
    }

    pub fn update(&self, id: &str, f: impl FnOnce(&mut Entry)) -> Result<Entry> {
        let mut e = self.load(id).ok_or_else(|| anyhow!("Brak dyktowania {id}"))?;
        f(&mut e);
        self.write(&e)?;
        Ok(e)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let _ = std::fs::remove_file(self.wav(id));
        std::fs::remove_file(self.json(id)).with_context(|| format!("usuwanie dyktowania {id}"))
    }

    pub fn audio(&self, id: &str) -> Result<Vec<f32>> {
        let mut r = hound::WavReader::open(self.wav(id)).map_err(|_| anyhow!("Nagranie tego dyktowania zostało już usunięte"))?;
        Ok(r.samples::<i16>().map_while(|s| s.ok()).map(|s| s as f32 / 32768.0).collect())
    }

    /// Usuwa nagrania dyktowań starszych niż `days` dni (tekst zostaje); 0 = nigdy.
    pub fn apply_retention(&self, days: u32, now: DateTime<Local>) -> usize {
        if days == 0 {
            return 0;
        }
        let limit = now - chrono::Duration::days(days as i64);
        let mut n = 0;
        for e in self.all().into_iter().filter(|e| !e.audio_deleted && e.created_at < limit) {
            let _ = std::fs::remove_file(self.wav(&e.id));
            if self.update(&e.id, |x| x.audio_deleted = true).is_ok() {
                n += 1;
            }
        }
        n
    }
}

fn changed(app: &AppHandle) {
    let _ = app.emit("dictations-changed", ());
}

/// Zapisuje dyktowanie w historii (wołane po wklejeniu — błąd nie psuje dyktowania).
pub fn record(app: &AppHandle, entry: &Entry, audio: &[f32]) {
    match History::default().save(entry, audio) {
        Ok(()) => changed(app),
        Err(e) => log::error!("zapis dyktowania w historii: {e}"),
    }
}

#[tauri::command]
pub fn list_dictations() -> Vec<Entry> {
    History::default().all()
}

#[tauri::command]
pub fn delete_dictation(app: AppHandle, id: String) -> Result<(), String> {
    History::default().delete(&id).map_err(|e| e.to_string())?;
    changed(&app);
    Ok(())
}

/// Okno zapisu i kopia nagrania dyktowania; zwraca ścieżkę, `None` = anulowano.
#[tauri::command]
pub async fn export_dictation_audio(app: AppHandle, id: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let h = History::default();
    let source = h.wav(&id);
    if !source.exists() {
        return Err("Nagranie tego dyktowania zostało już usunięte".into());
    }
    let picked = app
        .dialog()
        .file()
        .set_title("Zapisz nagranie dyktowania")
        .set_file_name(format!("dyktowanie-{id}.wav"))
        .add_filter("WAV", &["wav"])
        .blocking_save_file();
    let Some(file) = picked else { return Ok(None) };
    let target = file.into_path().map_err(|e| e.to_string())?;
    std::fs::copy(&source, &target).map_err(|e| e.to_string())?;
    Ok(Some(target.display().to_string()))
}

/// Przepisuje dyktowanie od nowa innym modelem / w innych językach (gdy wyszło słabo).
#[tauri::command]
pub async fn retranscribe_dictation(app: AppHandle, id: String, engine: EngineId, languages: Vec<String>) -> Result<Entry, String> {
    Engine::check_languages(engine, &languages).map_err(|e| e.to_string())?;
    if !engine.asset().is_installed() {
        return Err(format!("Brak modelu {} — pobierz go w Ustawieniach → Modele", engine.asset().title));
    }
    let vocabulary = app.state::<AppState>().settings.lock().unwrap().vocabulary.clone();
    let entry = tauri::async_runtime::spawn_blocking(move || retranscribe(&History::default(), &id, engine, languages, &vocabulary))
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    changed(&app);
    Ok(entry)
}

fn retranscribe(h: &History, id: &str, engine: EngineId, languages: Vec<String>, vocabulary: &str) -> Result<Entry> {
    let audio = h.audio(id)?;
    let mut e = Engine::load(engine)?;
    e.set_vocabulary(vocabulary);
    let codes: Vec<&str> = languages.iter().map(String::as_str).collect();
    let raw = e.transcribe_in(&audio, &codes)?;
    let text = crate::postprocess::apply(&raw);
    h.update(id, |x| {
        x.raw = raw;
        x.text = text;
        x.engine = engine.asset().title.to_string();
        x.languages = languages;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> History {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        History { root: std::env::temp_dir().join(format!("dx-dict-{}-{n}", std::process::id())) }
    }

    fn entry(at: DateTime<Local>, text: &str) -> Entry {
        Entry {
            id: new_id(at),
            created_at: at,
            duration_seconds: 1.0,
            text: text.into(),
            raw: text.to_lowercase(),
            engine: "Parakeet TDT 0.6B v3".into(),
            languages: vec!["pl".into()],
            pasted: true,
            audio_deleted: false,
        }
    }

    #[test]
    fn saves_lists_updates_and_deletes() {
        let h = history();
        let now = Local::now();
        let older = entry(now - chrono::Duration::minutes(5), "Pierwsze.");
        let newer = entry(now, "Drugie.");
        h.save(&older, &[0.5; 1600]).unwrap();
        h.save(&newer, &[0.25; 800]).unwrap();
        let all = h.all();
        assert_eq!(all.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["Drugie.", "Pierwsze."]);
        let audio = h.audio(&older.id).unwrap();
        assert_eq!(audio.len(), 1600);
        assert!((audio[0] - 0.5).abs() < 0.001);
        let e = h.update(&older.id, |x| x.text = "Poprawione.".into()).unwrap();
        assert_eq!(h.load(&older.id).unwrap(), e);
        h.delete(&older.id).unwrap();
        assert!(!h.wav(&older.id).exists());
        assert_eq!(h.all().len(), 1);
        std::fs::remove_dir_all(&h.root).ok();
    }

    /// Prawdziwy model: `cargo test --lib -- --ignored retranscribes_a_saved`.
    #[test]
    #[ignore]
    fn retranscribes_a_saved_dictation() {
        let h = history();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fleurs_kobieta.wav");
        let audio: Vec<f32> = hound::WavReader::open(path).unwrap().samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
        let e = entry(Local::now(), "źle przepisane");
        h.save(&e, &audio).unwrap();
        let got = retranscribe(&h, &e.id, EngineId::ParakeetV3, vec!["pl".into()], "").unwrap();
        println!("{} → {}", got.engine, got.text);
        assert!(got.text.to_lowercase().contains("wizy"), "{}", got.text);
        assert_eq!(h.load(&e.id).unwrap().text, got.text);
        std::fs::remove_dir_all(&h.root).ok();
    }

    #[test]
    fn retention_removes_old_audio_but_keeps_text() {
        let h = history();
        let now = Local::now();
        let old = entry(now - chrono::Duration::days(40), "Stare.");
        let fresh = entry(now - chrono::Duration::days(2), "Świeże.");
        h.save(&old, &[0.1; 160]).unwrap();
        h.save(&fresh, &[0.1; 160]).unwrap();
        assert_eq!(h.apply_retention(0, now), 0, "0 = nigdy");
        assert_eq!(h.apply_retention(30, now), 1);
        assert!(!h.wav(&old.id).exists() && h.wav(&fresh.id).exists());
        let kept = h.load(&old.id).unwrap();
        assert!(kept.audio_deleted && kept.text == "Stare.");
        assert!(h.audio(&old.id).is_err());
        assert_eq!(h.apply_retention(30, now), 0, "drugi raz nic");
        std::fs::remove_dir_all(&h.root).ok();
    }
}
