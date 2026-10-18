//! Learning the vocabulary from the user's corrections (the "auto-learn" of Wispr Flow's
//! dictionary): the next dictation reads the field again, finds the previously pasted text in it,
//! and where the user replaced misrecognized words with a name or a term, adds it to the
//! vocabulary. Only name-like corrections are learned (a capital letter, a digit, a foreign
//! letter, letters with digits, an inner symbol); "zrobię → zrobimy" or "2024 → 2025" is an
//! edit, not a misrecognized word.
use crate::vocabulary::{fold, levenshtein, Vocabulary};

/// More from one dictation would mostly be a rewritten sentence.
const MAX_LEARNED: usize = 4;
/// The longest replaced run of words (a name split into pieces: "en paw" → "NPaw").
const MAX_RUN: usize = 3;
/// Only the end of a long field is compared (the pasted text is just before the cursor).
const MAX_FIELD_WORDS: usize = 600;
const MIN_CHARS: usize = 4;
const STOPWORDS: &[&str] = &[
    "jest", "będzie", "tylko", "jeszcze", "bardzo", "więc", "kiedy", "który", "która", "które", "przez", "oraz", "albo",
    "jeśli", "gdzie", "wszystko", "sobie", "siebie", "swoje", "tego", "jego", "może", "można", "była", "było", "były",
    "this", "that", "with", "they", "there", "have", "were", "been", "what", "when", "then", "than", "about", "your",
];

/// A correction the user made: the pasted words and what they became.
#[derive(Debug, Clone, PartialEq)]
pub struct Correction {
    pub observed: String,
    pub edited: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Same,
    Changed,
    /// A pasted word that's no longer there.
    Removed,
    /// A word the user added.
    Added,
}

fn core(word: &str) -> &str {
    word.trim_matches(|c: char| !c.is_alphanumeric())
}

/// The pasted words aligned with the best-matching place in the field (free start and end in
/// the field): ops with (pasted index, field index) — None for no word on that side.
fn align(pasted: &[&str], field: &[&str]) -> (usize, Vec<(Op, Option<usize>, Option<usize>)>) {
    let (m, n) = (pasted.len(), field.len());
    let mut d = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for i in 1..=m {
        for j in 1..=n {
            let same = core(pasted[i - 1]) == core(field[j - 1]);
            d[i][j] = (d[i - 1][j - 1] + usize::from(!same)).min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
        }
    }
    // On a tie the later end: the last pasted word edited, rather than deleted.
    let (cost, mut j) = (0..=n).map(|j| (d[m][j], std::cmp::Reverse(j))).min().map_or((m, 0), |(c, j)| (c, j.0));
    let mut ops = Vec::new();
    let mut i = m;
    while i > 0 {
        let same = j > 0 && core(pasted[i - 1]) == core(field[j - 1]);
        if j > 0 && d[i][j] == d[i - 1][j - 1] + usize::from(!same) {
            ops.push((if same { Op::Same } else { Op::Changed }, Some(i - 1), Some(j - 1)));
            i -= 1;
            j -= 1;
        } else if d[i][j] == d[i - 1][j] + 1 {
            ops.push((Op::Removed, Some(i - 1), None));
            i -= 1;
        } else {
            ops.push((Op::Added, None, Some(j - 1)));
            j -= 1;
        }
    }
    ops.reverse();
    (cost, ops)
}

/// Like a name or a term rather than an ordinary word. A capital letter only counts when the case
/// change isn't just the start of a sentence.
fn name_like(edited: &str, observed: &str, sentence_start: bool) -> bool {
    let inner_symbol = edited.split_whitespace().any(|w| core(w).contains(['/', '-', '.', '_', '&', '+']));
    // Digits only next to letters ("H264"): a changed number or time is an edit of the content.
    let foreign = edited.chars().any(|c| "qvxQVX".contains(c))
        || edited.split_whitespace().any(|w| core(w).chars().any(|c| c.is_ascii_digit()) && core(w).chars().any(char::is_alphabetic));
    let capital = edited.split_whitespace().enumerate().any(|(i, w)| {
        let mut chars = core(w).chars();
        let first_only = chars.next().is_some_and(char::is_uppercase) && !chars.any(char::is_uppercase);
        let upper = core(w).chars().any(char::is_uppercase);
        upper && !(i == 0 && first_only && sentence_start && edited.to_lowercase() == observed.to_lowercase())
    });
    inner_symbol || foreign || capital
}

