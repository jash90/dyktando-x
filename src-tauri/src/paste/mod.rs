//! Inserting text into the active window: text to the clipboard → simulated paste →
//! restoring the previous clipboard contents (text only — arboard can't restore images).
//!
//! Pasting: macOS ⌘V (CGEvent via enigo, requires Accessibility), Windows Ctrl+V (SendInput),
//! Linux X11 Ctrl+V (XTest via enigo), Linux Wayland: wtype → dotool → ydotool, and when
//! none of them works — clipboard only.
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Pasted into the active window.
    Pasted,
    /// Clipboard only (no permissions, no tool, user's choice).
    Clipboard,
}

/// The clipboard must live for the whole app lifetime: on X11/Wayland it's us who "serve"
/// the contents to other programs — once the object is dropped, the pasted text would vanish.
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

/// Whether the system lets us simulate keys (macOS: Accessibility; elsewhere always yes).
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

/// Inserts text. `paste = false` → clipboard only (no restoring — the user pastes manually).
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
            // The app reads the clipboard only after handling the shortcut — we give it a moment.
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
    let (modifier, v) = (Key::Meta, Key::Other(9)); // kVK_ANSI_V — regardless of QWERTY/QWERTZ layout
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

/// wtype works on wlroots (Sway, Hyprland…), not on GNOME/KDE; dotool and ydotool require
/// access to /dev/uinput. Each in turn; the first that succeeds wins.
fn wayland_paste() -> Result<()> {
    let attempts: [(&str, &[&str], Option<&str>); 3] = [
        ("wtype", &["-M", "ctrl", "-k", "v", "-m", "ctrl"], None),
        ("dotool", &[], Some("key ctrl+v\n")),
        // 29 = KEY_LEFTCTRL, 47 = KEY_V (evdev codes, layout-independent)
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
