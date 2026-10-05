//! Windows: element z fokusem przez UI Automation.
use uiautomation::patterns::{UITextPattern, UIValuePattern};
use uiautomation::UIAutomation;

pub fn snapshot() -> super::Snapshot {
    let mut snap = super::Snapshot::default();
    let Ok(automation) = UIAutomation::new() else {
        snap.failed = true;
        return snap;
    };
    let Ok(el) = automation.get_focused_element() else {
        snap.nothing_focused = true;
        return snap;
    };
    snap.role = el.get_control_type().ok().map(|t| format!("{t:?}"));
    if let Ok(v) = el.get_pattern::<UIValuePattern>() {
        snap.value_settable = v.is_readonly().map(|r| !r).unwrap_or(false);
    }
    snap.has_text_range = el.get_pattern::<UITextPattern>().is_ok();
    // Lista plików Eksploratora (klasy okien powłoki) — wklejanie tekstu nie ma tam sensu.
    if let Ok(class) = el.get_classname() {
        if matches!(class.as_str(), "DirectUIHWND" | "SysListView32" | "UIItemsView") {
            snap.app = Some("explorer.exe".into());
        }
    }
    snap
}
