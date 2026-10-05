//! Dymek ze stanem dyktowania: małe, przezroczyste okno zawsze na wierzchu, które nie
//! przejmuje fokusu (inaczej tekst wkleiłby się do niego, a nie do aplikacji użytkownika).
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

use crate::dictation::HudState;

pub const LABEL: &str = "hud";
const WIDTH: f64 = 260.0;
const HEIGHT: f64 = 64.0;

/// Numer kolejnego stanu — opóźnione chowanie nie może schować dymka nowego nagrania.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html#hud".into()))
        .title("Dyktando X")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .focusable(false)
        .focused(false)
        .visible(false)
        .build()?;
    let _ = window.set_ignore_cursor_events(true);
    Ok(())
}

fn place(app: &AppHandle) {
    let Some(w) = app.get_webview_window(LABEL) else { return };
    let monitor = w.current_monitor().ok().flatten().or_else(|| w.primary_monitor().ok().flatten());
    if let Some(m) = monitor {
        let scale = m.scale_factor();
        let size = m.size();
        let pos = m.position();
        let x = pos.x + ((size.width as f64 - WIDTH * scale) / 2.0) as i32;
        let y = pos.y + (size.height as f64 - (HEIGHT + 110.0) * scale) as i32;
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
}

pub fn update(app: &AppHandle, state: &HudState) {
    let enabled = app
        .try_state::<crate::AppState>()
        .map(|s| s.settings.lock().unwrap().hud_enabled)
        .unwrap_or(true);
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(w) = app.get_webview_window(LABEL) else { return };
    let hide_after = match state {
        HudState::Idle => Some(Duration::ZERO),
        HudState::Done { .. } => Some(Duration::from_millis(1400)),
        HudState::Error { .. } => Some(Duration::from_secs(5)),
        HudState::Info { .. } => Some(Duration::from_secs(4)),
        HudState::Recording { .. } | HudState::Transcribing => None,
    };
    // Błędy pokazujemy zawsze — inaczej dyktowanie „nic nie robi” bez wyjaśnienia.
    let visible = enabled || matches!(state, HudState::Error { .. });
    match hide_after {
        Some(d) if d.is_zero() => {
            let _ = w.hide();
        }
        _ if !visible => {
            let _ = w.hide();
        }
        Some(d) => {
            show(app);
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(d);
                if GENERATION.load(Ordering::SeqCst) == generation {
                    if let Some(w) = app.get_webview_window(LABEL) {
                        let _ = w.hide();
                    }
                }
            });
        }
        None => show(app),
    }
}

fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        if !w.is_visible().unwrap_or(false) {
            place(app);
            let _ = w.show();
        }
    }
}
