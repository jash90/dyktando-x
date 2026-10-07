//! Okno „na żywo” przy nagrywaniu spotkania: małe, zawsze na wierzchu, w prawym dolnym rogu.
//! Pokazuje licznik, poziomy obu ścieżek i ostatnie przepisane wypowiedzi — żeby od razu
//! było widać, że nagranie i transkrypcja działają. Powstaje przy starcie nagrania, znika
//! przy jego zatrzymaniu; użytkownik może je zamknąć (schować) wcześniej.
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "live";
const WIDTH: f64 = 460.0;
const HEIGHT: f64 = 280.0;

pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.show();
        return;
    }
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html#live".into()))
        .title("Dyktando X — na żywo")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(320.0, 160.0)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .resizable(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .focused(false)
        .visible(false)
        .build();
    let w = match built {
        Ok(w) => w,
        Err(e) => {
            log::error!("okno na żywo: {e}");
            return;
        }
    };
    if let Ok(Some(m)) = w.primary_monitor() {
        let scale = m.scale_factor();
        let x = m.position().x + m.size().width as i32 - ((WIDTH + 20.0) * scale) as i32;
        let y = m.position().y + m.size().height as i32 - ((HEIGHT + 90.0) * scale) as i32;
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
    let _ = w.show();
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}
