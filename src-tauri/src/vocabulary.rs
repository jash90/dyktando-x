//! Vocabulary of names and terms, applied to the output of every engine (the approach of Wispr Flow's
//! dictionary: the list biases recognition where the model allows it, and the spelling is fixed
//! afterwards). Whisper gets the terms as its initial prompt; Parakeet and Canary can't be biased,
//! so their text is corrected after recognition:
//! - `wrong spelling -> Right spelling` is an explicit replacement (whole words, case-insensitive),
//! - a plain term fixes the spelling and case of near matches ("hisense", "hi sense", "tyzen" →
//!   "Hisense", "Tizen") and keeps Polish endings ("klaudiuszem" → "Klaudiuszem").

/// Shorter terms are only replaced explicitly: "ci" is a Polish word, not "CI".
const MIN_TERM_CHARS: usize = 4;
/// The most words one match can span (an engine splits a name: "Hi sense").
const MAX_WINDOW: usize = 4;
/// Polish case endings that may follow the term ("Tomasz-a", "Hisens-ie", "Klaudiusz-em",
/// "Claude-'a"). Only these: "kubek" is not a form of "Kuba".
const ENDINGS: [&str; 18] = ["", "a", "e", "i", "o", "u", "y", "ą", "ę", "em", "om", "ie", "owi", "ach", "ami", "ów", "ego", "emu"];
const ARROWS: [&str; 3] = ["->", "=>", "→"];
/// Short Polish words that are never a piece of a name split by the engine ("to masz" is not
/// "Tomasz", "do roty" is not "Dorota"); one letter never is either ("o lek" is not "Olek").
const FUNCTION_WORDS: &[&str] = &[
    "to", "do", "na", "po", "za", "od", "ze", "we", "co", "ja", "no", "ty", "on", "my", "wy", "mi", "ci", "go", "je", "ma",
    "by", "bo", "że", "aż", "ku", "ta", "te", "tu", "nie", "tak", "jak", "się", "ale", "czy", "już", "też", "tam", "ten",
    "tym", "pod", "nad", "dla", "bez", "ani", "lub", "oto",
];

#[derive(Default, Clone)]
pub struct Vocabulary {
    terms: Vec<Term>,
    replacements: Vec<Replacement>,
}

#[derive(Clone)]
struct Term {
    text: String,
    /// Lowercase, char for char with `text`.
    lower: Vec<char>,
    /// For fuzzy comparison: see `fold`.
    key: Vec<char>,
    words: usize,
}

#[derive(Clone)]
struct Replacement {
    /// Lowercase words to replace.
    from: Vec<String>,
    to: String,
}

impl Vocabulary {
    /// Entries separated by commas, semicolons or new lines.
    pub fn parse(text: &str) -> Self {
        let mut v = Self::default();
        for item in text.split([',', ';', '\n']) {
            let item = item.split_whitespace().collect::<Vec<_>>().join(" ");
            if let Some((from, to)) = ARROWS.iter().find_map(|a| item.split_once(a)) {
                let from: Vec<String> = from.split_whitespace().map(|w| lower(core(w))).filter(|w| !w.is_empty()).collect();
                let to = to.trim();
                if !from.is_empty() && !to.is_empty() {
                    v.replacements.push(Replacement { from, to: to.to_string() });
                    v.add_term(to);
                }
            } else {
                v.add_term(&item);
            }
        }
        // The longest replacement wins regardless of the order of the entries: "cloud kod" before
        // "cloud".
        v.replacements.sort_by_key(|r| std::cmp::Reverse(r.from.len()));
        v
    }

    fn add_term(&mut self, text: &str) {
        if text.is_empty() || self.terms.iter().any(|t| t.text == text) {
            return;
        }
        self.terms.push(Term { text: text.to_string(), lower: lower_chars(text), key: fold(text), words: text.split_whitespace().count() });
    }

    /// Initial prompt for Whisper: the terms (without the replacement syntax); empty = none.
    pub fn prompt(&self) -> String {
        self.terms.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(", ")
    }

