//! System audio (what comes out of the speakers: the other participants in Meet/Zoom/Teams),
//! without the sound of Dyktando X itself:
//! - macOS 14.4+: Core Audio process tap ("System Audio Recording" permission),
//! - Windows 10 20348+/11: WASAPI process loopback excluding our own process,
//! - Linux: monitor of the default PulseAudio/PipeWire output (`parec`).
use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Receiver of mono f32 samples at `SystemCapture::sample_rate`.
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

    /// How many buffers have arrived. 0 after a few seconds = "dead" capture (macOS: tap
    /// created before the user granted permission) — it must be recreated.
    pub fn buffers_received(&self) -> u64 {
        self.buffers.load(Ordering::Relaxed)
    }

    /// The device the capture is attached to has changed — it must be recreated.
    pub fn device_changed(&self) -> bool {
        self.inner.device_changed()
    }

    pub fn stop(self) {
        self.inner.stop();
    }
}

/// Whether the capture delivers buffers during silence too. WASAPI loopback sends nothing in
/// silence, so there a lack of buffers doesn't mean the audio was cut off.
pub const DELIVERS_IN_SILENCE: bool = cfg!(not(target_os = "windows"));

/// Whether the system supports recording app audio at all (e.g. macOS < 14.4 — no).
pub fn availability() -> Result<(), String> {
    platform::availability()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Diagnostics on real hardware: the reported sample rate must match the number of samples
    /// that actually arrive (otherwise the participants' recording is chopped up by silence).
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
