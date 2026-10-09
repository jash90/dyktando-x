//! Języki do wyboru przy przepisywaniu spotkania: wszystkie, które zna Whisper (kody jak
//! w whisper.cpp), z polskimi nazwami. Parakeet v3 i Canary v2 znają tylko `EUROPEAN`.
use serde::Serialize;

pub const ALL: &[(&str, &str)] = &[
    ("pl", "polski"),
    ("en", "angielski"),
    ("af", "afrikaans"),
    ("sq", "albański"),
    ("am", "amharski"),
    ("ar", "arabski"),
    ("as", "asamski"),
    ("az", "azerbejdżański"),
    ("eu", "baskijski"),
    ("ba", "baszkirski"),
    ("bn", "bengalski"),
    ("be", "białoruski"),
    ("my", "birmański"),
    ("bs", "bośniacki"),
    ("br", "bretoński"),
    ("bg", "bułgarski"),
    ("zh", "chiński"),
    ("hr", "chorwacki"),
    ("cs", "czeski"),
    ("da", "duński"),
    ("et", "estoński"),
    ("fo", "farerski"),
    ("fi", "fiński"),
    ("fr", "francuski"),
    ("gl", "galicyjski"),
    ("el", "grecki"),
    ("ka", "gruziński"),
    ("gu", "gudżarati"),
    ("ha", "hausa"),
    ("haw", "hawajski"),
    ("he", "hebrajski"),
    ("hi", "hindi"),
    ("es", "hiszpański"),
    ("id", "indonezyjski"),
    ("is", "islandzki"),
    ("ja", "japoński"),
    ("jw", "jawajski"),
    ("yi", "jidysz"),
    ("yo", "joruba"),
    ("kn", "kannada"),
    ("yue", "kantoński"),
    ("ca", "kataloński"),
    ("kk", "kazachski"),
    ("km", "khmerski"),
    ("ko", "koreański"),
    ("ht", "kreolski haitański"),
    ("lo", "laotański"),
    ("ln", "lingala"),
    ("lt", "litewski"),
    ("lb", "luksemburski"),
    ("la", "łaciński"),
    ("lv", "łotewski"),
    ("mk", "macedoński"),
    ("ml", "malajalam"),
    ("ms", "malajski"),
    ("mg", "malgaski"),
    ("mt", "maltański"),
    ("mi", "maoryski"),
    ("mr", "marathi"),
    ("mn", "mongolski"),
    ("ne", "nepalski"),
    ("nl", "niderlandzki"),
    ("de", "niemiecki"),
    ("no", "norweski"),
    ("nn", "norweski (nynorsk)"),
    ("oc", "oksytański"),
    ("hy", "ormiański"),
    ("ps", "paszto"),
    ("pa", "pendżabski"),
    ("fa", "perski"),
    ("pt", "portugalski"),
    ("ru", "rosyjski"),
    ("ro", "rumuński"),
    ("sa", "sanskryt"),
    ("sr", "serbski"),
    ("sn", "shona"),
    ("sd", "sindhi"),
    ("sk", "słowacki"),
    ("sl", "słoweński"),
    ("so", "somalijski"),
    ("sw", "suahili"),
    ("su", "sundajski"),
    ("si", "syngaleski"),
    ("sv", "szwedzki"),
    ("tg", "tadżycki"),
    ("tl", "tagalski"),
    ("th", "tajski"),
    ("ta", "tamilski"),
    ("tt", "tatarski"),
    ("te", "telugu"),
    ("tr", "turecki"),
    ("tk", "turkmeński"),
    ("bo", "tybetański"),
    ("uk", "ukraiński"),
    ("ur", "urdu"),
    ("uz", "uzbecki"),
    ("cy", "walijski"),
    ("hu", "węgierski"),
    ("vi", "wietnamski"),
    ("it", "włoski"),
];

/// Języki Parakeeta v3 i Canary v2.
pub const EUROPEAN: &[&str] = &[
    "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it", "lt", "lv", "mt", "nl", "pl", "pt", "ro", "ru", "sk",
    "sl", "sv", "uk",
];

pub fn name(code: &str) -> Option<&'static str> {
    ALL.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

#[derive(Serialize)]
pub struct LanguageInfo {
    code: &'static str,
    name: &'static str,
    /// Obsługiwany też przez Parakeeta i Canary.
    european: bool,
}

#[tauri::command]
pub fn list_languages() -> Vec<LanguageInfo> {
    ALL.iter().map(|&(code, name)| LanguageInfo { code, name, european: EUROPEAN.contains(&code) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_known_to_whisper_and_unique() {
        for (code, _) in ALL {
            assert!(whisper_rs::get_lang_id(code).is_some(), "{code}");
        }
        let mut codes: Vec<_> = ALL.iter().map(|(c, _)| *c).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), ALL.len());
        // whisper large-v3 zna 100 języków.
        assert_eq!(ALL.len() as i32, whisper_rs::get_lang_max_id() + 1);
        for code in EUROPEAN {
            assert!(name(code).is_some(), "{code}");
        }
    }
}
