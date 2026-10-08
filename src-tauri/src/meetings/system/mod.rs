//! Dźwięk systemowy (to, co słychać z głośników: rozmówcy w Meet/Zoom/Teams), bez dźwięku
//! samego Dyktando X:
//! - macOS 14.4+: Core Audio process tap (uprawnienie „Nagrywanie dźwięku systemowego”),
//! - Windows 10 20348+/11: WASAPI process loopback z wykluczeniem własnego procesu,
//! - Linux: monitor domyślnego wyjścia PulseAudio/PipeWire (`parec`).
use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Odbiorca próbek mono f32 w częstotliwości `SystemCapture::sample_rate`.
pub type Sink = Box<dyn FnMut(&[f32]) + Send + 'static>;

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

pub struct SystemCapture {
    inner: platform::Capture,
    pub sample_rate: u32,
    buffers: Arc<AtomicU64>,
}

impl SystemCapture {
    pub fn start(mut sink: Sink) -> Result<Self> {
        let buffers = Arc::new(AtomicU64::new(0));
        let counter = buffers.clone();
        let counting: Sink = Box::new(move |s: &[f32]| {
            counter.fetch_add(1, Ordering::Relaxed);
            sink(s)
        });
        let (inner, sample_rate) = platform::Capture::start(counting)?;
        Ok(Self { inner, sample_rate, buffers })
    }

    /// Ile buforów przyszło. 0 po kilku sekundach = przechwytywanie „martwe” (macOS: tap
    /// utworzony przed zgodą użytkownika) — trzeba je utworzyć od nowa.
    pub fn buffers_received(&self) -> u64 {
        self.buffers.load(Ordering::Relaxed)
    }

    pub fn stop(self) {
        self.inner.stop();
    }
}

/// Czy system w ogóle obsługuje nagrywanie dźwięku aplikacji (np. macOS < 14.4 — nie).
pub fn availability() -> Result<(), String> {
    platform::availability()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Diagnostyka na prawdziwym sprzęcie: zgłoszona częstotliwość musi się zgadzać z liczbą
    /// próbek, które naprawdę przychodzą (inaczej nagranie rozmówców jest pocięte ciszą).
    /// `cargo test --lib -- --ignored system_rate --nocapture`.
    #[test]
    #[ignore]
    fn system_rate_matches_delivered_samples() {
        let frames = Arc::new(AtomicU64::new(0));
        let f = frames.clone();
        let capture = SystemCapture::start(Box::new(move |s: &[f32]| {
            f.fetch_add(s.len() as u64, Ordering::Relaxed);
        }))
        .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let (t, before) = (std::time::Instant::now(), frames.load(Ordering::Relaxed));
        std::thread::sleep(std::time::Duration::from_secs(4));
        let measured = (frames.load(Ordering::Relaxed) - before) as f64 / t.elapsed().as_secs_f64();
        let declared = capture.sample_rate;
        capture.stop();
        println!("zgłoszone {declared} Hz, zmierzone {measured:.0} Hz");
        assert!(measured > 0.0, "brak buforów (zgoda na nagrywanie dźwięku systemowego?)");
        assert!((measured / declared as f64 - 1.0).abs() < 0.05, "zgłoszone {declared} Hz, zmierzone {measured:.0} Hz");
    }
}