    pub fn has_term(&self, word: &str) -> bool {
        self.terms.iter().any(|t| t.text == word || t.text.split_whitespace().next() == Some(word))
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The text with replacements applied and terms spelled as in the vocabulary.
    pub fn correct(&self, text: &str) -> String {
        if self.is_empty() {
            return text.to_string();
        }
        let tokens = tokens(text);
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0;
        let mut i = 0;
        while i < tokens.len() {
            match self.best_match(text, &tokens, i) {
                Some((n, replacement)) => {
                    out.push_str(&text[cursor..tokens[i].core.0]);
                    out.push_str(&replacement);
                    cursor = tokens[i + n - 1].core.1;
                    i += n;
                }
                None => i += 1,
            }
        }
        out.push_str(&text[cursor..]);
        out
    }

    /// The longest, closest match starting at token `i`: (token count, replacement).
    fn best_match(&self, text: &str, tokens: &[Token], i: usize) -> Option<(usize, String)> {
        for r in &self.replacements {
            let n = r.from.len();
            if window(text, tokens, i, n).is_some_and(|words| words.iter().map(|w| lower(w)).eq(r.from.iter().cloned())) {
                return Some((n, r.to.clone()));
            }
        }
        // (edits, tokens) — fewer edits first, then fewer words: "apple tv w" folds to "Apple TV"
        // too ("tww" → "tw"), but the "w" isn't part of the name.
        let mut best: Option<((usize, usize), usize, String)> = None;
        for term in self.terms.iter().filter(|t| t.letters() >= MIN_TERM_CHARS) {
            let sizes = term.words.saturating_sub(1).max(1)..=(term.words + 1).min(MAX_WINDOW);
            for n in sizes {
                let Some(words) = window(text, tokens, i, n) else { break };
                // A name split into pieces ("hi sense") has no Polish function word among them.
                if n > term.words && words.iter().any(|w| w.chars().count() < 2 || FUNCTION_WORDS.contains(&lower(w).as_str())) {
                    continue;
                }
                let Some((edits, replacement)) = term.matches(&words) else { continue };
                let rank = (edits, n);
                if best.as_ref().is_none_or(|(b, _, _)| rank < *b) {
                    best = Some((rank, n, replacement));
                }
            }
        }
        best.map(|(_, n, replacement)| (n, replacement))
    }
}

impl Term {
    /// Letters and digits of the term (not of `key`, which collapses "Anna" to "ana").
    fn letters(&self) -> usize {
        self.lower.iter().filter(|c| c.is_alphanumeric()).count()
    }

    /// The number of edits and the spelling to use, when `words` is this term.
    fn matches(&self, words: &[&str]) -> Option<(usize, String)> {
        if let [word] = words {
            if self.words == 1 {
                if let Some(inflected) = self.inflect(word) {
                    return Some((0, inflected));
                }
            }
        }
        // A key folded below the minimum ("Otto" → "oto") only matches through `inflect`: "oto" is a
        // Polish word.
        let key = fold(&words.join(" "));
        if self.key.len() < MIN_TERM_CHARS || key.first() != self.key.first() {
            return None;
        }
        let edits = levenshtein(&key, &self.key);
        (edits <= allowed_edits(self.key.len())).then(|| (edits, self.text.clone()))
    }

    /// The term with the word's Polish ending: everything but the last letter of the term must
    /// agree, and the rest must be a case ending ("kasią" → "Kasią", "tomasza" → "Tomasza");
    /// None when it's another word. A word shorter than the term isn't a form of it ("ani" is not
    /// "Ania").
    fn inflect(&self, word: &str) -> Option<String> {
        let chars: Vec<char> = word.chars().collect();
        let lower = lower_chars(word);
        let common = lower.iter().zip(&self.lower).take_while(|(a, b)| a == b).count();
        let ending: String = lower[common..].iter().collect();
        if lower.len() < self.lower.len() || common + 1 < self.lower.len() || !ENDINGS.contains(&ending.trim_start_matches(['\'', '’'])) {
            return None;
        }
        Some(self.text.chars().take(common).chain(chars[common..].iter().copied()).collect())
    }
}

/// How many edits a near match of a term of `len` folded characters may have.
fn allowed_edits(len: usize) -> usize {
    match len {
        0..=5 => 0,
        6..=8 => 1,
        9..=12 => 2,
        _ => 3,
    }
}

/// A whitespace-separated word, with the byte range of its core (without surrounding punctuation).
struct Token {
    span: (usize, usize),
    core: (usize, usize),
}

fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                let word = &text[s..i];
                let trimmed = word.trim_start_matches(|c: char| !c.is_alphanumeric());
                let core_start = s + word.len() - trimmed.len();
                let core_end = core_start + trimmed.trim_end_matches(|c: char| !c.is_alphanumeric()).len();
                out.push(Token { span: (s, i), core: (core_start, core_end) });
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// The cores of `n` words from `i`, when nothing but spaces separates them ("apple, tv" is not
/// "Apple TV").
fn window<'a>(text: &'a str, tokens: &[Token], i: usize, n: usize) -> Option<Vec<&'a str>> {
    let ts = tokens.get(i..i + n)?;
    let joined = ts.windows(2).all(|p| p[0].core.1 == p[0].span.1 && p[1].core.0 == p[1].span.0);
    let words: Vec<&str> = ts.iter().map(|t| &text[t.core.0..t.core.1]).collect();
    (joined && words.iter().all(|w| !w.is_empty())).then_some(words)
}

fn core(word: &str) -> &str {
    word.trim_matches(|c: char| !c.is_alphanumeric())
}

