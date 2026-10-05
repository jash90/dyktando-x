//! Wstawianie tekstu w aktywnym oknie: tekst do schowka → symulowane wklejenie →
//! przywrócenie poprzedniej zawartości schowka (tylko tekstu — obrazków arboard nie odtworzy).
//!
//! Wklejanie: macOS ⌘V (CGEvent przez enigo, wymaga Dostępności), Windows Ctrl+V (SendInput),
//! Linux X11 Ctrl+V (XTest przez enigo), Linux Wayland: wtype → dotool → ydotool, a gdy
//! żadne nie działa — tylko schowek.
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Wklejone do aktywnego okna.
    Pasted,
    /// Tylko w schowku (brak uprawnień, brak narzędzia, wybór użytkownika).
    Clipboard,
}

/// Schowek musi żyć przez cały czas działania aplikacji: na X11/Wayland to my „serwujemy”
/// zawartość innym programom — po zniszczeniu obiektu wklejany tekst by znikał.
static CLIPBOARD: Lazy<Mutex<Option<arboard::Clipboard>>> = Lazy::new(|| Mutex::new(arboard::Clipboard::new().ok()));

fn with_clipboard<T>(f: impl FnOnce(&mut arboard::Clipboard) -> T) -> Result<T> {
    let mut guard = CLIPBOARD.lock().map_err(|_| anyhow!("schowek zablokowany"))?;
    if guard.is_none() {
        *guard = Some(arboard::Clipboard::new().map_err(|e| anyhow!("schowek: {e}"))?);
    }
    Ok(f(guard.as_mut().expect("schowek")))
}

pub fn set_clipboard(text: &str) -> Result<()> {
    with_clipboard(|c| c.set_text(text.to_string()))?.map_err(|e| anyhow!("schowek: {e}"))
}

pub fn clipboard_text() -> Option<String> {
    with_clipboard(|c| c.get_text().ok()).ok().flatten()
}

/// Czy system pozwala nam symulować klawisze (macOS: Dostępność; gdzie indziej zawsze tak).
pub fn can_send_keys() -> bool {
    #[cfg(target_os = "macos")]
    {
        handy_keys::check_accessibility()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

pub fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var("XDG_SESSION_TYPE").map(|v| v == "wayland").unwrap_or(false)
            || std::env::var_os("WAYLAND_DISPLAY").is_some())
}

/// Wstawia tekst. `paste = false` → tylko schowek (bez przywracania — użytkownik wklei sam).
pub fn insert(text: &str, paste: bool) -> Result<Outcome> {
    if !paste || !can_send_keys() {
        set_clipboard(text)?;
        return Ok(Outcome::Clipboard);
    }
    let previous = clipboard_text();
    set_clipboard(text)?;
    std::thread::sleep(Duration::from_millis(40));
    match send_paste_keys() {
        Ok(()) => {
            // Aplikacja czyta schowek dopiero po obsłużeniu skrótu — dajemy jej chwilę.
            std::thread::sleep(Duration::from_millis(250));
            if let Some(prev) = previous {
                if clipboard_text().as_deref() == Some(text) {
                    let _ = set_clipboard(&prev);
                }
            }
            Ok(Outcome::Pasted)
        }
        Err(e) => {
            log::warn!("Wklejanie nie powiodło się, tekst został w schowku: {e}");
            Ok(Outcome::Clipboard)
        }
    }
}

fn send_paste_keys() -> Result<()> {
    if is_wayland() {
        return wayland_paste();
    }
    enigo_paste()
}

fn enigo_paste() -> Result<()> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| anyhow!("enigo: {e}"))?;
    #[cfg(target_os = "macos")]
    let (modifier, v) = (Key::Meta, Key::Other(9)); // kVK_ANSI_V — niezależnie od układu QWERTY/QWERTZ
    #[cfg(not(target_os = "macos"))]
    let (modifier, v) = (Key::Control, Key::Unicode('v'));
    enigo.key(modifier, Direction::Press).map_err(|e| anyhow!("{e}"))?;
    let r = enigo.key(v, Direction::Click).map_err(|e| anyhow!("{e}"));
    enigo.key(modifier, Direction::Release).map_err(|e| anyhow!("{e}"))?;
    r
}

fn run(cmd: &str, args: &[&str], stdin: Option<&str>) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("{cmd}: {e}"))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(input.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!("{cmd}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// wtype działa na wlroots (Sway, Hyprland…), nie na GNOME/KDE; dotool i ydotool wymagają
/// dostępu do /dev/uinput. Każde po kolei, pierwsze udane wygrywa.
fn wayland_paste() -> Result<()> {
    let attempts: [(&str, &[&str], Option<&str>); 3] = [
        ("wtype", &["-M", "ctrl", "-k", "v", "-m", "ctrl"], None),
        ("dotool", &[], Some("key ctrl+v\n")),
        // 29 = KEY_LEFTCTRL, 47 = KEY_V (kody evdev, niezależne od układu)
        ("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"], None),
    ];
    let mut errors = Vec::new();
    for (cmd, args, stdin) in attempts {
        match run(cmd, args, stdin) {
            Ok(()) => return Ok(()),
            Err(e) => errors.push(e.to_string()),
        }
    }
    Err(anyhow!("Wayland: brak działającego narzędzia ({})", errors.join("; ")))
}
