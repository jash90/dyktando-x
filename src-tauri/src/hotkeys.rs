//! Globalne skróty (handy-keys: macOS CGEventTap, Windows hook, Linux evdev — działa też na
//! Waylandzie, gdy użytkownik ma dostęp do /dev/input).
//!
//! Zwykłe skróty (F5, ⌃⌥R…) idą przez `HotkeyManager` z blokowaniem, żeby F5 nie odświeżał
//! przeglądarki. Push-to-talk samym modyfikatorem (prawy ⌘/Ctrl) idzie osobnym, nieblokującym
//! nasłuchem — zablokowany prawy ⌘ zepsułby skróty typu ⌘C — z anulowaniem, gdy w trakcie
//! wciśnięto inny klawisz (port `ModifierPTTState` ze Swifta).
use anyhow::{anyhow, Result};
use handy_keys::{Hotkey, HotkeyManager, HotkeyState, KeyEvent, KeyboardListener, Modifiers};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PushToTalkDown,
    PushToTalkUp,
    /// Nagrywanie trzymanym modyfikatorem przerwane innym klawiszem — odrzuć nagranie.
    Cancel,
    Toggle,
    Meeting,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub push_to_talk: String,
    pub toggle: String,
    pub modifier: String,
    pub meeting: String,
}

pub struct Hotkeys {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    /// Ostrzeżenia dla użytkownika (np. brak uprawnień), pokazywane w ustawieniach.
    pub warnings: Vec<String>,
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

pub fn parse(s: &str) -> Result<Option<Hotkey>> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<Hotkey>().map(Some).map_err(|e| anyhow!("Nieprawidłowy skrót „{s}”: {e}"))
}

pub fn parse_modifier(s: &str) -> Result<Option<Modifiers>> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let hk = parse(s)?.expect("niepusty");
    if hk.key.is_some() || hk.modifiers.bits().count_ones() != 1 {
        return Err(anyhow!("„{s}” to nie jest pojedynczy modyfikator (np. CmdRight, CtrlRight)"));
    }
    Ok(Some(hk.modifiers))
}

impl Hotkeys {
    pub fn start(config: &Config, on_action: impl Fn(Action) + Send + Sync + Clone + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        let mut warnings = Vec::new();

        let mut bindings: Vec<(Hotkey, Kind)> = Vec::new();
        for (text, kind) in [
            (&config.push_to_talk, Kind::PushToTalk),
            (&config.toggle, Kind::Toggle),
            (&config.meeting, Kind::Meeting),
        ] {
            match parse(text) {
                Ok(Some(hk)) => bindings.push((hk, kind)),
                Ok(None) => {}
                Err(e) => warnings.push(e.to_string()),
            }
        }

        if !bindings.is_empty() {
            let manager = HotkeyManager::new_with_blocking().or_else(|e| {
                log::warn!("Skróty bez blokowania: {e}");
                HotkeyManager::new()
            });
            match manager {
                Ok(manager) => {
                    let mut ids = HashMap::new();
                    for (hk, kind) in bindings {
                        match manager.register(hk) {
                            Ok(id) => {
                                ids.insert(id, kind);
                            }
                            Err(e) => warnings.push(format!("Skrót {hk}: {e}")),
                        }
                    }
                    let stop = stop.clone();
                    let on_action = on_action.clone();
                    threads.push(std::thread::spawn(move || {
                        while !stop.load(Ordering::Relaxed) {
                            match manager.try_recv() {
                                Some(ev) => {
                                    if let Some(kind) = ids.get(&ev.id) {
                                        if let Some(a) = kind.action(ev.state) {
                                            on_action(a);
                                        }
                                    }
                                }
                                None => std::thread::sleep(Duration::from_millis(8)),
                            }
                        }
                    }));
                }
                Err(e) => warnings.push(permission_hint(&e.to_string())),
            }
        }

        match parse_modifier(&config.modifier) {
            Ok(Some(target)) => match KeyboardListener::new() {
                Ok(listener) => {
                    let stop = stop.clone();
                    threads.push(std::thread::spawn(move || {
                        let mut state = ModifierPtt::new(target);
                        while !stop.load(Ordering::Relaxed) {
                            if let Ok(ev) = listener.recv_timeout(Duration::from_millis(100)) {
                                if let Some(a) = state.handle(&ev) {
                                    on_action(a);
                                }
                            }
                        }
                    }));
                }
                Err(e) => warnings.push(permission_hint(&e.to_string())),
            },
            Ok(None) => {}
            Err(e) => warnings.push(e.to_string()),
        }

        Self { stop, threads, warnings }
    }
}

fn permission_hint(err: &str) -> String {
    if cfg!(target_os = "macos") {
        let _ = err;
        "Skróty globalne wymagają uprawnienia Dostępność: zakładka Uprawnienia → „Otwórz ustawienia”, włącz Dyktando X, potem „Sprawdź ponownie”.".to_string()
    } else if cfg!(target_os = "linux") {
        format!("Skróty globalne wymagają dostępu do /dev/input — dodaj regułę udev albo grupę „input” ({err})")
    } else {
        format!("Skróty globalne niedostępne: {err}")
    }
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    PushToTalk,
    Toggle,
    Meeting,
}

