//! Dictation postprocessing (a port of `Postprocess/` from the Swift version): dictation markers →
//! period at the end of a longer utterance → capitalization after sentences → whitespace cleanup.
//! With the field's text around the cursor (`apply_in`) the dictation is also joined with it.

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

/// Replaces markers that occur as separate words (surrounded by spaces).
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

/// A period at the end when there's no sentence-ending mark and the utterance has ≥ 6 words
/// (Whisper adds punctuation itself, Parakeet often doesn't).
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

/// Capital letter at the start and after every `.`, `!`, `?`; preserves Polish characters.
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
    // A dictated new line shouldn't leave spaces at the end and start of a line.
    s.replace(" \n", "\n").replace("\n ", "\n")
}

pub fn apply(raw: &str) -> String {
    smart_space(&capitalize(&punctuate(&replace_markers(raw))))
}

/// The field's text around the cursor where the dictation goes (see `focus::FieldText`).
pub struct Around<'a> {
    pub before: &'a str,
    pub after: &'a str,
}

/// Whether the text before the cursor ends in the middle of a sentence.
fn mid_sentence(before: &str) -> bool {
    if before.trim_end_matches([' ', '\t']).ends_with('\n') {
        return false;
    }
    before.trim_end().chars().last().is_some_and(|c| !".!?…".contains(c))
}

/// The first word in lowercase — unless it's a name (`keep`), an abbreviation ("NPaw", "API") or "I".
fn lowercase_first(text: &str, keep: impl Fn(&str) -> bool) -> String {
    let word = text.split_whitespace().next().unwrap_or("");
    let core = word.trim_matches(|c: char| !c.is_alphanumeric());
    let mut chars = core.chars();
    let Some(first) = chars.next() else { return text.to_string() };
    let rest_lower = chars.all(|c| !c.is_uppercase());
    if !first.is_uppercase() || !rest_lower || core == "I" || keep(core) {
        return text.to_string();
    }
    let at = text.find(first).expect("the first letter is in the text");
    format!("{}{}{}", &text[..at], first.to_lowercase(), &text[at + first.len_utf8()..])
}

/// Like `apply`, joined with what's already in the field: in the middle of a sentence the first
/// word stays lowercase (`keep` = a name to leave as is), there's no period when the sentence goes
/// on after the cursor, and spaces where the words would stick together.
pub fn apply_in(raw: &str, around: &Around, keep: impl Fn(&str) -> bool) -> String {
    let text = replace_markers(raw);
    let continues = around.after.trim_start().starts_with(|c: char| c.is_alphanumeric());
    let text = if continues {
        let t = text.trim();
        t.strip_suffix('.').filter(|t| !t.ends_with('.')).unwrap_or(t).to_string()
    } else {
        punctuate(&text)
    };
    let mut text = smart_space(&capitalize(&text));
    if mid_sentence(around.before) {
        text = lowercase_first(&text, keep);
    }
    if text.is_empty() {
        return text;
    }
    let opening = "([{„\"'«/-";
    if around.before.chars().last().is_some_and(|c| !c.is_whitespace() && !opening.contains(c)) && text.starts_with(char::is_alphanumeric) {
        text.insert(0, ' ');
    }
    if continues && around.after.starts_with(char::is_alphanumeric) {
        text.push(' ');
    }
    text
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

    fn joined(raw: &str, before: &str, after: &str) -> String {
        apply_in(raw, &Around { before, after }, |w| w == "Kasia")
    }

    #[test]
    fn continues_the_sentence_before_the_cursor() {
        assert_eq!(joined("Jutro o dziesiątej", "Spotkanie mamy", ""), " jutro o dziesiątej");
        assert_eq!(joined("Kasia też będzie", "Na spotkaniu,", ""), " Kasia też będzie");
        assert_eq!(joined("NPaw działa", "Sprawdziłem i", ""), " NPaw działa");
        assert_eq!(joined("Nowe zdanie tutaj", "Koniec.", ""), " Nowe zdanie tutaj");
        assert_eq!(joined("Nowe zdanie", "Lista:\n", ""), "Nowe zdanie");
        assert_eq!(joined("Początek", "", ""), "Początek");
        assert_eq!(joined("w nawiasie", "(", ""), "w nawiasie");
    }

    #[test]
    fn no_period_when_the_sentence_goes_on_after_the_cursor() {
        assert_eq!(joined("Bardzo ważny i pilny raport z całego tygodnia", "To jest", "do przeczytania."), " bardzo ważny i pilny raport z całego tygodnia ");
        assert_eq!(joined("Raport.", "", " dla zespołu"), "Raport");
        assert_eq!(joined("to jest dość długie zdanie bez kropki", "", ""), "To jest dość długie zdanie bez kropki.");
    }

    #[test]
    fn empty_input() {
        assert_eq!(apply(""), "");
        assert_eq!(apply("   "), "");
    }
}
