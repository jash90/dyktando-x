//! Podpowiedź „Wykryto spotkanie w Zoom — nagrać?”: co 5 s sprawdzamy, które aplikacje do rozmów
//! używają mikrofonu. Nic nie nagrywa się samo — użytkownik klika „Nagraj”.
use std::collections::BTreeSet;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

/// (identyfikator, nazwa do wyświetlenia). macOS: bundle ID (dopasowanie po prefiksie, bo
/// przeglądarki używają mikrofonu z procesów pomocniczych `….helper`), Windows: nazwa pliku .exe
/// albo rodzina pakietu, Linux: nazwa programu z PulseAudio/PipeWire.
pub const MEETING_APPS: &[(&str, &str)] = &[
    // macOS
    ("us.zoom.xos", "Zoom"),
    ("com.microsoft.teams2", "Microsoft Teams"),
    ("com.microsoft.teams", "Microsoft Teams"),
    ("com.cisco.webexmeetingsapp", "Webex"),
    ("Cisco-Systems.Spark", "Webex"),
    ("com.tinyspeck.slackmacgap", "Slack"),
    ("com.apple.FaceTime", "FaceTime"),
    ("com.hnc.Discord", "Discord"),
    ("com.google.Chrome", "Chrome"),
    ("com.apple.Safari", "Safari"),
    ("com.apple.WebKit", "Safari"),
    ("company.thebrowser.Browser", "Arc"),
    ("org.mozilla.firefox", "Firefox"),
    ("com.microsoft.edgemac", "Edge"),
    ("com.brave.Browser", "Brave"),
    ("com.operasoftware.Opera", "Opera"),
    ("com.vivaldi.Vivaldi", "Vivaldi"),
    // Windows
    ("zoom.exe", "Zoom"),
    ("ms-teams.exe", "Microsoft Teams"),
    ("teams.exe", "Microsoft Teams"),
    ("msteams_8wekyb3d8bbwe", "Microsoft Teams"),
    ("webex.exe", "Webex"),
    ("ciscocollabhost.exe", "Webex"),
    ("slack.exe", "Slack"),
    ("discord.exe", "Discord"),
    ("chrome.exe", "Chrome"),
    ("msedge.exe", "Edge"),
    ("firefox.exe", "Firefox"),
    ("brave.exe", "Brave"),
    ("opera.exe", "Opera"),
    ("vivaldi.exe", "Vivaldi"),
    ("arc.exe", "Arc"),
    // Linux
    ("zoom", "Zoom"),
    ("teams-for-linux", "Microsoft Teams"),
    ("slack", "Slack"),
    ("discord", "Discord"),
    ("chrome", "Chrome"),
    ("google-chrome", "Chrome"),
    ("chromium", "Chromium"),
    ("chromium-browser", "Chromium"),
    ("firefox", "Firefox"),
    ("firefox-esr", "Firefox"),
    ("brave", "Brave"),
    ("msedge", "Edge"),
    ("vivaldi-bin", "Vivaldi"),
    ("opera", "Opera"),
    ("webex", "Webex"),
];

/// Nazwa aplikacji do rozmów dla identyfikatora procesu albo `None`.
pub fn matching_app(id: &str) -> Option<&'static str> {
    let lower = id.to_lowercase();
    MEETING_APPS.iter().find_map(|(app, name)| {
        let app_l = app.to_lowercase();
        (lower == app_l || lower.starts_with(&format!("{app_l}."))).then_some(*name)
    })
}

/// Czysta logika: o którą aplikację zapytać (raz na „sesję” mikrofonu danej aplikacji).
#[derive(Default)]
pub struct DetectionLogic {
    handled: BTreeSet<&'static str>,
}

impl DetectionLogic {
    pub fn update(&mut self, active_ids: &[String], is_recording: bool, enabled: bool) -> Option<&'static str> {
        let active: BTreeSet<&'static str> = active_ids.iter().filter_map(|id| matching_app(id)).collect();
        // Aplikacja przestała używać mikrofonu → następna rozmowa znów zapyta.
        self.handled.retain(|a| active.contains(a));
        if !enabled || is_recording {
            // Nie pytaj o rozmowę, która już trwa w chwili startu/końca nagrywania.
            self.handled.extend(active);
            return None;
        }
        let app = active.difference(&self.handled).next().copied()?;
        self.handled.insert(app);
        Some(app)
    }
}

/// Identyfikatory procesów używających teraz mikrofonu (bez Dyktando X).
pub fn processes_using_microphone() -> Vec<String> {
    platform::processes_using_microphone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn matches_helpers_and_exe_names() {
        assert_eq!(matching_app("com.google.Chrome.helper"), Some("Chrome"));
        assert_eq!(matching_app("us.zoom.xos"), Some("Zoom"));
        assert_eq!(matching_app("Zoom.exe"), Some("Zoom"));
        assert_eq!(matching_app("MSTeams_8wekyb3d8bbwe"), Some("Microsoft Teams"));
        assert_eq!(matching_app("com.apple.WebKit.GPU"), Some("Safari"));
        assert_eq!(matching_app("com.spotify.client"), None);
        assert_eq!(matching_app("chromeX"), None);
    }

    #[test]
    fn asks_once_per_microphone_session() {
        let mut l = DetectionLogic::default();
        assert_eq!(l.update(&ids(&["us.zoom.xos"]), false, true), Some("Zoom"));
        assert_eq!(l.update(&ids(&["us.zoom.xos"]), false, true), None);
        assert_eq!(l.update(&[], false, true), None);
        assert_eq!(l.update(&ids(&["us.zoom.xos"]), false, true), Some("Zoom"), "nowa rozmowa");
    }

    #[test]
    fn does_not_ask_while_recording_or_disabled() {
        let mut l = DetectionLogic::default();
        assert_eq!(l.update(&ids(&["zoom.exe"]), true, true), None);
        assert_eq!(l.update(&ids(&["zoom.exe"]), false, true), None, "rozmowa trwała podczas nagrywania");
        let mut l = DetectionLogic::default();
        assert_eq!(l.update(&ids(&["firefox"]), false, false), None);
    }
}

#[cfg(test)]
mod live {
    /// Odczyt z prawdziwego systemu: `cargo test -- --ignored detector_live --nocapture`.
    #[test]
    #[ignore]
    fn detector_live() {
        println!("mikrofon używają: {:?}", super::processes_using_microphone());
    }
}
