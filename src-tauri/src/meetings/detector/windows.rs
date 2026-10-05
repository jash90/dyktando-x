//! Windows: rejestr „CapabilityAccessManager\ConsentStore\microphone” — aplikacja, która teraz
//! używa mikrofonu, ma `LastUsedTimeStop = 0`. Klasyczne programy są w `NonPackaged` pod ścieżką
//! .exe (z `#` zamiast `\`), aplikacje ze sklepu pod nazwą rodziny pakietu.
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const BASE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

fn in_use(key: &RegKey) -> bool {
    let start: u64 = key.get_value("LastUsedTimeStart").unwrap_or(0);
    let stop: u64 = key.get_value("LastUsedTimeStop").unwrap_or(1);
    start > 0 && stop == 0
}

pub fn processes_using_microphone() -> Vec<String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(root) = hkcu.open_subkey(BASE) else { return Vec::new() };
    let own = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    let mut out = Vec::new();
    for name in root.enum_keys().flatten() {
        if name == "NonPackaged" {
            if let Ok(np) = root.open_subkey(&name) {
                for exe in np.enum_keys().flatten() {
                    if np.open_subkey(&exe).map(|k| in_use(&k)).unwrap_or(false) {
                        let file = exe.rsplit('#').next().unwrap_or(&exe).to_lowercase();
                        if Some(&file) != own.as_ref() {
                            out.push(file);
                        }
                    }
                }
            }
        } else if root.open_subkey(&name).map(|k| in_use(&k)).unwrap_or(false) {
            out.push(name.to_lowercase());
        }
    }
    out
}
