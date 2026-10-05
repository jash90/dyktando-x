//! Klucze API dostawców AI w pęku kluczy systemu (macOS Keychain, Windows Credential Manager,
//! Secret Service na Linuksie) — nigdy w `settings.json` ani w logach.
use anyhow::Context;

use crate::settings::ProviderId;

pub const SERVICE: &str = "com.bartekzimny.dyktandox.ai";

fn entry(service: &str, account: &str) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(service, account)
}

fn set_in(service: &str, account: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty() {
        delete_in(service, account);
        return Ok(());
    }
    entry(service, account)
        .and_then(|e| e.set_password(value))
        .context("Nie udało się zapisać klucza API w pęku kluczy")
}

fn get_in(service: &str, account: &str) -> Option<String> {
    entry(service, account).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

fn delete_in(service: &str, account: &str) {
    // Brak wpisu to nie błąd — usuwanie ma być idempotentne.
    if let Ok(e) = entry(service, account) {
        let _ = e.delete_credential();
    }
}

/// Zapisuje klucz; pusty napis usuwa wpis.
pub fn set(id: ProviderId, value: &str) -> anyhow::Result<()> {
    set_in(SERVICE, id.key(), value)
}

/// Klucz dostawcy; `None`, gdy brak albo pusty.
pub fn get(id: ProviderId) -> Option<String> {
    get_in(SERVICE, id.key())
}

pub fn has(id: ProviderId) -> bool {
    get(id).is_some()
}

/// Usługa, pod którą klucze trzyma Dyktando dla macOS (wersja Swift) — konta mają te same nazwy.
pub const LEGACY_SERVICE: &str = "com.bartekzimny.dyktando.ai";

/// Jednorazowy import kluczy z Dyktando (Swift): tylko dla dostawców bez własnego klucza.
/// macOS zapyta o zgodę na odczyt wpisu innej aplikacji. Zwraca nazwy zaimportowanych dostawców.
pub fn import_legacy() -> Vec<&'static str> {
    let mut imported = Vec::new();
    for id in ProviderId::ALL {
        if has(id) {
            continue;
        }
        if let Some(key) = get_in(LEGACY_SERVICE, id.key()) {
            if set(id, &key).is_ok() {
                imported.push(id.display_name());
            }
        }
    }
    imported
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dotyka prawdziwego pęku kluczy, więc tylko na żądanie: `cargo test -- --ignored keychain`.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let service = format!("com.bartekzimny.dyktandox.tests.{}", std::process::id());
        let account = "p";
        assert_eq!(get_in(&service, account), None);
        set_in(&service, account, "sekret-1").unwrap();
        assert_eq!(get_in(&service, account).as_deref(), Some("sekret-1"));
        set_in(&service, account, "sekret-2").unwrap();
        assert_eq!(get_in(&service, account).as_deref(), Some("sekret-2"));
        set_in(&service, account, "").unwrap();
        assert_eq!(get_in(&service, account), None);
        delete_in(&service, account);
    }
}