fn lower_chars(s: &str) -> Vec<char> {
    s.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

fn lower(s: &str) -> String {
    lower_chars(s).into_iter().collect()
}

/// Letters only, lowercase, without diacritics, with spellings of the same sound unified
/// ("Tyzeń" ~ "tizen", "Philips" ~ "filips"), double letters collapsed.
pub(crate) fn fold(s: &str) -> Vec<char> {
    let mut plain = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        plain.push(match c {
            'ą' | 'á' | 'à' | 'â' | 'ä' => 'a',
            'ę' | 'é' | 'è' | 'ê' | 'ë' => 'e',
            'ó' | 'ö' | 'ô' => 'o',
            'ü' | 'ú' => 'u',
            'ł' => 'l',
            'ś' => 's',
            'ć' | 'ç' => 'c',
            'ń' | 'ñ' => 'n',
            'ź' | 'ż' => 'z',
            'v' => 'w',
            'y' | 'í' => 'i',
            c => c,
        });
    }
    plain.retain(char::is_alphanumeric);
    for (from, to) in [("ph", "f"), ("th", "t"), ("ck", "k"), ("ch", "h"), ("rz", "z"), ("q", "k"), ("x", "ks")] {
        plain = plain.replace(from, to);
    }
    let mut out: Vec<char> = plain.chars().collect();
    out.dedup();
    out
}

pub(crate) fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(vocabulary: &str, text: &str) -> String {
        Vocabulary::parse(vocabulary).correct(text)
    }

    #[test]
    fn prompt_lists_terms_without_replacement_syntax() {
        let v = Vocabulary::parse("NPaw, Hisense\ncloud kod -> Claude Code; Hisense");
        assert_eq!(v.prompt(), "NPaw, Hisense, Claude Code");
        assert_eq!(Vocabulary::parse(" ,\n ; ").prompt(), "");
    }

    #[test]
    fn fixes_case_spacing_and_near_spellings() {
        let v = "Hisense, Tizen, Kubernetes, Apple TV";
        assert_eq!(fix(v, "kupiłem telewizor hisense z systemem tyzen."), "kupiłem telewizor Hisense z systemem Tizen.");
        assert_eq!(fix(v, "to hi sense działa"), "to Hisense działa");
        assert_eq!(fix(v, "klaster kubernetis padł"), "klaster Kubernetes padł");
        assert_eq!(fix(v, "mam apple tv w domu"), "mam Apple TV w domu");
        assert_eq!(fix(v, "pilot do apple tv i hisense"), "pilot do Apple TV i Hisense");
    }

    #[test]
    fn keeps_polish_endings() {
        let v = "Klaudiusz, Kasia, Tomasz, Hisense";
        assert_eq!(fix(v, "rozmawiałem z klaudiuszem i kasią"), "rozmawiałem z Klaudiuszem i Kasią");
        assert_eq!(fix(v, "nie ma tomasza"), "nie ma Tomasza");
        assert_eq!(fix(v, "na hisensie"), "na Hisensie");
        assert_eq!(fix("Claude", "zapytaj claude'a"), "zapytaj Claude'a");
        // Double letters don't make a name too short ("Anna" folds to "ana").
        assert_eq!(fix("Anna, Otto", "rozmawiałem z anną i otto"), "rozmawiałem z Anną i Otto");
    }

    #[test]
    fn explicit_replacements() {
        let v = "cloud kod -> Claude Code, jira => Jira";
        assert_eq!(fix(v, "otworzyłem Cloud Kod, potem jirę i jira."), "otworzyłem Claude Code, potem Jirę i Jira.");
        // The longest replacement wins, whatever the order of the entries.
        assert_eq!(fix("cloud -> Claude, cloud kod -> Claude Code", "otworzyłem cloud kod i cloud"), "otworzyłem Claude Code i Claude");
    }

    #[test]
    fn leaves_other_words_alone() {
        let v = "CI, Tizen, Tomasz, Apple TV";
        assert_eq!(fix(v, "daję ci to"), "daję ci to");
        assert_eq!(fix(v, "to jest zwykłe zdanie o tym"), "to jest zwykłe zdanie o tym");
        assert_eq!(fix(v, "romasz i tomek"), "romasz i tomek");
        assert_eq!(fix("Kasia, Ania, Kuba", "kasa, ani kubek"), "kasa, ani kubek");
        // One-letter Polish words are not pieces of a name.
        assert_eq!(fix("Olek, Adam, Alek", "pytałem o lek, a dam ci znać, a lek nie działa"), "pytałem o lek, a dam ci znać, a lek nie działa");
        // …nor are short ones.
        assert_eq!(fix(v, "no to masz rację"), "no to masz rację");
        assert_eq!(fix("Dorota", "wrócił do roty"), "wrócił do roty");
        // A name that folds to a short key matches its own forms only.
        assert_eq!(fix("Otto, Anna", "oto dlaczego ana"), "oto dlaczego ana");
        assert_eq!(fix("Otto", "otto i otta"), "Otto i Otta");
        // A match doesn't cross punctuation.
        assert_eq!(fix(v, "apple, tv"), "apple, tv");
        assert_eq!(fix("", "bez słownika"), "bez słownika");
    }

    #[test]
    fn keeps_surrounding_punctuation_and_spaces() {
        assert_eq!(fix("Hisense", "„hisense”, a potem\n\nhisense?"), "„Hisense”, a potem\n\nHisense?");
    }
}
