//! UI language of the backend-produced texts (tray, HUD, dialogs, window titles, errors shown in the
//! UI, job steps). English is the default and the fallback; Polish when the system's first preferred
//! language is Polish or when chosen in settings (`ui_language`). The dictation / meeting language
//! is a separate thing (see `settings::Language`).
//!
//! Translations live in `locales/{en,pl}.json` (flat `key -> text`, `{name}` placeholders, plural
//! forms as `key_one` / `key_few` / `key_many` / `key_other`). A missing Polish key falls back to
//! English, a missing English key to the key itself.
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    En,
    Pl,
}

impl Locale {
    pub fn code(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Pl => "pl",
        }
    }

    fn table(self) -> &'static HashMap<String, String> {
        match self {
            Locale::En => &EN,
            Locale::Pl => &PL,
        }
    }
}

fn parse_table(json: &str, name: &str) -> HashMap<String, String> {
    serde_json::from_str(json).unwrap_or_else(|e| panic!("locales/{name}.json: {e}"))
}

static EN: Lazy<HashMap<String, String>> = Lazy::new(|| parse_table(include_str!("../locales/en.json"), "en"));
static PL: Lazy<HashMap<String, String>> = Lazy::new(|| parse_table(include_str!("../locales/pl.json"), "pl"));

/// 0 = English, 1 = Polish.
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> Locale {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        Locale::Pl
    } else {
        Locale::En
    }
}

/// Sets the current locale; returns whether it changed.
pub fn set(locale: Locale) -> bool {
    let value = if locale == Locale::Pl { 1 } else { 0 };
    CURRENT.swap(value, Ordering::Relaxed) != value
}

/// Polish when the language tag's primary subtag is `pl` (`pl`, `pl-PL`, `pl_PL.UTF-8`…).
fn from_tag(tag: Option<&str>) -> Locale {
    let primary = tag.and_then(|t| t.split(['-', '_', '.', '@']).next()).unwrap_or("");
    if primary.eq_ignore_ascii_case("pl") {
        Locale::Pl
    } else {
        Locale::En
    }
}

/// The `ui_language` setting → locale: `"en"` / `"pl"` win, anything else (`"system"`) follows the
/// system's first preferred language.
pub fn resolve(setting: &str) -> Locale {
    match setting {
        "en" => Locale::En,
        "pl" => Locale::Pl,
        _ => from_tag(sys_locale::get_locales().next().as_deref()),
    }
}

fn lookup(locale: Locale, key: &str) -> Option<&'static str> {
    locale.table().get(key).or_else(|| EN.get(key)).map(String::as_str)
}

fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

pub fn t_in(locale: Locale, key: &str) -> String {
    lookup(locale, key).unwrap_or(key).to_string()
}

pub fn t_with_in(locale: Locale, key: &str, args: &[(&str, &dyn Display)]) -> String {
    fill(lookup(locale, key).unwrap_or(key), args)
}

/// Text for `key` in the current locale.
pub fn t(key: &str) -> String {
    t_in(current(), key)
}

/// Text for `key` in the current locale with `{name}` placeholders filled in.
pub fn t_with(key: &str, args: &[(&str, &dyn Display)]) -> String {
    t_with_in(current(), key, args)
}

/// CLDR plural category of an integer `n`.
fn plural_category(locale: Locale, n: u64) -> &'static str {
    match locale {
        Locale::En => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
        Locale::Pl => {
            let (m10, m100) = (n % 10, n % 100);
            if n == 1 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
    }
}

/// The plural form of `key` for `n` (falling back to `key_other`, then to the key itself).
fn plural_template<'a>(locale: Locale, key: &'a str, n: u64, get: impl Fn(&str) -> Option<&'a str>) -> &'a str {
    get(&format!("{key}_{}", plural_category(locale, n))).or_else(|| get(&format!("{key}_other"))).unwrap_or(key)
}

pub fn plural_in(locale: Locale, key: &str, n: u64, args: &[(&str, &dyn Display)]) -> String {
    let template = plural_template(locale, key, n, |k| lookup(locale, k));
    let mut all: Vec<(&str, &dyn Display)> = vec![("count", &n)];
    all.extend_from_slice(args);
    fill(template, &all)
}

