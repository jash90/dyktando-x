//! Postprocessing dyktowania (port `Postprocess/` ze Swifta): znaczniki dyktowania →
//! kropka na końcu dłuższej wypowiedzi → wielkie litery po zdaniach → sprzątanie spacji.

const RULES: &[(&str, &str)] = &[
    ("kropka", "."),
    ("przecinek", ","),
    ("znak zapytania", "?"),
    ("wykrzyknik", "!"),
    ("nowa linia", "\n"),
    ("nowy akapit", "\n\n"),
    ("dwukropek", ":"),
    ("średnik", ";"),
];

/// Zamienia znaczniki występujące jako osobne słowa (otoczone spacjami).
pub fn replace_markers(text: &str) -> String {
    let mut out = format!(" {text} ");
    for (marker, replacement) in RULES {
        let pattern = format!(" {marker} ");
        let with = format!(" {replacement} ");
        while out.contains(&pattern) {
            out = out.replacen(&pattern, &with, 1);
        }
    }
    out.trim_matches(' ').to_string()
}

/// Kropka na końcu, gdy brak znaku końca zdania i wypowiedź ma ≥ 6 słów
/// (Whisper sam stawia interpunkcję, Parakeet często nie).
pub fn punctuate(text: &str) -> String {
    let trimmed = text.trim();
    let Some(last) = trimmed.chars().last() else {
        return text.to_string();
    };
    if ".!?".contains(last) {
        return trimmed.to_string();
    }
    if trimmed.split_whitespace().count() >= 6 {
        format!("{trimmed}.")
    } else {
        trimmed.to_string()
    }
}

/// Wielka litera na początku i po każdym `.`, `!`, `?`; zachowuje polskie znaki.
pub fn capitalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut next = true;
    for ch in text.chars() {
        if next && ch.is_alphabetic() {
            out.extend(ch.to_uppercase());
            next = false;
        } else {
            out.push(ch);
        }
        if ".!?".contains(ch) {
            next = true;
        }
    }
    out
}

fn smart_space(s: &str) -> String {
    let mut s = s.to_string();
    for p in [".", ",", "?", "!", ":", ";"] {
        s = s.replace(&format!(" {p}"), p);
    }
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    // Nowa linia z dyktowania nie powinna zostawiać spacji na końcu i na początku wiersza.
    s.replace(" \n", "\n").replace("\n ", "\n")
}

pub fn apply(raw: &str) -> String {
    smart_space(&capitalize(&punctuate(&replace_markers(raw))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_become_punctuation() {
        assert_eq!(apply("ala ma kota kropka"), "Ala ma kota.");
        assert_eq!(apply("czy to działa znak zapytania"), "Czy to działa?");
        assert_eq!(apply("raz przecinek dwa"), "Raz, dwa");
    }

    #[test]
    fn marker_inside_word_is_kept() {
        assert_eq!(replace_markers("kropkami"), "kropkami");
    }

    #[test]
    fn long_utterance_gets_period() {
        assert_eq!(punctuate("to jest dość długie zdanie bez kropki"), "to jest dość długie zdanie bez kropki.");
        assert_eq!(punctuate("krótkie zdanie"), "krótkie zdanie");
        assert_eq!(punctuate("już ma!"), "już ma!");
    }

    #[test]
    fn capitalizes_polish_letters_after_sentences() {
        assert_eq!(capitalize("źle. łatwo! ćma? żaba"), "Źle. Łatwo! Ćma? Żaba");
    }

    #[test]
    fn new_paragraph_marker() {
        assert_eq!(apply("pierwszy nowy akapit drugi"), "Pierwszy\n\ndrugi");
    }

    #[test]
    fn empty_input() {
        assert_eq!(apply(""), "");
        assert_eq!(apply("   "), "");
    }
}
