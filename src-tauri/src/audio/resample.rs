//! Conversion to 16 kHz mono (the counterpart of `MonoResampler` in the Swift version).
//! Streaming — for meeting recordings, without holding it all in RAM — and one-shot for dictation.
use anyhow::{anyhow, Result};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

pub const TARGET_RATE: u32 = 16_000;
const CHUNK: usize = 1024;

/// Averages channels to mono (otherwise only the left channel would remain — a speaker on the
/// right would vanish).
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

pub struct StreamResampler {
    inner: Option<Fft<f32>>,
    pending: Vec<f32>,
    out_buf: Vec<f32>,
    /// How many leading output samples are filter delay (we cut them so the tracks line up).
    skip: usize,
}

impl StreamResampler {
    pub fn new(input_rate: u32) -> Result<Self> {
        if input_rate == TARGET_RATE {
            return Ok(Self { inner: None, pending: Vec::new(), out_buf: Vec::new(), skip: 0 });
        }
        let r = Fft::<f32>::new(input_rate as usize, TARGET_RATE as usize, CHUNK, 1, FixedSync::Input)
            .map_err(|e| anyhow!("resampler {input_rate} Hz: {e}"))?;
        let skip = r.output_delay();
        let out_buf = vec![0.0; r.output_frames_max()];
        Ok(Self { inner: Some(r), pending: Vec::with_capacity(CHUNK * 2), out_buf, skip })
    }

    /// Takes mono at the input sample rate, returns what has already been converted (16 kHz).
    pub fn push(&mut self, mono: &[f32]) -> Vec<f32> {
        let Some(r) = self.inner.as_mut() else {
            return mono.to_vec();
        };
        self.pending.extend_from_slice(mono);
        let mut out = Vec::new();
        while self.pending.len() >= r.input_frames_next() {
            let need = r.input_frames_next();
            let produced = {
                let input = InterleavedSlice::new(&self.pending[..need], 1, need).expect("input buffer");
                let cap = self.out_buf.len();
                let mut output = InterleavedSlice::new_mut(&mut self.out_buf, 1, cap).expect("output buffer");
                match r.process_into_buffer(&input, &mut output, None) {
                    Ok((_, produced)) => produced,
                    Err(e) => {
                        log::error!("resampler: {e}");
                        0
                    }
                }
            };
            self.pending.drain(..need);
            let mut slice = &self.out_buf[..produced];
            if self.skip > 0 {
                let n = self.skip.min(slice.len());
                self.skip -= n;
                slice = &slice[n..];
            }
            out.extend_from_slice(slice);
        }
        out
    }

    /// Flushes the remainder with silence and returns the last samples.
    pub fn flush(&mut self) -> Vec<f32> {
        let Some(r) = self.inner.as_ref() else {
            return Vec::new();
        };
        if self.pending.is_empty() {
            return Vec::new();
        }
        // Remaining input + the filter delay still held inside.
        let expected = (self.pending.len() as f64 * r.resample_ratio()).round() as usize + r.output_delay();
        let zeros = vec![0.0; r.input_frames_next() * 3];
        let mut out = self.push(&zeros);
        out.truncate(expected);
        self.pending.clear();
        out
    }
}

/// One-shot conversion of the whole recording.
pub fn to_16k(mono: &[f32], input_rate: u32) -> Result<Vec<f32>> {
    if input_rate == TARGET_RATE {
        return Ok(mono.to_vec());
    }
    let expected = (mono.len() as u64 * TARGET_RATE as u64 / input_rate as u64) as usize;
    let mut r = StreamResampler::new(input_rate)?;
    let mut out = r.push(mono);
    out.extend(r.flush());
    out.resize(expected, 0.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
    }

    #[test]
    fn length_matches_ratio() {
        for rate in [44_100, 48_000, 96_000, 16_000, 8_000] {
            let input = sine(rate, 440.0, 2.0);
            let out = to_16k(&input, rate).unwrap();
            assert_eq!(out.len(), 32_000, "rate {rate}");
        }
    }

    #[test]
    fn preserves_tone_and_energy() {
        let out = to_16k(&sine(48_000, 440.0, 1.0), 48_000).unwrap();
        let reference = sine(16_000, 440.0, 1.0);
        // Skip the edges, compare the energy of the middle.
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let a = rms(&out[2000..14000]);
        let b = rms(&reference[2000..14000]);
        assert!((a - b).abs() < 0.02, "rms {a} vs {b}");
        // Phase matches (filter delay removed): correlation close to 1.
        let dot: f32 = out[2000..14000].iter().zip(&reference[2000..14000]).map(|(x, y)| x * y).sum();
        let corr = dot / (a * b * 12000.0);
        assert!(corr > 0.95, "korelacja {corr}");
    }

    #[test]
    fn streaming_equals_offline() {
        let input = sine(44_100, 300.0, 1.5);
        let offline = to_16k(&input, 44_100).unwrap();
        let mut r = StreamResampler::new(44_100).unwrap();
        let mut streamed = Vec::new();
        for chunk in input.chunks(333) {
            streamed.extend(r.push(chunk));
        }
        streamed.extend(r.flush());
        streamed.resize(offline.len(), 0.0);
        let max_diff = offline.iter().zip(&streamed).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(max_diff < 1e-4, "{max_diff}");
    }
}
