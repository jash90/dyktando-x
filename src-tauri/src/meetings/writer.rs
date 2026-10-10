//! Writing a meeting track: 16 kHz mono, 16-bit WAV, a new file every 5 minutes (`mic-001.wav`,
//! `mic-002.wav`…). The header is updated every few seconds, and after a crash `repair` recomputes
//! the sizes from the file length — we lose at most the last few seconds, never the whole recording.
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const RATE: u32 = 16_000;
pub const SEGMENT_SECONDS: u64 = 300;
const FLUSH_EVERY: Duration = Duration::from_secs(3);

fn spec() -> hound::WavSpec {
    hound::WavSpec { channels: 1, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }
}

pub fn segment_path(dir: &Path, prefix: &str, index: usize) -> PathBuf {
    dir.join(format!("{prefix}-{index:03}.wav"))
}

/// Files of a given track, in order.
pub fn segments(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    name.starts_with(&format!("{prefix}-")) && name.ends_with(".wav")
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

pub struct SegmentedWriter {
    dir: PathBuf,
    prefix: String,
    index: usize,
    current: Option<hound::WavWriter<BufWriter<File>>>,
    in_segment: u64,
    written: u64,
    last_flush: Instant,
}

impl SegmentedWriter {
    pub fn new(dir: &Path, prefix: &str) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            prefix: prefix.to_string(),
            index: 0,
            current: None,
            in_segment: 0,
            written: 0,
            last_flush: Instant::now(),
        })
    }

    pub fn samples_written(&self) -> u64 {
        self.written
    }

    fn open_next(&mut self) -> Result<()> {
        if let Some(w) = self.current.take() {
            w.finalize()?;
        }
        self.index += 1;
        let path = segment_path(&self.dir, &self.prefix, self.index);
        self.current = Some(hound::WavWriter::create(&path, spec()).with_context(|| crate::i18n::t_with("file.creating", &[("path", &path.display())]))?);
        self.in_segment = 0;
        Ok(())
    }

    pub fn append(&mut self, samples: &[f32]) -> Result<()> {
        let mut rest = samples;
        while !rest.is_empty() {
            if self.current.is_none() || self.in_segment >= SEGMENT_SECONDS * RATE as u64 {
                self.open_next()?;
            }
            let room = (SEGMENT_SECONDS * RATE as u64 - self.in_segment) as usize;
            let (now, later) = rest.split_at(room.min(rest.len()));
            let w = self.current.as_mut().expect("open segment");
            for s in now {
                w.write_sample((s.clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
            }
            self.in_segment += now.len() as u64;
            self.written += now.len() as u64;
            rest = later;
        }
        if self.last_flush.elapsed() >= FLUSH_EVERY {
            if let Some(w) = self.current.as_mut() {
                w.flush()?;
            }
            self.last_flush = Instant::now();
        }
        Ok(())
    }

    pub fn append_silence(&mut self, samples: u64) -> Result<()> {
        const CHUNK: usize = 16_000;
        let zeros = [0.0f32; CHUNK];
        let mut left = samples;
        while left > 0 {
            let n = left.min(CHUNK as u64) as usize;
            self.append(&zeros[..n])?;
            left -= n as u64;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64> {
        if let Some(w) = self.current.take() {
            w.finalize()?;
        }
        Ok(self.written)
    }
}

/// Repairs the WAV header after a crash (RIFF and `data` sizes computed from the file length).
pub fn repair(path: &Path) -> Result<bool> {
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let len = f.metadata()?.len();
    let mut header = vec![0u8; 512.min(len as usize)];
    f.read_exact(&mut header)?;
    if header.len() < 12 || &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Ok(false);
    }
    let mut pos = 12usize;
    while pos + 8 <= header.len() {
        let id = &header[pos..pos + 4];
        let size = u32::from_le_bytes(header[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            let data_start = pos as u64 + 8;
            // Whole 16-bit samples.
            let data_len = ((len - data_start) / 2 * 2) as u32;
            let riff_len = (data_start + data_len as u64 - 8) as u32;
            let current_data = size as u32;
            if current_data == data_len && u32::from_le_bytes(header[4..8].try_into().unwrap()) == riff_len {
                return Ok(false);
            }
            f.seek(SeekFrom::Start(4))?;
            f.write_all(&riff_len.to_le_bytes())?;
            f.seek(SeekFrom::Start(pos as u64 + 4))?;
            f.write_all(&data_len.to_le_bytes())?;
            f.set_len(data_start + data_len as u64)?;
            return Ok(true);
        }
        pos += 8 + size + (size & 1);
    }
    Ok(false)
}

/// Reads the whole track (all segments) as 16 kHz f32 — in chunks via `on_chunk`, so that
/// an hour-long recording doesn't have to sit in RAM.
pub fn read_track(dir: &Path, prefix: &str, chunk_seconds: u32, mut on_chunk: impl FnMut(u64, &[f32]) -> Result<()>) -> Result<u64> {
    let chunk = (chunk_seconds * RATE) as usize;
    let mut buf: Vec<f32> = Vec::with_capacity(chunk);
    let mut offset: u64 = 0;
    for path in segments(dir, prefix) {
        let mut reader = match hound::WavReader::open(&path) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("Skipping {}: {e}", path.display());
                continue;
            }
        };
        for s in reader.samples::<i16>() {
            let Ok(s) = s else { break };
            buf.push(s as f32 / 32768.0);
            if buf.len() == chunk {
                on_chunk(offset, &buf)?;
                offset += buf.len() as u64;
                buf.clear();
            }
        }
    }
    if !buf.is_empty() {
        on_chunk(offset, &buf)?;
        offset += buf.len() as u64;
    }
    Ok(offset)
}

pub fn track_duration_samples(dir: &Path, prefix: &str) -> u64 {
    segments(dir, prefix)
        .iter()
        .filter_map(|p| hound::WavReader::open(p).ok().map(|r| r.duration() as u64))
        .sum()
}

/// Successive samples of a track (all segments in order) without holding the whole thing in RAM.
/// Every segment except the last is exactly `SEGMENT_SECONDS` — a corrupted or truncated one is
/// padded with silence to that length so the rest of the track doesn't shift in time
/// (in the mix the two sides of the conversation would drift apart).
fn track_samples(dir: &Path, prefix: &str) -> impl Iterator<Item = i16> {
    let paths = segments(dir, prefix);
    let last = paths.len().saturating_sub(1);
    let full = (SEGMENT_SECONDS * RATE as u64) as usize;
    paths.into_iter().enumerate().flat_map(move |(i, path)| {
        let samples: Box<dyn Iterator<Item = i16>> = match hound::WavReader::open(&path) {
            Ok(r) => Box::new(r.into_samples::<i16>().map_while(|s| s.ok())),
            Err(e) => {
                log::warn!("Skipping {}: {e}", path.display());
                Box::new(std::iter::empty())
            }
        };
        if i < last {
            Box::new(samples.chain(std::iter::repeat(0)).take(full)) as Box<dyn Iterator<Item = i16>>
        } else {
            samples
        }
    })
}

/// Tracks as a single WAV file (export). Multiple tracks are mixed sample by sample:
/// they start at the same moment (the recorder pads gaps with silence), the shorter one ends in silence.
/// The sum is clipped to the 16-bit range — both sides speaking at once is rare and brief.
/// Writing goes to a temporary file next to the target, so an interrupted export leaves no truncated
/// file and doesn't corrupt an existing one.
pub fn export_wav(dir: &Path, prefixes: &[&str], target: &Path) -> Result<u64> {
    let mut part = target.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let result = (|| {
        let mut w = hound::WavWriter::create(&part, spec())?;
        let mut tracks: Vec<_> = prefixes.iter().map(|p| track_samples(dir, p).fuse()).collect();
        let mut n = 0u64;
        loop {
            let mut any = false;
            let mut sum = 0i32;
            for t in &mut tracks {
                if let Some(s) = t.next() {
                    any = true;
                    sum += s as i32;
                }
            }
            if !any {
                break;
            }
            w.write_sample(sum.clamp(i16::MIN as i32, i16::MAX as i32) as i16)?;
            n += 1;
        }
        w.finalize()?;
        std::fs::rename(&part, target).with_context(|| crate::i18n::t_with("file.save_failed", &[("path", &target.display())]))?;
        Ok(n)
    })();
    if result.is_err() {
        std::fs::remove_file(&part).ok();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("dx-writer-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    /// Tests run in parallel, and the macOS clock has microsecond resolution — time alone can
    /// give two tests the same directory, so we add a counter.
    fn rand_suffix() -> String {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("{}-{n}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
    }

    #[test]
    fn rotates_segments_and_reads_back() {
        let dir = tmp();
        let mut w = SegmentedWriter::new(&dir, "mic").unwrap();
        // 12.5 min in 0.1 s pieces
        let block: Vec<f32> = (0..1600).map(|i| (i as f32 / 1600.0) - 0.5).collect();
        for _ in 0..(750 * 10) {
            w.append(&block).unwrap();
        }
        assert_eq!(w.finish().unwrap(), 12_000_000);
        let segs = segments(&dir, "mic");
        assert_eq!(segs.len(), 3);
        assert_eq!(track_duration_samples(&dir, "mic"), 12_000_000);
        let mut total = 0;
        let mut first = None;
        read_track(&dir, "mic", 60, |_, c| {
            if first.is_none() {
                first = Some(c[0]);
            }
            total += c.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(total, 12_000_000);
        assert!((first.unwrap() + 0.5).abs() < 1e-3);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn repairs_truncated_header() {
        let dir = tmp();
        let mut w = SegmentedWriter::new(&dir, "system").unwrap();
        w.append(&vec![0.25; 16_000]).unwrap();
        // "Crash": we don't call finish, we flush the buffer and abandon the object without finalize.
        if let Some(writer) = w.current.as_mut() {
            writer.flush().unwrap();
        }
        w.append(&vec![0.25; 8_000]).unwrap();
        let path = segment_path(&dir, "system", 1);
        {
            let inner = w.current.take().unwrap();
            // Append data without updating the header.
            std::mem::forget(inner);
        }
        // forget might not have flushed the buffer — append 8000 samples manually as after a crash
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        let len_before = f.metadata().unwrap().len();
        if len_before < 44 + 48_000 {
            for _ in 0..(44 + 48_000 - len_before) / 2 {
                f.write_all(&8192i16.to_le_bytes()).unwrap();
            }
        }
        drop(f);
        assert!(repair(&path).unwrap());
        let r = hound::WavReader::open(&path).unwrap();
        assert_eq!(r.duration(), 24_000);
        assert!(!repair(&path).unwrap(), "drugie wywołanie nic nie zmienia");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn failed_export_leaves_no_part_file() {
        let dir = tmp();
        let mut w = SegmentedWriter::new(&dir, "mic").unwrap();
        w.append(&[0.1; 100]).unwrap();
        w.finish().unwrap();
        // Target occupied by a directory: writing to `.part` succeeds, the rename doesn't.
        let out = dir.join("taken.wav");
        std::fs::create_dir_all(&out).unwrap();
        assert!(export_wav(&dir, &["mic"], &out).is_err());
        assert!(!dir.join("taken.wav.part").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn mixes_tracks_and_pads_the_shorter_one() {
        let dir = tmp();
        let mut mic = SegmentedWriter::new(&dir, "mic").unwrap();
        mic.append(&[0.25; 3]).unwrap();
        mic.finish().unwrap();
        let mut sys = SegmentedWriter::new(&dir, "system").unwrap();
        sys.append(&[0.5, 1.0, -0.25, 0.5, 0.5]).unwrap();
        sys.finish().unwrap();
        let out = dir.join("mix.wav");
        assert_eq!(export_wav(&dir, &["mic", "system"], &out).unwrap(), 5);
        let got: Vec<i16> = hound::WavReader::open(&out).unwrap().into_samples().map(|s| s.unwrap()).collect();
        // 0.25+0.5, 0.25+1.0 (clipped), 0.25-0.25, then system alone.
        assert_eq!(got, vec![24576, i16::MAX, 0, 16384, 16384]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn broken_middle_segment_keeps_later_audio_in_place() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        // Truncated middle segment (10 samples instead of 5 minutes), then a healthy last one.
        let mut a = hound::WavWriter::create(segment_path(&dir, "mic", 1), spec()).unwrap();
        (0..10).for_each(|_| a.write_sample(100i16).unwrap());
        a.finalize().unwrap();
        let mut b = hound::WavWriter::create(segment_path(&dir, "mic", 2), spec()).unwrap();
        (0..3).for_each(|_| b.write_sample(7i16).unwrap());
        b.finalize().unwrap();
        let full = (SEGMENT_SECONDS * RATE as u64) as usize;
        let got: Vec<i16> = track_samples(&dir, "mic").collect();
        assert_eq!(got.len(), full + 3);
        assert_eq!(&got[..10], &[100; 10]);
        assert!(got[10..full].iter().all(|&s| s == 0));
        assert_eq!(&got[full..], &[7; 3]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn silence_and_export() {
        let dir = tmp();
        let mut w = SegmentedWriter::new(&dir, "mic").unwrap();
        w.append_silence(40_000).unwrap();
        w.append(&[0.5; 100]).unwrap();
        w.finish().unwrap();
        let out = dir.join("all.wav");
        assert_eq!(export_wav(&dir, &["mic"], &out).unwrap(), 40_100);
        assert_eq!(hound::WavReader::open(&out).unwrap().duration(), 40_100);
        assert!(!dir.join("all.wav.part").exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
