//! Linux: monitor of the default output via `parec` (pulseaudio-utils; also works with PipeWire
//! via pipewire-pulse). Records the whole output — including sounds from Dyktando X itself,
//! which the app doesn't play anyway.
use anyhow::{anyhow, Context, Result};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;

use super::Sink;

const RATE: u32 = 16_000;

pub fn availability() -> Result<(), String> {
    match Command::new("parec").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status() {
        Ok(s) if s.success() => Ok(()),
        _ => Err(crate::i18n::t("system.linux_no_parec")),
    }
}

pub struct Capture {
    child: Option<Child>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    pub fn start(mut sink: Sink) -> Result<(Self, u32)> {
        availability().map_err(|e| anyhow!(e))?;
        let mut child = Command::new("parec")
            .args([
                "--device=@DEFAULT_MONITOR@",
                "--format=float32le",
                &format!("--rate={RATE}"),
                "--channels=1",
                "--raw",
                "--latency-msec=100",
                "--client-name=Dyktando X",
                &format!("--stream-name={}", crate::i18n::t("system.linux_stream_name")),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| crate::i18n::t("system.parec_starting"))?;
        let mut stdout = child.stdout.take().ok_or_else(|| anyhow!(crate::i18n::t("system.parec_no_output")))?;
        let thread = std::thread::Builder::new().name("system-audio".into()).spawn(move || {
            let mut buf = vec![0u8; 3200 * 4];
            let mut carry: Vec<u8> = Vec::new();
            let mut samples: Vec<f32> = Vec::new();
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        carry.extend_from_slice(&buf[..n]);
                        let whole = carry.len() / 4 * 4;
                        samples.clear();
                        samples.extend(carry[..whole].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
                        carry.drain(..whole);
                        if !samples.is_empty() {
                            sink(&samples);
                        }
                    }
                }
            }
        })?;
        log::info!("parec: monitor of the default output, {RATE} Hz");
        Ok((Self { child: Some(child), thread: Some(thread) }, RATE))
    }

    /// `parec` stays on the monitor chosen at startup; when it closes (the device disappeared),
    /// deliveries stop and the recording supervisor recreates the capture.
    pub fn device_changed(&self) -> bool {
        false
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shutdown();
    }
}
