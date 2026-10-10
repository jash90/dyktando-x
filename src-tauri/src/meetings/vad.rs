//! Splitting a track into utterances (Silero VAD v4, 32 ms frames). Probabilities are computed
//! as a stream over the whole track (continuous LSTM state), and segments are built by the pure function `segments`.
use anyhow::{anyhow, Result};
use ndarray::{Array1, Array2, Array3};
use ort::inputs;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;

use super::writer;

/// Silero v4 at 16 kHz is trained on 512-sample windows. With a 480 window (as transcribe-rs does)
/// the LSTM state "drifts" after a minute of recording and the model stops detecting speech.
pub const FRAME: usize = 512;
pub const FRAME_SECONDS: f64 = FRAME as f64 / 16_000.0;

#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub threshold: f32,
    /// Shorter speech is noise/a click.
    pub min_speech: f64,
    /// Shorter silence doesn't split an utterance.
    pub min_silence: f64,
    /// Margin added on both sides (we don't cut off the beginning and end of words).
    pub pad: f64,
    /// Longer segments are split (engines handle shorter ones better; as in FluidAudio ~14 s).
    pub max_segment: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self { threshold: 0.5, min_speech: 0.25, min_silence: 0.5, pad: 0.2, max_segment: 14.0 }
    }
}

/// Speech segments (start, end) in seconds from frame probabilities.
pub fn segments(probs: &[f32], p: Params) -> Vec<(f64, f64)> {
    let total = probs.len() as f64 * FRAME_SECONDS;
    let mut raw: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut silence = 0usize;
    let min_silence = (p.min_silence / FRAME_SECONDS).round() as usize;
    for (i, &pr) in probs.iter().enumerate() {
        if pr >= p.threshold {
            if start.is_none() {
                start = Some(i);
            }
            silence = 0;
        } else if let Some(s) = start {
            silence += 1;
            if silence >= min_silence {
                raw.push((s, i + 1 - silence));
                start = None;
                silence = 0;
            }
        }
    }
    if let Some(s) = start {
        raw.push((s, probs.len() - silence));
    }
    let max_frames = (p.max_segment / FRAME_SECONDS) as usize;
    let mut out = Vec::new();
    for (s, e) in raw {
        if ((e - s) as f64) * FRAME_SECONDS < p.min_speech {
            continue;
        }
        // Splitting long segments at the quietest frame in the second half of the window.
        let mut cur = s;
        while e - cur > max_frames {
            let lo = cur + max_frames / 2;
            let hi = cur + max_frames;
            let cut = (lo..hi).min_by(|a, b| probs[*a].total_cmp(&probs[*b])).unwrap_or(hi);
            out.push((cur, cut));
            cur = cut;
        }
        out.push((cur, e));
    }
    out.into_iter()
        .map(|(s, e)| {
            ((s as f64 * FRAME_SECONDS - p.pad).max(0.0), (e as f64 * FRAME_SECONDS + p.pad).min(total))
        })
        .collect()
}

pub struct Silero {
    session: Session,
    h: Array3<f32>,
    c: Array3<f32>,
    sr: Array1<i64>,
}

