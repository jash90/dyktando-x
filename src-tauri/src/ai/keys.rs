//! AI provider API keys in the system keychain (macOS Keychain, Windows Credential Manager,
//! Secret Service on Linux) — never in `settings.json` or in logs.
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
    // A missing entry is not an error — deletion must be idempotent.
    if let Ok(e) = entry(service, account) {
        let _ = e.delete_credential();
    }
}

/// Stores the key; an empty string deletes the entry.
pub fn set(id: ProviderId, value: &str) -> anyhow::Result<()> {
    set_in(SERVICE, id.key(), value)
}

/// The provider's key; `None` when missing or empty.
pub fn get(id: ProviderId) -> Option<String> {
    get_in(SERVICE, id.key())
}

pub fn has(id: ProviderId) -> bool {
    get(id).is_some()
}

/// Service under which Dyktando for macOS (Swift version) keeps keys — accounts use the same names.
pub const LEGACY_SERVICE: &str = "com.bartekzimny.dyktando.ai";

/// One-time import of keys from Dyktando (Swift): only for providers without a key of their own.
/// macOS will ask for consent to read another app's entry. Returns names of imported providers.
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

    /// Touches the real keychain, so only on demand: `cargo test -- --ignored keychain`.
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
