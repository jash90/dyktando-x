//! Microphone capture via cpal (CoreAudio / WASAPI / ALSA-PulseAudio-PipeWire).
//! The cpal stream is not `Send` on every platform, so it lives on its own thread;
//! `InputCapture` only stops it.
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use super::resample::downmix;

/// Receiver of mono samples at the device's native rate (called from the audio thread — be quick!).
pub type Sink = Box<dyn FnMut(&[f32]) + Send + 'static>;

pub fn input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .input_devices()
        .map(|it| it.map(|d| d.to_string()).collect())
        .unwrap_or_default();
    names.dedup();
    names
}

pub fn default_input_name() -> Option<String> {
    cpal::default_host().default_input_device().map(|d| d.to_string())
}

fn find_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(d) = devices.find(|d| d.to_string() == name) {
                return Ok(d);
            }
        }
        log::warn!("Mikrofon „{name}” niedostępny — używam domyślnego");
    }
    host.default_input_device().ok_or_else(|| anyhow!("Brak mikrofonu w systemie"))
}

pub struct InputCapture {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    pub sample_rate: u32,
    pub device_name: String,
    /// The stream reported an error (e.g. device unplugged) — nothing more will arrive.
    failed: Arc<AtomicBool>,
}

impl InputCapture {
    pub fn start(device: Option<&str>, sink: Sink) -> Result<Self> {
        let device = device.map(str::to_string);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, String)>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let failed = Arc::new(AtomicBool::new(false));
        let on_error = failed.clone();
        let thread = std::thread::Builder::new()
            .name("mic-capture".into())
            .spawn(move || {
                let started = (|| -> Result<(cpal::Stream, u32, String)> {
                    let dev = find_device(device.as_deref())?;
                    let name = dev.to_string();
                    let cfg = dev.default_input_config().context("konfiguracja mikrofonu")?;
                    let stream = build(&dev, cfg.sample_format(), cfg.config(), sink, on_error)?;
                    stream.play().context("start mikrofonu")?;
                    Ok((stream, cfg.sample_rate(), name))
                })();
                match started {
                    Ok((stream, rate, name)) => {
                        let _ = ready_tx.send(Ok((rate, name)));
                        let _ = stop_rx.recv();
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })?;
        let (sample_rate, device_name) = ready_rx
            .recv()
            .map_err(|_| anyhow!("wątek mikrofonu zakończył się"))??;
        Ok(Self { stop: Some(stop_tx), thread: Some(thread), sample_rate, device_name, failed })
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    fn shutdown(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for InputCapture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn build(dev: &cpal::Device, format: SampleFormat, config: cpal::StreamConfig, sink: Sink, failed: Arc<AtomicBool>) -> Result<cpal::Stream> {
    match format {
        SampleFormat::F32 => typed::<f32>(dev, config, sink, failed),
        SampleFormat::I16 => typed::<i16>(dev, config, sink, failed),
        SampleFormat::I32 => typed::<i32>(dev, config, sink, failed),
        SampleFormat::U16 => typed::<u16>(dev, config, sink, failed),
        SampleFormat::I8 => typed::<i8>(dev, config, sink, failed),
        SampleFormat::U8 => typed::<u8>(dev, config, sink, failed),
        SampleFormat::F64 => typed::<f64>(dev, config, sink, failed),
        other => Err(anyhow!("Nieobsługiwany format próbek mikrofonu: {other:?}")),
    }
}

fn typed<T>(dev: &cpal::Device, config: cpal::StreamConfig, mut sink: Sink, failed: Arc<AtomicBool>) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let mut scratch: Vec<f32> = Vec::new();
    let stream = dev
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                scratch.clear();
                scratch.extend(data.iter().map(|s| s.to_sample::<f32>()));
                if channels > 1 {
                    sink(&downmix(&scratch, channels));
                } else {
                    sink(&scratch);
                }
            },
            move |e| {
                log::error!("strumień mikrofonu: {e}");
                failed.store(true, Ordering::Relaxed);
            },
            None,
        )
        .context("otwieranie mikrofonu")?;
    Ok(stream)
}
