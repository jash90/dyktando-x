//! Składanie transkryptu z dwóch ścieżek (port `TranscriptBuilder` ze Swifta) — czysta logika,
//! bez audio i modeli: etykiety mówców, usuwanie echa, scalanie wypowiedzi, Markdown.
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const ME: &str = "Ja";
pub const OTHERS: &str = "Rozmówcy";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    Mic,
    System,
}

/// Jedna wypowiedź z jednej ścieżki (fragment VAD przepisany przez silnik).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Utterance {
    pub start: f64,
    pub end: f64,
    pub track: Track,
    pub text: String,
    #[serde(default)]
    pub speaker: String,
    /// Tłumaczenie wypowiedzi (transkrypcja na żywo z tłumaczeniem przez Canary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
}

/// Fragment przypisany jednemu mówcy przez rozpoznawanie mówców (ścieżka „system”).
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerSegment {
    pub speaker_id: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptDocument {
    #[serde(rename = "meetingID")]
    pub meeting_id: String,
    pub engine: String,
    pub created_at: DateTime<Local>,
    pub duration_seconds: f64,
    pub utterances: Vec<Utterance>,
}

impl TranscriptDocument {
    pub fn speakers(&self) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for u in &self.utterances {
            if !seen.contains(&u.speaker) {
                seen.push(u.speaker.clone());
            }
        }
        seen
    }
}

fn by_start(a: &Utterance, b: &Utterance) -> std::cmp::Ordering {
    a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal)
}

/// Etykiety mówców dla ścieżki „system”: największe nakładanie z segmentem mówcy;
/// numeracja „Rozmówca 1, 2…” w kolejności pierwszego pojawienia się. Bez danych — „Rozmówcy”.
pub fn label_system(utterances: &[Utterance], speakers: Option<&[SpeakerSegment]>) -> Vec<Utterance> {
    let Some(speakers) = speakers.filter(|s| !s.is_empty()) else {
        return utterances.iter().map(|u| Utterance { speaker: OTHERS.into(), ..u.clone() }).collect();
    };
    let mut sorted = utterances.to_vec();
    sorted.sort_by(by_start);
    let mut names: HashMap<String, String> = HashMap::new();
    sorted
        .into_iter()
        .map(|mut u| {
            let mut totals: Vec<(String, f64)> = Vec::new();
            for s in speakers {
                let overlap = u.end.min(s.end) - u.start.max(s.start);
                if overlap > 0.0 {
                    match totals.iter_mut().find(|(id, _)| *id == s.speaker_id) {
                        Some((_, t)) => *t += overlap,
                        None => totals.push((s.speaker_id.clone(), overlap)),
                    }
                }
            }
            let best = totals.into_iter().fold(None::<(String, f64)>, |best, (id, o)| match best {
                Some((_, bo)) if bo >= o => best,
                _ => Some((id, o)),
            });
            u.speaker = match best {
                Some((id, _)) => {
                    let n = names.len() + 1;
                    names.entry(id).or_insert_with(|| format!("Rozmówca {n}")).clone()
                }
                None => OTHERS.into(),
            };
            u
        })
        .collect()
}

/// Podobieństwo tekstów: część wspólna słów (≥ 3 litery) względem krótszej wypowiedzi.
pub fn word_similarity(a: &str, b: &str) -> f64 {
    fn words(s: &str) -> HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphabetic())
            .filter(|w| w.chars().count() >= 3)
            .map(str::to_string)
            .collect()
    }
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    wa.intersection(&wb).count() as f64 / wa.len().min(wb.len()) as f64
}

/// Echo: bez słuchawek głos rozmówców z głośników trafia też do mikrofonu. Wypowiedź z mikrofonu,
/// która nakłada się w czasie z wypowiedzią systemową i ma podobny tekst, odrzucamy.
pub fn remove_echo(mic: &[Utterance], system: &[Utterance], threshold: f64) -> Vec<Utterance> {
    mic.iter()
        .filter(|m| {
            !system.iter().any(|s| {
                let overlap = m.end.min(s.end) - m.start.max(s.start);
                let shorter = (m.end - m.start).min(s.end - s.start).max(0.01);
                overlap / shorter > 0.5 && word_similarity(&m.text, &s.text) >= threshold
            })
        })
        .cloned()
        .collect()
}

/// Kolejne wypowiedzi tej samej osoby z przerwą < `gap` łączymy w jedną.
pub fn merge_consecutive(utterances: &[Utterance], gap: f64) -> Vec<Utterance> {
    let mut sorted = utterances.to_vec();
    sorted.sort_by(by_start);
    let mut out: Vec<Utterance> = Vec::new();
    for u in sorted {
        if let Some(last) = out.last_mut() {
            if last.speaker == u.speaker && u.start - last.end < gap {
                last.end = last.end.max(u.end);
                last.text.push(' ');
                last.text.push_str(&u.text);
                if let Some(t) = u.translation {
                    let lt = last.translation.get_or_insert_with(String::new);
                    if !lt.is_empty() {
                        lt.push(' ');
                    }
                    lt.push_str(&t);
                }
                continue;
            }
        }
        out.push(u);
    }
    out
}

pub fn build(mic: &[Utterance], system: &[Utterance], speakers: Option<&[SpeakerSegment]>) -> Vec<Utterance> {
    let labeled_system = label_system(system, speakers);
    let labeled_mic: Vec<Utterance> =
        remove_echo(mic, system, 0.6).into_iter().map(|u| Utterance { speaker: ME.into(), ..u }).collect();
    let mut all = labeled_mic;
    all.extend(labeled_system);
    merge_consecutive(&all, 2.0)
}