impl Kind {
    fn action(self, state: HotkeyState) -> Option<Action> {
        match (self, state) {
            (Kind::PushToTalk, HotkeyState::Pressed) => Some(Action::PushToTalkDown),
            (Kind::PushToTalk, HotkeyState::Released) => Some(Action::PushToTalkUp),
            (Kind::Toggle, HotkeyState::Pressed) => Some(Action::Toggle),
            (Kind::Meeting, HotkeyState::Pressed) => Some(Action::Meeting),
            _ => None,
        }
    }
}

/// Stan push-to-talk samym modyfikatorem — czysta logika, testowana.
#[derive(Debug)]
pub struct ModifierPtt {
    target: Modifiers,
    held: bool,
    cancelled: bool,
    last: Modifiers,
}

impl ModifierPtt {
    pub fn new(target: Modifiers) -> Self {
        Self { target, held: false, cancelled: false, last: Modifiers::empty() }
    }

    pub fn handle(&mut self, ev: &KeyEvent) -> Option<Action> {
        if ev.key.is_some() {
            // Zwykły klawisz przy trzymanym modyfikatorze (⌘C, Ctrl+Tab…) — to skrót.
            return if ev.is_key_down && self.held { self.cancel() } else { None };
        }
        let changed = ev.changed_modifier.unwrap_or(self.last ^ ev.modifiers);
        self.last = ev.modifiers;
        if changed != self.target {
            // Inny modyfikator w trakcie (np. prawy ⌘ + ⇧) — skrót, nie dyktowanie.
            return if self.held && ev.modifiers.intersects(changed) { self.cancel() } else { None };
        }
        let down = ev.modifiers.contains(self.target);
        if down && !self.held {
            self.held = true;
            self.cancelled = false;
            // Wciśnięty razem z innym modyfikatorem od początku — nie startujemy.
            if ev.modifiers != self.target {
                self.cancelled = true;
                return None;
            }
            return Some(Action::PushToTalkDown);
        }
        if !down && self.held {
            self.held = false;
            return if self.cancelled { None } else { Some(Action::PushToTalkUp) };
        }
        None
    }

    fn cancel(&mut self) -> Option<Action> {
        if self.cancelled {
            return None;
        }
        self.cancelled = true;
        Some(Action::Cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use handy_keys::Key;

    fn modev(mods: Modifiers, changed: Modifiers) -> KeyEvent {
        KeyEvent { modifiers: mods, key: None, is_key_down: !mods.is_empty(), changed_modifier: Some(changed) }
    }
    fn key(mods: Modifiers, k: Key, down: bool) -> KeyEvent {
        KeyEvent { modifiers: mods, key: Some(k), is_key_down: down, changed_modifier: None }
    }

    const R: Modifiers = Modifiers::CMD_RIGHT;

    #[test]
    fn press_and_release_dictates() {
        let mut s = ModifierPtt::new(R);
        assert_eq!(s.handle(&modev(R, R)), Some(Action::PushToTalkDown));
        assert_eq!(s.handle(&modev(Modifiers::empty(), R)), Some(Action::PushToTalkUp));
    }

    #[test]
    fn other_key_cancels_once() {
        let mut s = ModifierPtt::new(R);
        s.handle(&modev(R, R));
        assert_eq!(s.handle(&key(R, Key::C, true)), Some(Action::Cancel));
        assert_eq!(s.handle(&key(R, Key::V, true)), None);
        assert_eq!(s.handle(&modev(Modifiers::empty(), R)), None);
        // następne przytrzymanie działa normalnie
        assert_eq!(s.handle(&modev(R, R)), Some(Action::PushToTalkDown));
    }

    #[test]
    fn other_modifier_cancels() {
        let mut s = ModifierPtt::new(R);
        s.handle(&modev(R, R));
        assert_eq!(s.handle(&modev(R | Modifiers::SHIFT_LEFT, Modifiers::SHIFT_LEFT)), Some(Action::Cancel));
        s.handle(&modev(R, Modifiers::SHIFT_LEFT));
        assert_eq!(s.handle(&modev(Modifiers::empty(), R)), None);
    }

    #[test]
    fn left_cmd_is_ignored() {
        let mut s = ModifierPtt::new(R);
        assert_eq!(s.handle(&modev(Modifiers::CMD_LEFT, Modifiers::CMD_LEFT)), None);
        assert_eq!(s.handle(&modev(Modifiers::empty(), Modifiers::CMD_LEFT)), None);
    }

    #[test]
    fn target_pressed_while_other_held_does_not_start() {
        let mut s = ModifierPtt::new(R);
        s.handle(&modev(Modifiers::SHIFT_LEFT, Modifiers::SHIFT_LEFT));
        assert_eq!(s.handle(&modev(Modifiers::SHIFT_LEFT | R, R)), None);
        assert_eq!(s.handle(&modev(Modifiers::SHIFT_LEFT, R)), None);
    }

    #[test]
    fn works_without_changed_modifier_field() {
        let mut s = ModifierPtt::new(R);
        let ev = |m: Modifiers| KeyEvent { modifiers: m, key: None, is_key_down: true, changed_modifier: None };
        assert_eq!(s.handle(&ev(R)), Some(Action::PushToTalkDown));
        assert_eq!(s.handle(&ev(Modifiers::empty())), Some(Action::PushToTalkUp));
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_modifier("CmdRight").unwrap(), Some(Modifiers::CMD_RIGHT));
        assert!(parse_modifier("Cmd+K").is_err());
        assert!(parse("F5").unwrap().is_some());
        assert!(parse("").unwrap().is_none());
    }
}