impl Silero {
    pub fn load(model: &Path) -> Result<Self> {
        let err = |e: &dyn std::fmt::Display| anyhow!("VAD: {e}");
        let session = Session::builder()
            .map_err(|e| err(&e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| err(&e))?
            .with_intra_threads(1)
            .map_err(|e| err(&e))?
            .commit_from_file(model)
            .map_err(|e| err(&e))?;
        Ok(Self { session, h: Array3::zeros((2, 1, 64)), c: Array3::zeros((2, 1, 64)), sr: Array1::from_vec(vec![16_000]) })
    }

    pub fn probability(&mut self, frame: &[f32]) -> Result<f32> {
        let input = Array2::from_shape_vec((1, frame.len()), frame.to_vec())?;
        let err = |e: &dyn std::fmt::Display| anyhow!("VAD: {e}");
        let outputs = self
            .session
            .run(inputs![
                "input" => TensorRef::from_array_view(input.view().into_dyn()).map_err(|e| err(&e))?,
                "sr" => TensorRef::from_array_view(self.sr.view().into_dyn()).map_err(|e| err(&e))?,
                "h" => TensorRef::from_array_view(self.h.view().into_dyn()).map_err(|e| err(&e))?,
                "c" => TensorRef::from_array_view(self.c.view().into_dyn()).map_err(|e| err(&e))?,
            ])
            .map_err(|e| err(&e))?;
        let get = |name: &str| -> Result<Vec<f32>> {
            Ok(outputs
                .get(name)
                .ok_or_else(|| anyhow!(crate::i18n::t_with("model.vad_no_output", &[("name", &name)])))?
                .try_extract_array::<f32>()
                .map_err(|e| err(&e))?
                .iter()
                .copied()
                .collect())
        };
        let p = get("output")?.first().copied().unwrap_or(0.0);
        let (hn, cn) = (get("hn")?, get("cn")?);
        self.h = Array3::from_shape_vec((2, 1, 64), hn)?;
        self.c = Array3::from_shape_vec((2, 1, 64), cn)?;
        Ok(p)
    }

    pub fn reset(&mut self) {
        self.h.fill(0.0);
        self.c.fill(0.0);
    }
}

/// After this many consecutive silent frames (~1 s) we reset the state — otherwise on long recordings
/// (especially with very even silence) the LSTM saturates and later utterances get lowered scores.
const RESET_AFTER_SILENT_FRAMES: usize = 31;

/// Speech probabilities for the whole track. `cancel` is checked every chunk.
pub fn track_probabilities(model: &Path, dir: &Path, prefix: &str, cancel: &dyn Fn() -> bool) -> Result<Vec<f32>> {
    let mut vad = Silero::load(model)?;
    let mut probs = Vec::new();
    let mut carry: Vec<f32> = Vec::new();
    let mut silent = 0usize;
    writer::read_track(dir, prefix, 60, |_, chunk| {
        if cancel() {
            return Err(anyhow!(super::CANCELLED));
        }
        carry.extend_from_slice(chunk);
        let whole = carry.len() / FRAME * FRAME;
        for frame in carry[..whole].chunks_exact(FRAME) {
            let p = vad.probability(frame)?;
            probs.push(p);
            silent = if p < 0.15 { silent + 1 } else { 0 };
            if silent == RESET_AFTER_SILENT_FRAMES {
                vad.reset();
            }
        }
        carry.drain(..whole);
        Ok(())
    })?;
    Ok(probs)
}

/// Goes over the track once and yields the samples of each segment (segments sorted).
pub fn for_each_segment(dir: &Path, prefix: &str, segs: &[(f64, f64)], mut f: impl FnMut(usize, &[f32]) -> Result<()>) -> Result<()> {
    let to_sample = |t: f64| (t * 16_000.0).round() as u64;
    let mut next = 0usize;
    let mut buf: Vec<f32> = Vec::new();
    let mut buf_start: u64 = 0;
    writer::read_track(dir, prefix, 30, |offset, chunk| {
        if buf.is_empty() {
            buf_start = offset;
        }
        buf.extend_from_slice(chunk);
        let buf_end = buf_start + buf.len() as u64;
        while next < segs.len() && to_sample(segs[next].1) <= buf_end {
            let (s, e) = (to_sample(segs[next].0).max(buf_start), to_sample(segs[next].1));
            f(next, &buf[(s - buf_start) as usize..(e - buf_start) as usize])?;
            next += 1;
        }
        // Keep only what the next segments need.
        let keep_from = segs.get(next).map(|x| to_sample(x.0)).unwrap_or(buf_end).clamp(buf_start, buf_end);
        buf.drain(..(keep_from - buf_start) as usize);
        buf_start = keep_from;
        Ok(())
    })?;
    // Segments reaching the end of the track (rounding).
    let buf_end = buf_start + buf.len() as u64;
    while next < segs.len() {
        let (s, e) = (to_sample(segs[next].0).clamp(buf_start, buf_end), to_sample(segs[next].1).clamp(buf_start, buf_end));
        if e > s {
            f(next, &buf[(s - buf_start) as usize..(e - buf_start) as usize])?;
        }
        next += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probs(pattern: &[(f32, f64)]) -> Vec<f32> {
        pattern.iter().flat_map(|(p, secs)| std::iter::repeat(*p).take((secs / FRAME_SECONDS).round() as usize)).collect()
    }

    #[test]
    fn short_pause_does_not_split() {
        let pr = probs(&[(0.0, 1.0), (0.9, 2.0), (0.1, 0.3), (0.9, 2.0), (0.0, 1.0)]);
        let s = segments(&pr, Params::default());
        assert_eq!(s.len(), 1);
        assert!((s[0].0 - 0.8).abs() < 0.05 && (s[0].1 - 5.5).abs() < 0.05, "{s:?}");
    }

    #[test]
    fn long_pause_splits_and_clicks_are_dropped() {
        let pr = probs(&[(0.9, 2.0), (0.0, 1.0), (0.9, 0.09), (0.0, 1.0), (0.9, 1.5)]);
        let s = segments(&pr, Params::default());
        assert_eq!(s.len(), 2, "{s:?}");
    }

    #[test]
    fn long_speech_is_cut_below_max() {
        let mut pr = probs(&[(0.9, 40.0)]);
        pr[(10.0 / FRAME_SECONDS) as usize] = 0.2; // quietest spot in the second half of the window
        let s = segments(&pr, Params::default());
        assert!(s.len() >= 3);
        assert!(s.iter().all(|(a, b)| b - a <= 14.0 + 0.4 + 1e-9), "{s:?}");
        assert!((s[0].1 - 10.2).abs() < 0.05, "{s:?}");
    }
}
