//! Menu bar / tray icon: meeting recording (with a timer), windows, quit.
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

use crate::i18n::{t, t_with};
use crate::meetings::transcript::clock;
use crate::AppState;

pub struct TrayItems {
    pub meeting: MenuItem<Wry>,
    pub status: MenuItem<Wry>,
    pub update: MenuItem<Wry>,
    meetings: MenuItem<Wry>,
    settings: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<TrayItems> {
    let meeting = MenuItem::with_id(app, "meeting", t("tray.record_meeting"), true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "", false, None::<&str>)?;
    let meetings = MenuItem::with_id(app, "meetings", t("tray.meetings"), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", t("tray.settings"), true, None::<&str>)?;
    let update = MenuItem::with_id(app, "update", t("tray.check_updates"), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", t("tray.quit"), true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(
        app,
        &[&meeting, &status, &PredefinedMenuItem::separator(app)?, &meetings, &settings, &update, &PredefinedMenuItem::separator(app)?, &quit],
    )?;
    // macOS: monochrome template (the system picks the color for a light/dark menu bar itself).
    #[cfg(target_os = "macos")]
    let (icon, template) = (tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?, true);
    #[cfg(not(target_os = "macos"))]
    let (icon, template) = (app.default_window_icon().cloned().expect("app icon"), false);
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(template)
        .tooltip("Dyktando X")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "meeting" => crate::commands::toggle_meeting_from(app),
            "meetings" => crate::show_window(app, crate::Window::Meetings),
            "settings" => crate::show_window(app, crate::Window::Settings),
            "update" => crate::updater::open_from_tray(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(TrayItems { meeting, status, update, meetings, settings, quit })
}

/// After a UI language change: the fixed labels (the changing ones are set by `refresh`).
pub fn relabel(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let tray = state.tray.lock().unwrap();
    let Some(items) = tray.as_ref() else { return };
    let _ = items.meetings.set_text(t("tray.meetings"));
    let _ = items.settings.set_text(t("tray.settings"));
    let _ = items.quit.set_text(t("tray.quit"));
}

/// Refreshes the menu labels and the timer by the icon (macOS: title next to the icon).
pub fn refresh(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let rec = state.recorder.status();
    let job = state.jobs.current();
    let Some(items) = state.tray.lock().unwrap().as_ref().map(|t| (t.meeting.clone(), t.status.clone(), t.update.clone())) else { return };
    let (meeting, status, update) = items;
    let _ = update.set_text(match app.state::<crate::updater::Updates>().available_version() {
        Some(v) => t_with("tray.install_update", &[("version", &v)]),
        None => t("tray.check_updates"),
    });
    let _ = meeting.set_text(if rec.recording { t_with("tray.stop_recording", &[("time", &clock(rec.seconds))]) } else { t("tray.record_meeting") });
    let status_text = match (&rec.warning, &job) {
        (Some(w), _) if rec.recording => w.clone(),
        (_, Some(j)) => format!("{} — {:.0}%", j.step, j.fraction * 100.0),
        _ => t("tray.ready"),
    };
    let _ = status.set_text(&status_text);
    if let Some(tray) = app.tray_by_id("main") {
        let title = if rec.recording { Some(format!("● {}", clock(rec.seconds))) } else { None };
        #[cfg(target_os = "macos")]
        let _ = tray.set_title(title.as_deref());
        let _ = tray.set_tooltip(Some(title.map(|time| t_with("tray.tooltip_recording", &[("time", &time)])).unwrap_or_else(|| "Dyktando X".into())));
    }
}
