//! Languages to choose from when transcribing a meeting: all that Whisper knows (codes as
//! in whisper.cpp), with Polish and English names (sorted by the Polish name, Polish and English
//! first). Parakeet v3 and Canary v2 only know `EUROPEAN`.
use serde::Serialize;

use crate::i18n::{self, Locale};

/// `(code, Polish name, English name)`.
pub const ALL: &[(&str, &str, &str)] = &[
    ("pl", "polski", "Polish"),
    ("en", "angielski", "English"),
    ("af", "afrikaans", "Afrikaans"),
    ("sq", "albański", "Albanian"),
    ("am", "amharski", "Amharic"),
    ("ar", "arabski", "Arabic"),
    ("as", "asamski", "Assamese"),
    ("az", "azerbejdżański", "Azerbaijani"),
    ("eu", "baskijski", "Basque"),
    ("ba", "baszkirski", "Bashkir"),
    ("bn", "bengalski", "Bengali"),
    ("be", "białoruski", "Belarusian"),
    ("my", "birmański", "Burmese"),
    ("bs", "bośniacki", "Bosnian"),
    ("br", "bretoński", "Breton"),
    ("bg", "bułgarski", "Bulgarian"),
    ("zh", "chiński", "Chinese"),
    ("hr", "chorwacki", "Croatian"),
    ("cs", "czeski", "Czech"),
    ("da", "duński", "Danish"),
    ("et", "estoński", "Estonian"),
    ("fo", "farerski", "Faroese"),
    ("fi", "fiński", "Finnish"),
    ("fr", "francuski", "French"),
    ("gl", "galicyjski", "Galician"),
    ("el", "grecki", "Greek"),
    ("ka", "gruziński", "Georgian"),
    ("gu", "gudżarati", "Gujarati"),
    ("ha", "hausa", "Hausa"),
    ("haw", "hawajski", "Hawaiian"),
    ("he", "hebrajski", "Hebrew"),
    ("hi", "hindi", "Hindi"),
    ("es", "hiszpański", "Spanish"),
    ("id", "indonezyjski", "Indonesian"),
    ("is", "islandzki", "Icelandic"),
    ("ja", "japoński", "Japanese"),
    ("jw", "jawajski", "Javanese"),
    ("yi", "jidysz", "Yiddish"),
    ("yo", "joruba", "Yoruba"),
    ("kn", "kannada", "Kannada"),
    ("yue", "kantoński", "Cantonese"),
    ("ca", "kataloński", "Catalan"),
    ("kk", "kazachski", "Kazakh"),
    ("km", "khmerski", "Khmer"),
    ("ko", "koreański", "Korean"),
    ("ht", "kreolski haitański", "Haitian Creole"),
    ("lo", "laotański", "Lao"),
    ("ln", "lingala", "Lingala"),
    ("lt", "litewski", "Lithuanian"),
    ("lb", "luksemburski", "Luxembourgish"),
    ("la", "łaciński", "Latin"),
    ("lv", "łotewski", "Latvian"),
    ("mk", "macedoński", "Macedonian"),
    ("ml", "malajalam", "Malayalam"),
    ("ms", "malajski", "Malay"),
    ("mg", "malgaski", "Malagasy"),
    ("mt", "maltański", "Maltese"),
    ("mi", "maoryski", "Maori"),
    ("mr", "marathi", "Marathi"),
    ("mn", "mongolski", "Mongolian"),
    ("ne", "nepalski", "Nepali"),
    ("nl", "niderlandzki", "Dutch"),
    ("de", "niemiecki", "German"),
    ("no", "norweski", "Norwegian"),
    ("nn", "norweski (nynorsk)", "Norwegian (Nynorsk)"),
    ("oc", "oksytański", "Occitan"),
    ("hy", "ormiański", "Armenian"),
    ("ps", "paszto", "Pashto"),
    ("pa", "pendżabski", "Punjabi"),
    ("fa", "perski", "Persian"),
    ("pt", "portugalski", "Portuguese"),
    ("ru", "rosyjski", "Russian"),
    ("ro", "rumuński", "Romanian"),
    ("sa", "sanskryt", "Sanskrit"),
    ("sr", "serbski", "Serbian"),
    ("sn", "shona", "Shona"),
    ("sd", "sindhi", "Sindhi"),
    ("sk", "słowacki", "Slovak"),
    ("sl", "słoweński", "Slovenian"),
    ("so", "somalijski", "Somali"),
    ("sw", "suahili", "Swahili"),
    ("su", "sundajski", "Sundanese"),
    ("si", "syngaleski", "Sinhala"),
    ("sv", "szwedzki", "Swedish"),
    ("tg", "tadżycki", "Tajik"),
    ("tl", "tagalski", "Tagalog"),
    ("th", "tajski", "Thai"),
    ("ta", "tamilski", "Tamil"),
    ("tt", "tatarski", "Tatar"),
    ("te", "telugu", "Telugu"),
    ("tr", "turecki", "Turkish"),
    ("tk", "turkmeński", "Turkmen"),
    ("bo", "tybetański", "Tibetan"),
    ("uk", "ukraiński", "Ukrainian"),
    ("ur", "urdu", "Urdu"),
    ("uz", "uzbecki", "Uzbek"),
    ("cy", "walijski", "Welsh"),
    ("hu", "węgierski", "Hungarian"),
    ("vi", "wietnamski", "Vietnamese"),
    ("it", "włoski", "Italian"),
];

/// Languages of Parakeet v3 and Canary v2.
pub const EUROPEAN: &[&str] = &[
    "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it", "lt", "lv", "mt", "nl", "pl", "pt", "ro", "ru", "sk",
    "sl", "sv", "uk",
];

/// Name in the current UI language.
pub fn name(code: &str) -> Option<&'static str> {
    ALL.iter().find(|(c, _, _)| *c == code).map(|&(_, pl, en)| localized(pl, en))
}

fn localized(pl: &'static str, en: &'static str) -> &'static str {
    match i18n::current() {
        Locale::Pl => pl,
        Locale::En => en,
    }
}

#[derive(Serialize)]
pub struct LanguageInfo {
    code: &'static str,
    name: &'static str,
    /// Also supported by Parakeet and Canary.
    european: bool,
}

/// In the current UI language; in English sorted by the English name (Polish and English stay first).
#[tauri::command]
pub fn list_languages() -> Vec<LanguageInfo> {
    let mut list: Vec<LanguageInfo> =
        ALL.iter().map(|&(code, pl, en)| LanguageInfo { code, name: localized(pl, en), european: EUROPEAN.contains(&code) }).collect();
    if i18n::current() == Locale::En {
        list[2..].sort_by(|a, b| a.name.cmp(b.name));
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_known_to_whisper_and_unique() {
        for (code, _, _) in ALL {
            assert!(whisper_rs::get_lang_id(code).is_some(), "{code}");
        }
        let mut codes: Vec<_> = ALL.iter().map(|(c, _, _)| *c).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), ALL.len());
        // whisper large-v3 knows 100 languages.
        assert_eq!(ALL.len() as i32, whisper_rs::get_lang_max_id() + 1);
        for code in EUROPEAN {
            assert!(name(code).is_some(), "{code}");
        }
    }
}
