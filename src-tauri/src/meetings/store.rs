//! Foldery spotkań: `<dane>/Meetings/<yyyy-MM-dd_HH-mm-ss>/` z `meeting.json`, `audio/`,
//! `transcript.md|json`, `summaries/`. Ten sam układ co w wersji Swift.
use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::paths;

pub const MIC: &str = "mic";
pub const SYSTEM: &str = "system";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Recording,
    /// Wczytywanie pliku audio wskazanego przez użytkownika (import nagrania).
    Importing,
    /// Aplikacja zamknęła się w trakcie nagrywania — pliki zostały.
    Interrupted,
    Recorded,
    Transcribing,
    Transcribed,
    Summarizing,
    Summarized,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    pub id: String,
    pub started_at: DateTime<Local>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Local>>,
    #[serde(default)]
    pub duration_seconds: f64,
    pub state: State,
    #[serde(default)]
    pub has_system_audio: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub audio_deleted: bool,
    #[serde(default)]
    pub transcript_engine: Option<String>,
    /// Języki ostatniej transkrypcji (puste = rozpoznane przez silnik); `None` = sprzed tej opcji.
    #[serde(default)]
    pub transcript_languages: Option<Vec<String>>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Meeting {
    /// Czy da się przepisać (jest audio i nic akurat nie trwa).
    pub fn can_transcribe(&self) -> bool {
        !self.audio_deleted && !matches!(self.state, State::Recording | State::Importing | State::Transcribing | State::Summarizing)
    }

    /// Czy pliki audio są właśnie zapisywane (nagrywanie albo wczytywanie importu).
    pub fn audio_in_progress(&self) -> bool {
        matches!(self.state, State::Recording | State::Importing)
    }
}

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self { root: paths::meetings() }
    }
}

pub fn make_id(date: DateTime<Local>) -> String {
    date.format("%Y-%m-%d_%H-%M-%S").to_string()
}

impl Store {
    pub fn folder(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }
    pub fn audio_folder(&self, id: &str) -> PathBuf {
        self.folder(id).join("audio")
    }
    pub fn transcript_md(&self, id: &str) -> PathBuf {
        self.folder(id).join("transcript.md")
    }
    pub fn transcript_json(&self, id: &str) -> PathBuf {
        self.folder(id).join("transcript.json")
    }
    pub fn summaries_folder(&self, id: &str) -> PathBuf {
        self.folder(id).join("summaries")
    }
    fn metadata(&self, id: &str) -> PathBuf {
        self.folder(id).join("meeting.json")
    }

