//! Logi: konsola i plik `paths::logs()/dyktando-x.log` — aplikacja okienkowa nie ma konsoli,
//! a po problemie z nagraniem (przerwy w dźwięku, restart przechwytywania) trzeba mieć co przejrzeć.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::paths;

const FILTER: &str = "info,ort=warn,whisper_rs=warn,transcribe_rs=warn";
/// Większy plik przy starcie przechodzi do `dyktando-x.1.log` (poprzedni znika).
const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn file() -> PathBuf {
    paths::logs().join("dyktando-x.log")
}

pub fn init() {
    let path = file();
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(FILTER));
    let opened = open(&path);
    if let Some(f) = opened.as_ref().ok().and_then(|f| f.try_clone().ok()) {
        builder.target(env_logger::Target::Pipe(Box::new(Tee(f)))).write_style(env_logger::WriteStyle::Never);
    }
    let _ = builder.try_init();
    match opened {
        Ok(_) => log::info!("Dyktando X {} ({}), log: {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, path.display()),
        Err(e) => log::warn!("Log tylko na konsoli — {}: {e}", path.display()),
    }
}

fn open(path: &Path) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("1.log"));
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Każdy wpis na stderr (podgląd w `tauri dev`) i do pliku.
struct Tee(File);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        self.0.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        self.0.flush()
    }
}

/// Pokazuje plik logu w Finderze / Eksploratorze / menedżerze plików.
#[tauri::command]
pub fn reveal_logs(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let path = file();
    let target = if path.exists() { path } else { paths::logs() };
    app.opener().reveal_item_in_dir(target).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_a_big_log_and_appends_otherwise() {
        let dir = std::env::temp_dir().join(format!("dx-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dyktando-x.log");
        std::fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        let mut f = open(&path).unwrap();
        f.write_all(b"nowy\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"nowy\n");
        assert_eq!(std::fs::metadata(dir.join("dyktando-x.1.log")).unwrap().len(), MAX_BYTES + 1);
        drop(f);
        open(&path).unwrap().write_all(b"dalej\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"nowy\ndalej\n");
        std::fs::remove_dir_all(dir).ok();
    }
}