/// Count-dependent text: picks `key_one` / `key_few` / `key_many` / `key_other` and fills `{count}`.
#[allow(dead_code)] // available for count-dependent texts; the frontend has most of them
pub fn plural(key: &str, n: u64, args: &[(&str, &dyn Display)]) -> String {
    plural_in(current(), key, n, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value};
    use std::collections::{BTreeMap, BTreeSet};

    const PLURAL_SUFFIXES: [&str; 4] = ["_one", "_few", "_many", "_other"];

    fn load(json: &str, name: &str) -> Map<String, Value> {
        let value: Value = serde_json::from_str(json).unwrap_or_else(|e| panic!("{name}: {e}"));
        let map = value.as_object().unwrap_or_else(|| panic!("{name}: not a JSON object")).clone();
        for (k, v) in &map {
            assert!(v.is_string(), "{name}: {k} is not a string");
        }
        map
    }

    /// Base key → set of plural suffixes (empty for a plain key).
    fn bases(map: &Map<String, Value>) -> BTreeMap<String, BTreeSet<&'static str>> {
        let mut out: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
        for key in map.keys() {
            match PLURAL_SUFFIXES.iter().find(|s| key.ends_with(*s)) {
                Some(s) => {
                    out.entry(key[..key.len() - s.len()].to_string()).or_default().insert(s);
                }
                None => {
                    out.entry(key.clone()).or_default();
                }
            }
        }
        out
    }

    fn placeholders(text: &str) -> BTreeSet<String> {
        let re = regex::Regex::new(r"\{([A-Za-z0-9_]+)\}").unwrap();
        re.captures_iter(text).map(|c| c[1].to_string()).collect()
    }

    /// Placeholders of a base key: union over its forms (`{count}` is optional in the `_one` form).
    fn base_placeholders(map: &Map<String, Value>, base: &str, suffixes: &BTreeSet<&str>) -> BTreeSet<String> {
        if suffixes.is_empty() {
            return placeholders(map[base].as_str().unwrap());
        }
        let mut all = BTreeSet::new();
        for s in suffixes {
            all.extend(placeholders(map[&format!("{base}{s}")].as_str().unwrap()));
        }
        all.remove("count");
        all
    }

    fn check_pair(en_json: &str, pl_json: &str, what: &str) {
        let (en, pl) = (load(en_json, &format!("{what}/en.json")), load(pl_json, &format!("{what}/pl.json")));
        let (en_bases, pl_bases) = (bases(&en), bases(&pl));
        let en_keys: BTreeSet<_> = en_bases.keys().collect();
        let pl_keys: BTreeSet<_> = pl_bases.keys().collect();
        assert_eq!(
            en_keys.symmetric_difference(&pl_keys).collect::<Vec<_>>(),
            Vec::<&&String>::new(),
            "{what}: base keys differ between en and pl"
        );
        for (base, en_suffixes) in &en_bases {
            let pl_suffixes = &pl_bases[base];
            if en_suffixes.is_empty() || pl_suffixes.is_empty() {
                assert!(en_suffixes.is_empty() && pl_suffixes.is_empty(), "{what}: {base} is plural in one language only");
            } else {
                for s in ["_one", "_other"] {
                    assert!(en_suffixes.contains(s), "{what}: en {base} needs {base}{s}");
                }
                for s in ["_one", "_few", "_many"] {
                    assert!(pl_suffixes.contains(s), "{what}: pl {base} needs {base}{s}");
                }
            }
            assert_eq!(
                base_placeholders(&en, base, en_suffixes),
                base_placeholders(&pl, base, pl_suffixes),
                "{what}: placeholders of {base} differ"
            );
        }
    }

    #[test]
    fn backend_locales_have_the_same_keys_and_placeholders() {
        check_pair(include_str!("../locales/en.json"), include_str!("../locales/pl.json"), "src-tauri/locales");
    }

    #[test]
    fn frontend_locales_have_the_same_keys_and_placeholders() {
        check_pair(include_str!("../../src/locales/en.json"), include_str!("../../src/locales/pl.json"), "src/locales");
    }

    #[test]
    fn polish_plural_rule() {
        let cases = [(0, "many"), (1, "one"), (2, "few"), (4, "few"), (5, "many"), (12, "many"), (14, "many"), (22, "few"), (25, "many"), (112, "many"), (1001, "many"), (1002, "few")];
        for (n, expected) in cases {
            assert_eq!(plural_category(Locale::Pl, n), expected, "{n}");
        }
        assert_eq!(plural_category(Locale::En, 1), "one");
        for n in [0, 2, 5, 12, 22, 112] {
            assert_eq!(plural_category(Locale::En, n), "other", "{n}");
        }
    }

    #[test]
    fn plural_picks_the_form_and_fills_count() {
        let pl: HashMap<&str, &str> =
            [("d_one", "{count} dyktowanie"), ("d_few", "{count} dyktowania"), ("d_many", "{count} dyktowań")].into_iter().collect();
        let form = |n: u64| fill(plural_template(Locale::Pl, "d", n, |k| pl.get(k).copied()), &[("count", &n)]);
        assert_eq!(form(1), "1 dyktowanie");
        assert_eq!(form(3), "3 dyktowania");
        assert_eq!(form(5), "5 dyktowań");
        assert_eq!(form(22), "22 dyktowania");
        let en: HashMap<&str, &str> = [("d_one", "{count} dictation"), ("d_other", "{count} dictations")].into_iter().collect();
        assert_eq!(plural_template(Locale::En, "d", 2, |k| en.get(k).copied()), "{count} dictations");
        // A missing key falls back to the key itself.
        assert_eq!(plural_in(Locale::Pl, "no.such.key", 5, &[]), "no.such.key");
    }

    #[test]
    fn lookup_falls_back_to_english_then_to_the_key() {
        assert_eq!(t_in(Locale::Pl, "no.such.key"), "no.such.key");
        assert_eq!(t_in(Locale::En, "tray.quit"), "Quit Dyktando X");
        assert_eq!(t_in(Locale::Pl, "tray.quit"), "Zakończ Dyktando X");
        assert_eq!(t_with_in(Locale::En, "hud.meeting_saved", &[("duration", &"1:02")]), "Recording saved (1:02)");
    }

    #[test]
    fn system_tag_resolution() {
        assert_eq!(from_tag(Some("pl-PL")), Locale::Pl);
        assert_eq!(from_tag(Some("pl_PL.UTF-8")), Locale::Pl);
        assert_eq!(from_tag(Some("PL")), Locale::Pl);
        assert_eq!(from_tag(Some("en-PL")), Locale::En);
        assert_eq!(from_tag(Some("plt")), Locale::En);
        assert_eq!(from_tag(None), Locale::En);
        assert_eq!(resolve("pl"), Locale::Pl);
        assert_eq!(resolve("en"), Locale::En);
    }
}
