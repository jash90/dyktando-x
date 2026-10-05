//! Linux: monitor domyślnego wyjścia przez `parec` (pulseaudio-utils; działa też z PipeWire
//! przez pipewire-pulse). Nagrywa całe wyjście — także dźwięki samego Dyktando X, których
//! aplikacja zresztą nie odtwarza.
use anyhow::{anyhow, Context, Result};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;

use super::Sink;

const RATE: u32 = 16_000;

pub fn availability() -> Result<(), String> {
    match Command::new("parec").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status() {
        Ok(s) if s.success() => Ok(()),
        _ => Err("Brak programu parec — zainstaluj pakiet pulseaudio-utils (Ubuntu/Debian) albo pulseaudio-utils/pipewire-pulseaudio (Fedora). Bez niego nagrywany będzie tylko mikrofon.".into()),
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
                "--stream-name=Nagrywanie spotkania",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("uruchamianie parec")?;
        let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("parec bez wyjścia"))?;
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
        log::info!("parec: monitor domyślnego wyjścia, {RATE} Hz");
        Ok((Self { child: Some(child), thread: Some(thread) }, RATE))
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
