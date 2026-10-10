//! WASAPI process loopback (Windows 10 build 20348+ / Windows 11): all system audio except
//! the Dyktando X process tree. Older systems return an activation error.
use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use wasapi::{initialize_mta, AudioClient, Direction, SampleType, StreamMode, WaveFormat};

use super::Sink;

const RATE: usize = 48_000;
const CHANNELS: usize = 2;

pub fn availability() -> Result<(), String> {
    Ok(())
}

pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    pub fn start(sink: Sink) -> Result<(Self, u32)> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let flag = stop.clone();
        let thread = std::thread::Builder::new().name("system-audio".into()).spawn(move || {
            if let Err(e) = run(sink, &flag, &ready_tx) {
                let _ = ready_tx.send(Err(e.to_string()));
            }
        })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok((Self { stop, thread: Some(thread) }, RATE as u32)),
            Ok(Err(e)) => Err(anyhow!(
                "Dźwięk systemowy: {e}. Nagrywanie dźwięku aplikacji wymaga Windows 11 albo Windows 10 (kompilacja 20348+)."
            )),
            Err(_) => Err(anyhow!("Dźwięk systemowy: wątek przechwytywania zakończył się")),
        }
    }

    /// Process loopback isn't bound to a specific output — there's nothing to watch.
    pub fn device_changed(&self) -> bool {
        false
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
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

fn run(mut sink: Sink, stop: &AtomicBool, ready: &mpsc::Sender<Result<(), String>>) -> Result<()> {
    let _ = initialize_mta();
    let format = WaveFormat::new(32, 32, &SampleType::Float, RATE, CHANNELS, None);
    let block_align = format.get_blockalign() as usize;
    // include_tree = false → PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE: everything except us.
    let mut client = AudioClient::new_application_loopback_client(std::process::id(), false).map_err(|e| anyhow!("{e}"))?;
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 };
    client.initialize_client(&format, &Direction::Capture, &mode).map_err(|e| anyhow!("{e}"))?;
    let event = client.set_get_eventhandle().map_err(|e| anyhow!("{e}"))?;
    let capture = client.get_audiocaptureclient().map_err(|e| anyhow!("{e}"))?;
    client.start_stream().map_err(|e| anyhow!("{e}"))?;
    let _ = ready.send(Ok(()));
    log::info!("WASAPI process loopback: {RATE} Hz, {CHANNELS} kan.");

    let mut queue: VecDeque<u8> = VecDeque::new();
    let mut mono: Vec<f32> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        if capture.get_next_packet_size().ok().flatten().unwrap_or(0) > 0 {
            if let Err(e) = capture.read_from_device_to_deque(&mut queue) {
                log::warn!("WASAPI: {e}");
            }
        }
        let frames = queue.len() / block_align;
        if frames > 0 {
            mono.clear();
            let bytes: Vec<u8> = queue.drain(..frames * block_align).collect();
            for frame in bytes.chunks_exact(block_align) {
                let l = f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
                let r = f32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
                mono.push((l + r) * 0.5);
            }
            sink(&mono);
        }
        // During silence the loopback sends no events — a timeout is not an error.
        let _ = event.wait_for_event(200);
    }
    let _ = client.stop_stream();
    Ok(())
}