/// The corrections of `pasted` found in `field` (the field's text around the cursor); empty when
/// the pasted text isn't there anymore (another field, rewritten).
pub fn corrections(pasted: &str, field: &str) -> Vec<Correction> {
    let pasted: Vec<&str> = pasted.split_whitespace().collect();
    let all: Vec<&str> = field.split_whitespace().collect();
    let field = &all[all.len().saturating_sub(MAX_FIELD_WORDS)..];
    if pasted.len() < 2 {
        return Vec::new();
    }
    let (cost, ops) = align(&pasted, field);
    let same = ops.iter().filter(|o| o.0 == Op::Same).count();
    // Most of the text must still be there, unchanged.
    if cost == 0 || same < 2 || same * 3 < pasted.len() * 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut k = 0;
    while k < ops.len() {
        if ops[k].0 == Op::Same {
            k += 1;
            continue;
        }
        let start = k;
        while k < ops.len() && ops[k].0 != Op::Same {
            k += 1;
        }
        let run = &ops[start..k];
        let observed: Vec<&str> = run.iter().filter_map(|o| o.1).map(|i| pasted[i]).collect();
        let edited: Vec<&str> = run.iter().filter_map(|o| o.2).map(|j| field[j]).collect();
        if observed.is_empty() || edited.is_empty() || observed.len() > MAX_RUN || edited.len() > MAX_RUN {
            continue;
        }
        let first_field = run.iter().find_map(|o| o.2).unwrap_or(0);
        let sentence_start = first_field == 0 || field[first_field - 1].ends_with(['.', '!', '?']);
        let observed = observed.iter().map(|w| core(w)).collect::<Vec<_>>().join(" ");
        let edited = edited.iter().map(|w| core(w)).collect::<Vec<_>>().join(" ");
        let (fo, fe) = (fold(&observed), fold(&edited));
        let similar = levenshtein(&fo, &fe) <= (fe.len() / 2).max(2);
        let long_enough = edited.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_CHARS;
        let stopword = edited.split_whitespace().all(|w| STOPWORDS.contains(&w.to_lowercase().as_str()));
        if edited != observed && similar && long_enough && !stopword && name_like(&edited, &observed, sentence_start) {
            out.push(Correction { observed, edited });
        }
        if out.len() == MAX_LEARNED {
            break;
        }
    }
    out
}

/// Vocabulary entries for the corrections the vocabulary doesn't handle yet: the term alone
/// when it already fixes the observed spelling or when one word was replaced (a replacement rule
/// for a real word — "marką → Markiem" — would rewrite that word everywhere), otherwise an
/// explicit replacement of the words a name was split into ("en paw -> NPaw").
pub fn entries(corrections: &[Correction], vocabulary: &str) -> Vec<String> {
    let current = Vocabulary::parse(vocabulary);
    let mut out: Vec<String> = Vec::new();
    for c in corrections {
        if current.correct(&c.observed) == c.edited {
            continue;
        }
        let entry = if Vocabulary::parse(&c.edited).correct(&c.observed) == c.edited || !c.observed.contains(' ') {
            c.edited.clone()
        } else {
            format!("{} -> {}", c.observed.to_lowercase(), c.edited)
        };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

/// The vocabulary with the entries appended.
pub fn append(vocabulary: &str, entries: &[String]) -> String {
    let v = vocabulary.trim_end();
    let sep = if v.is_empty() {
        ""
    } else if v.contains('\n') && !v.contains(',') {
        "\n"
    } else {
        ", "
    };
    format!("{v}{sep}{}", entries.join(if sep == "\n" { "\n" } else { ", " }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(observed: &str, edited: &str) -> Correction {
        Correction { observed: observed.into(), edited: edited.into() }
    }

    #[test]
    fn finds_corrected_names_in_the_field() {
        let pasted = "Wrzuciłem poprawkę do repo en paw i dałem znać tomkowi.";
        let field = "Cześć!\nWrzuciłem poprawkę do repo NPaw i dałem znać Tomkowi. Pozdrawiam";
        assert_eq!(corrections(pasted, field), vec![c("en paw", "NPaw"), c("tomkowi", "Tomkowi")]);
        assert_eq!(corrections("Sprawdź czy kubernetis działa", "Sprawdź czy Kubernetes działa"), vec![c("kubernetis", "Kubernetes")]);
    }

    #[test]
    fn ignores_ordinary_edits_and_other_text() {
        // An edit of an ordinary word, not a name.
        assert!(corrections("Jutro to zrobię na pewno", "Jutro to zrobimy na pewno").is_empty());
        // A capital at the start of a sentence.
        assert!(corrections("jutro będzie spotkanie z zespołem", "Wczoraj. Jutro będzie spotkanie z zespołem").is_empty());
        // Rewritten or another field.
        assert!(corrections("Spotkanie przeniesione na piątek rano", "Zupełnie inna wiadomość w tym polu").is_empty());
        assert!(corrections("Spotkanie jutro", "Spotkanie jutro").is_empty());
        // A different word, not a fixed spelling.
        assert!(corrections("Zadzwoń do Marka jutro rano", "Zadzwoń do Zbigniewa jutro rano").is_empty());
        // Changed numbers and times are edits of the content; letters with digits are terms.
        assert!(corrections("Raport za 2024 jest gotowy", "Raport za 2025 jest gotowy").is_empty());
        assert!(corrections("Spotkanie o 15:30 w sali", "Spotkanie o 16:30 w sali").is_empty());
        assert_eq!(corrections("Kodek h265 działa dobrze", "Kodek h264 działa dobrze"), vec![c("h265", "h264")]);
    }

    #[test]
    fn entries_prefer_a_term_and_skip_known_ones() {
        let found = [c("hisens", "Hisense"), c("cloud kod", "Claude Code"), c("tizen", "Tizen")];
        assert_eq!(entries(&found, "Tizen"), vec!["Hisense".to_string(), "cloud kod -> Claude Code".to_string()]);
        assert!(entries(&found, "Hisense, cloud kod -> Claude Code, Tizen").is_empty());
        // One real word is never turned into a rule that rewrites it everywhere: the term only.
        assert_eq!(entries(&[c("marką", "Markiem")], ""), vec!["Markiem".to_string()]);
    }

    #[test]
    fn appends_in_the_vocabulary_style() {
        let new = ["NPaw".to_string(), "Tizen".to_string()];
        assert_eq!(append("", &new), "NPaw, Tizen");
        assert_eq!(append("Hisense, Borys\n", &new), "Hisense, Borys, NPaw, Tizen");
        assert_eq!(append("Hisense\nBorys", &new), "Hisense\nBorys\nNPaw\nTizen");
    }
}
