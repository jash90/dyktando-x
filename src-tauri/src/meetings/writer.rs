//! Zapis ścieżki spotkania: 16 kHz mono, 16-bit WAV, nowy plik co 5 minut (`mic-001.wav`,
//! `mic-002.wav`…). Nagłówek aktualizujemy co kilka sekund, a po awarii `repair` przelicza
//! rozmiary z długości pliku — tracimy najwyżej ostatnie sekundy, nigdy całe nagranie.
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

/// Pliki danej ścieżki w kolejności.
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
        self.current = Some(hound::WavWriter::create(&path, spec()).with_context(|| format!("tworzenie {}", path.display()))?);
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
            let w = self.current.as_mut().expect("otwarty segment");
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

/// Naprawia nagłówek WAV po awarii (rozmiary RIFF i `data` liczone z długości pliku).
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
            // Pełne próbki 16-bit.
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

/// Odczyt całej ścieżki (wszystkich segmentów) jako 16 kHz f32 — porcjami przez `on_chunk`,
/// żeby godzinne nagranie nie musiało leżeć w RAM.
pub fn read_track(dir: &Path, prefix: &str, chunk_seconds: u32, mut on_chunk: impl FnMut(u64, &[f32]) -> Result<()>) -> Result<u64> {
    let chunk = (chunk_seconds * RATE) as usize;
    let mut buf: Vec<f32> = Vec::with_capacity(chunk);
    let mut offset: u64 = 0;
    for path in segments(dir, prefix) {
        let mut reader = match hound::WavReader::open(&path) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("Pomijam {}: {e}", path.display());
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

/// Cała ścieżka jako jeden plik WAV (eksport). Zapis idzie do pliku tymczasowego obok celu,
/// więc przerwany eksport nie zostawia uciętego pliku ani nie psuje istniejącego.
pub fn export_wav(dir: &Path, prefix: &str, target: &Path) -> Result<u64> {
    let mut part = target.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let result = (|| {
        let mut w = hound::WavWriter::create(&part, spec())?;
        let n = read_track(dir, prefix, 60, |_, chunk| {
            for s in chunk {
                w.write_sample((s * 32768.0).round().clamp(-32768.0, 32767.0) as i16)?;
            }
            Ok(())
        })?;
        w.finalize()?;
        std::fs::rename(&part, target).with_context(|| format!("Nie udało się zapisać {}", target.display()))?;
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
    fn rand_suffix() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn rotates_segments_and_reads_back() {
        let dir = tmp();
        let mut w = SegmentedWriter::new(&dir, "mic").unwrap();
        // 12,5 min w kawałkach po 0,1 s
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
        // „Awaria”: nie wołamy finish, zrzucamy bufor i porzucamy obiekt bez finalize.
        if let Some(writer) = w.current.as_mut() {
            writer.flush().unwrap();
        }
        w.append(&vec![0.25; 8_000]).unwrap();
        let path = segment_path(&dir, "system", 1);
        {
            let inner = w.current.take().unwrap();
            // Dopisz dane bez aktualizacji nagłówka.
            std::mem::forget(inner);
        }
        // forget mógł nie zrzucić bufora — dopisz ręcznie 8000 próbek jak po awarii
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
    fn export_of_missing_track_leaves_no_file() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("none.wav");
        export_wav(&dir.join("brak"), "mic", &out).ok();
        assert!(!dir.join("none.wav.part").exists());
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
        assert_eq!(export_wav(&dir, "mic", &out).unwrap(), 40_100);
        assert_eq!(hound::WavReader::open(&out).unwrap().duration(), 40_100);
        assert!(!dir.join("all.wav.part").exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