    pub fn create(&self, started_at: DateTime<Local>, has_system_audio: bool) -> Result<Meeting> {
        let base = make_id(started_at);
        let mut id = base.clone();
        let mut n = 2;
        while self.folder(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        std::fs::create_dir_all(self.audio_folder(&id)).context("tworzenie folderu spotkania")?;
        let m = Meeting {
            id,
            started_at,
            ended_at: None,
            duration_seconds: 0.0,
            state: State::Recording,
            has_system_audio,
            title: None,
            audio_deleted: false,
            transcript_engine: None,
            transcript_languages: None,
            last_error: None,
        };
        self.save(&m)?;
        Ok(m)
    }

    pub fn save(&self, m: &Meeting) -> Result<()> {
        let path = self.metadata(&m.id);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Option<Meeting> {
        std::fs::read(self.metadata(id)).ok().and_then(|b| serde_json::from_slice(&b).ok())
    }

    pub fn update(&self, id: &str, f: impl FnOnce(&mut Meeting)) -> Result<Meeting> {
        let mut m = self.load(id).with_context(|| format!("brak spotkania {id}"))?;
        f(&mut m);
        self.save(&m)?;
        Ok(m)
    }

    /// Wszystkie spotkania, najnowsze pierwsze.
    pub fn all(&self) -> Vec<Meeting> {
        let mut v: Vec<Meeting> = std::fs::read_dir(&self.root)
            .map(|it| it.filter_map(|e| e.ok()).filter_map(|e| self.load(e.file_name().to_str()?)).collect())
            .unwrap_or_default();
        v.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        v
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let dir = self.folder(id);
        if dir.starts_with(&self.root) && dir != self.root {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    /// Po starcie aplikacji: spotkania zostawione w `recording` (awaria, wyłączenie prądu)
    /// → naprawa nagłówków WAV i stan `interrupted`; przerwane przetwarzanie → poprzedni stan.
    pub fn recover(&self) -> Vec<String> {
        let mut recovered = Vec::new();
        for m in self.all() {
            let new_state = match m.state {
                State::Recording | State::Importing => Some(State::Interrupted),
                State::Transcribing => Some(State::Recorded),
                State::Summarizing => Some(State::Transcribed),
                _ => None,
            };
            let Some(state) = new_state else { continue };
            if matches!(m.state, State::Recording | State::Importing) {
                for prefix in [MIC, SYSTEM] {
                    for p in super::writer::segments(&self.audio_folder(&m.id), prefix) {
                        if let Err(e) = super::writer::repair(&p) {
                            log::warn!("naprawa {}: {e}", p.display());
                        }
                    }
                }
            }
            let audio = self.audio_folder(&m.id);
            let samples = super::writer::track_duration_samples(&audio, MIC).max(super::writer::track_duration_samples(&audio, SYSTEM));
            let duration = samples as f64 / 16_000.0;
            let _ = self.update(&m.id, |x| {
                x.state = state;
                if x.duration_seconds == 0.0 {
                    x.duration_seconds = duration;
                }
            });
            recovered.push(m.id);
        }
        recovered
    }

    /// Usuwa audio spotkań starszych niż `days` dni — tylko tych już przepisanych (tekst zostaje).
    pub fn apply_retention(&self, days: u32, now: DateTime<Local>) -> usize {
        if days == 0 {
            return 0;
        }
        let limit = now - chrono::Duration::days(days as i64);
        let mut n = 0;
        for m in self.all() {
            let transcribed = matches!(m.state, State::Transcribed | State::Summarized | State::Summarizing);
            if m.audio_deleted || !transcribed || m.started_at > limit {
                continue;
            }
            if std::fs::remove_dir_all(self.audio_folder(&m.id)).is_ok() {
                let _ = self.update(&m.id, |x| x.audio_deleted = true);
                n += 1;
            }
        }
        n
    }

    pub fn summaries(&self, id: &str) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(self.summaries_folder(id))
            .map(|it| it.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "md")).collect())
            .unwrap_or_default();
        v.sort();
        v.reverse(); // najnowsze pierwsze (nazwy zaczynają się od daty)
        v
    }
}

pub fn read_to_string(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn store() -> Store {
        let root = std::env::temp_dir().join(format!(
            "dx-store-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        Store { root }
    }

    #[test]
    fn create_unique_ids_and_list() {
        let s = store();
        let t = Local.with_ymd_and_hms(2026, 10, 5, 14, 3, 7).unwrap();
        let a = s.create(t, true).unwrap();
        let b = s.create(t, false).unwrap();
        assert_eq!(a.id, "2026-10-05_14-03-07");
        assert_eq!(b.id, "2026-10-05_14-03-07-2");
        assert_eq!(s.all().len(), 2);
        std::fs::remove_dir_all(&s.root).ok();
    }

    #[test]
    fn recover_marks_interrupted() {
        let s = store();
        let m = s.create(Local::now(), true).unwrap();
        let t = s.create(Local::now() - chrono::Duration::hours(1), true).unwrap();
        s.update(&t.id, |x| x.state = State::Transcribing).unwrap();
        let ids = s.recover();
        assert_eq!(ids.len(), 2);
        assert_eq!(s.load(&m.id).unwrap().state, State::Interrupted);
        assert_eq!(s.load(&t.id).unwrap().state, State::Recorded);
        std::fs::remove_dir_all(&s.root).ok();
    }

    #[test]
    fn retention_deletes_only_old_transcribed_audio() {
        let s = store();
        let now = Local::now();
        let old_done = s.create(now - chrono::Duration::days(40), true).unwrap();
        let old_raw = s.create(now - chrono::Duration::days(41), true).unwrap();
        let fresh = s.create(now - chrono::Duration::days(2), true).unwrap();
        for m in [&old_done, &fresh] {
            s.update(&m.id, |x| x.state = State::Transcribed).unwrap();
        }
        s.update(&old_raw.id, |x| x.state = State::Recorded).unwrap();
        assert_eq!(s.apply_retention(30, now), 1);
        assert!(!s.audio_folder(&old_done.id).exists());
        assert!(s.load(&old_done.id).unwrap().audio_deleted);
        assert!(s.audio_folder(&old_raw.id).exists());
        assert!(s.audio_folder(&fresh.id).exists());
        std::fs::remove_dir_all(&s.root).ok();
    }

    #[test]
    fn reads_swift_meeting_json() {
        let j = r#"{"durationSeconds":61.5,"hasSystemAudio":true,"id":"2026-09-30_10-00-00","startedAt":"2026-09-30T10:00:00+02:00","state":"summarized","audioDeleted":false}"#;
        let m: Meeting = serde_json::from_str(j).unwrap();
        assert_eq!(m.state, State::Summarized);
        assert!(m.has_system_audio);
    }
}