pub fn timestamp(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

pub fn clock(seconds: f64) -> String {
    let s = seconds.round().max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

const MONTHS: [&str; 12] = [
    "stycznia", "lutego", "marca", "kwietnia", "maja", "czerwca", "lipca", "sierpnia", "września", "października", "listopada", "grudnia",
];

pub fn polish_date(d: DateTime<Local>) -> String {
    use chrono::{Datelike, Timelike};
    format!("{} {} {}, {:02}:{:02}", d.day(), MONTHS[d.month0() as usize], d.year(), d.hour(), d.minute())
}

pub fn markdown(doc: &TranscriptDocument, started_at: DateTime<Local>) -> String {
    let mut lines = vec![
        format!("# Spotkanie — {}", polish_date(started_at)),
        String::new(),
        format!("Długość: {} · Model: {} · Mówcy: {}", clock(doc.duration_seconds), doc.engine, doc.speakers().join(", ")),
        String::new(),
    ];
    for u in &doc.utterances {
        lines.push(format!("[{}] **{}:** {}", timestamp(u.start), u.speaker, u.text));
        if let Some(t) = u.translation.as_deref().filter(|t| !t.is_empty()) {
            lines.push(format!("> {t}"));
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn u(start: f64, end: f64, track: Track, text: &str) -> Utterance {
        Utterance { start, end, track, text: text.into(), speaker: String::new(), translation: None }
    }
    fn seg(id: &str, start: f64, end: f64) -> SpeakerSegment {
        SpeakerSegment { speaker_id: id.into(), start, end }
    }

    #[test]
    fn speaker_numbers_in_order_of_appearance() {
        let system = [u(10., 14., Track::System, "druga"), u(0., 4., Track::System, "pierwsza"), u(20., 24., Track::System, "trzecia")];
        let speakers = [seg("S7", 0., 5.), seg("S2", 9., 15.), seg("S7", 19., 25.)];
        let l = label_system(&system, Some(&speakers));
        assert_eq!(l.iter().map(|x| x.speaker.as_str()).collect::<Vec<_>>(), ["Rozmówca 1", "Rozmówca 2", "Rozmówca 1"]);
        assert_eq!(l.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["pierwsza", "druga", "trzecia"]);
    }

    #[test]
    fn without_diarization_everyone_is_rozmowcy() {
        let l = label_system(&[u(0., 2., Track::System, "x")], None);
        assert_eq!(l[0].speaker, "Rozmówcy");
    }

    #[test]
    fn largest_overlap_wins() {
        let l = label_system(&[u(0., 4., Track::System, "x")], Some(&[seg("A", 0., 1.), seg("B", 1., 4.)]));
        assert_eq!(l[0].speaker, "Rozmówca 1");
    }

    #[test]
    fn echo_from_speakers_into_mic_is_removed() {
        let system = [u(5., 9., Track::System, "Spotkanie przesuwamy na czwartek po południu")];
        let mic = [u(5.1, 9.2, Track::Mic, "spotkanie przesuwamy na czwartek południu"), u(12., 14., Track::Mic, "Dobrze, pasuje mi czwartek")];
        let kept = remove_echo(&mic, &system, 0.6);
        assert_eq!(kept.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["Dobrze, pasuje mi czwartek"]);
    }

    #[test]
    fn overlapping_but_different_text_is_not_echo() {
        let system = [u(0., 4., Track::System, "Jaki jest termin wdrożenia projektu")];
        let mic = [u(1., 3., Track::Mic, "Zaraz sprawdzę harmonogram")];
        assert_eq!(remove_echo(&mic, &system, 0.6).len(), 1);
    }

    #[test]
    fn consecutive_same_speaker_is_merged() {
        let mut a = u(0., 3., Track::Mic, "Pierwsze zdanie.");
        a.speaker = "Ja".into();
        let mut b = u(4., 6., Track::Mic, "Drugie zdanie.");
        b.speaker = "Ja".into();
        let mut c = u(6.5, 8., Track::System, "Odpowiedź.");
        c.speaker = "Rozmówca 1".into();
        let mut d = u(20., 22., Track::Mic, "Po przerwie.");
        d.speaker = "Ja".into();
        let m = merge_consecutive(&[d, c, b, a], 2.0);
        assert_eq!(m.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["Pierwsze zdanie. Drugie zdanie.", "Odpowiedź.", "Po przerwie."]);
        assert_eq!(m[0].end, 6.0);
    }

    #[test]
    fn markdown_format() {
        let mut a = u(3725., 3730., Track::Mic, "Podsumujmy.");
        a.speaker = "Ja".into();
        let doc = TranscriptDocument {
            meeting_id: "x".into(),
            engine: "Parakeet TDT v3".into(),
            created_at: Local::now(),
            duration_seconds: 3800.,
            utterances: vec![a],
        };
        let md = markdown(&doc, Local.with_ymd_and_hms(2026, 10, 5, 9, 7, 0).unwrap());
        assert!(md.contains("[01:02:05] **Ja:** Podsumujmy."), "{md}");
        assert!(md.contains("Model: Parakeet TDT v3"));
        assert!(md.contains("Mówcy: Ja"));
        assert!(md.contains("Długość: 1:03:20"));
        assert!(md.starts_with("# Spotkanie — 5 października 2026, 09:07"));
    }

    #[test]
    fn build_puts_me_and_others_together() {
        let mic = [u(0., 2., Track::Mic, "Cześć wszystkim")];
        let system = [u(3., 5., Track::System, "Dzień dobry")];
        let b = build(&mic, &system, None);
        assert_eq!(b.iter().map(|x| x.speaker.as_str()).collect::<Vec<_>>(), ["Ja", "Rozmówcy"]);
    }
}
