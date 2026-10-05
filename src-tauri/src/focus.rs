//! Czy wklejać: pytamy system, co ma fokus. Tylko gdy na pewno NIE jest to pole tekstowe
//! (przycisk, lista plików, brak fokusu) zostawiamy tekst w schowku. Niepewność → wklejamy,
//! jak w wersji Swift. macOS: Accessibility, Windows: UI Automation, Linux (AT-SPI bywa
//! zawodne) → zawsze „nie wiem”.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Focus {
    Editable,
    NotEditable,
    Unknown,
}

/// Opis elementu z fokusem niezależny od systemu — testowalny bez API dostępności.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Rola AX (macOS, np. `AXTextField`) albo typ kontrolki UIA (Windows, np. `Edit`).
    pub role: Option<String>,
    pub value_settable: bool,
    pub has_text_range: bool,
    /// macOS `AXEditableAncestor` (contenteditable w przeglądarce / Electronie).
    pub editable_ancestor: bool,
    /// Bundle ID (macOS) albo nazwa pliku .exe (Windows).
    pub app: Option<String>,
    /// Aplikacja odpowiedziała, że nic nie ma fokusu.
    pub nothing_focused: bool,
    /// Nie udało się zapytać (brak uprawnień, limit czasu, aplikacja bez dostępności).
    pub failed: bool,
}

const TEXT_ROLES: &[&str] = &["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField", "Edit", "ComboBox"];
const NON_TEXT_ROLES: &[&str] = &[
    // macOS
    "AXButton", "AXCheckBox", "AXRadioButton", "AXList", "AXOutline", "AXTable", "AXRow", "AXCell", "AXImage",
    "AXMenuItem", "AXMenuButton", "AXMenuBar", "AXLink", "AXStaticText", "AXTabGroup", "AXSlider", "AXPopUpButton",
    "AXWindow", "AXBrowser", "AXToolbar", "AXDisclosureTriangle", "AXIncrementor", "AXColorWell", "AXSheet", "AXDrawer",
    // Windows (UIA)
    "Button", "CheckBox", "RadioButton", "List", "ListItem", "Tree", "TreeItem", "Menu", "MenuItem", "MenuBar",
    "Hyperlink", "Image", "Tab", "TabItem", "Slider", "ToolBar", "Text", "SplitButton", "Header", "HeaderItem", "Table",
    "DataItem", "TitleBar", "ScrollBar", "Spinner", "ProgressBar",
];

pub fn classify(s: &Snapshot) -> Focus {
    if s.nothing_focused {
        return Focus::NotEditable;
    }
    if s.failed {
        return Focus::Unknown;
    }
    if s.role.as_deref().is_some_and(|r| TEXT_ROLES.contains(&r)) || s.editable_ancestor {
        return Focus::Editable;
    }
    // Własne edytory (Word, edytory kodu): wartość do zmiany + zaznaczenie tekstu.
    if s.value_settable && s.has_text_range {
        return Focus::Editable;
    }
    if s.role.as_deref().is_some_and(|r| NON_TEXT_ROLES.contains(&r)) {
        return Focus::NotEditable;
    }
    // Menedżer plików: Ctrl/⌘V z tekstem w schowku nic sensownego nie zrobi poza polami nazw.
    if s.app.as_deref().is_some_and(|a| a == "com.apple.finder" || a.eq_ignore_ascii_case("explorer.exe")) {
        return Focus::NotEditable;
    }
    Focus::Unknown // AXGroup, AXWebArea, Document, Pane… — zbyt ogólne, nie blokujemy wklejania
}

pub fn snapshot() -> Snapshot {
    platform::snapshot()
}

pub fn should_paste() -> bool {
    let s = snapshot();
    let f = classify(&s);
    log::info!("Fokus: {f:?} ({:?} w {:?})", s.role, s.app);
    f != Focus::NotEditable
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub fn snapshot() -> super::Snapshot {
        super::Snapshot { failed: true, ..Default::default() }
    }
}

#[cfg(target_os = "macos")]
#[path = "focus_macos.rs"]
mod platform;

#[cfg(target_os = "windows")]
#[path = "focus_windows.rs"]
mod platform;

#[cfg(test)]
mod tests {
    use super::*;

    fn s(role: &str) -> Snapshot {
        Snapshot { role: Some(role.into()), ..Default::default() }
    }

    #[test]
    fn text_fields_are_editable() {
        assert_eq!(classify(&s("AXTextArea")), Focus::Editable);
        assert_eq!(classify(&s("Edit")), Focus::Editable);
        assert_eq!(classify(&Snapshot { role: Some("AXGroup".into()), editable_ancestor: true, ..Default::default() }), Focus::Editable);
        assert_eq!(classify(&Snapshot { role: Some("Document".into()), value_settable: true, has_text_range: true, ..Default::default() }), Focus::Editable);
    }

    #[test]
    fn controls_and_file_managers_are_not() {
        assert_eq!(classify(&s("AXButton")), Focus::NotEditable);
        assert_eq!(classify(&s("ListItem")), Focus::NotEditable);
        assert_eq!(classify(&Snapshot { role: Some("Pane".into()), app: Some("explorer.exe".into()), ..Default::default() }), Focus::NotEditable);
        assert_eq!(classify(&Snapshot { nothing_focused: true, ..Default::default() }), Focus::NotEditable);
    }

    #[test]
    fn uncertainty_still_pastes() {
        assert_eq!(classify(&s("AXWebArea")), Focus::Unknown);
        assert_eq!(classify(&s("Document")), Focus::Unknown);
        assert_eq!(classify(&Snapshot { failed: true, ..Default::default() }), Focus::Unknown);
    }
}

#[cfg(test)]
mod live {
    /// `cargo test -- --ignored focus_live --nocapture` (wymaga Dostępności dla terminala).
    #[test]
    #[ignore]
    fn focus_live() {
        let s = super::snapshot();
        println!("{s:?} → {:?}", super::classify(&s));
    }
}
