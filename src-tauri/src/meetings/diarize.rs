//! Rozpoznawanie mówców na ścieżce „system” bez sherpa-onnx: dla każdej wypowiedzi (fragmentu
//! VAD) liczymy wektor głosu modelem WeSpeaker ResNet34 (ONNX, 256 wymiarów), a potem grupujemy
//! wypowiedzi aglomeracyjnie po podobieństwie kosinusowym. Krótkie wypowiedzi (< 1 s) dają
//! niepewne wektory — dopisujemy je do najbliższej grupy, nie tworzą własnych.
use anyhow::{anyhow, Result};
use ndarray::Array3;
use ort::inputs;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;
use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;
use std::path::Path;

const RATE: f32 = 16_000.0;
const FRAME: usize = 400; // 25 ms
const SHIFT: usize = 160; // 10 ms
const FFT: usize = 512;
const MELS: usize = 80;
/// Podobieństwo, powyżej którego dwie grupy to ten sam mówca (dobrane na nagraniach FLEURS).
pub const SAME_SPEAKER: f32 = 0.45;
const MIN_SECONDS_FOR_CLUSTER: f64 = 1.0;

/// Fbank w stylu Kaldi (jak w WeSpeaker): bez ditheru, usunięcie składowej stałej, preemfaza 0,97,
/// okno Poveya, 80 pasm mel 20 Hz–8 kHz, log energii; na końcu odjęcie średniej (CMN).
pub struct Fbank {
    window: Vec<f32>,
    mel: Vec<Vec<(usize, f32)>>,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
}

fn mel_scale(hz: f32) -> f32 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

impl Default for Fbank {
    fn default() -> Self {
        let window = (0..FRAME)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FRAME - 1) as f32).cos()).powf(0.85))
            .collect();
        let (low, high) = (mel_scale(20.0), mel_scale(RATE / 2.0));
        let delta = (high - low) / (MELS + 1) as f32;
        let bin_hz = RATE / FFT as f32;
        let mel = (0..MELS)
            .map(|m| {
                let (left, center, right) = (low + m as f32 * delta, low + (m + 1) as f32 * delta, low + (m + 2) as f32 * delta);
                (0..FFT / 2)
                    .filter_map(|k| {
                        let f = mel_scale(bin_hz * k as f32);
                        let w = if f > left && f <= center {
                            (f - left) / (center - left)
                        } else if f > center && f < right {
                            (right - f) / (right - center)
                        } else {
                            0.0
                        };
                        (w > 0.0).then_some((k, w))
                    })
                    .collect()
            })
            .collect();
        Self { window, mel, fft: FftPlanner::new().plan_fft_forward(FFT) }
    }
}

impl Fbank {
    /// `samples` w zakresie [-1, 1]; model oczekuje skali int16 (normalize_samples = 0).
    pub fn compute(&self, samples: &[f32]) -> Vec<[f32; MELS]> {
        if samples.len() < FRAME {
            return Vec::new();
        }
        let frames = 1 + (samples.len() - FRAME) / SHIFT;
        let mut out = Vec::with_capacity(frames);
        let mut buf = vec![Complex32::new(0.0, 0.0); FFT];
        let mut frame = [0f32; FRAME];
        for f in 0..frames {
            let src = &samples[f * SHIFT..f * SHIFT + FRAME];
            for (d, s) in frame.iter_mut().zip(src) {
                *d = s * 32768.0;
            }
            let mean = frame.iter().sum::<f32>() / FRAME as f32;
            frame.iter_mut().for_each(|x| *x -= mean);
            for i in (1..FRAME).rev() {
                frame[i] -= 0.97 * frame[i - 1];
            }
            frame[0] -= 0.97 * frame[0];
            for (i, c) in buf.iter_mut().enumerate() {
                *c = Complex32::new(if i < FRAME { frame[i] * self.window[i] } else { 0.0 }, 0.0);
            }
            self.fft.process(&mut buf);
            let mut row = [0f32; MELS];
            for (m, filt) in self.mel.iter().enumerate() {
                let e: f32 = filt.iter().map(|(k, w)| buf[*k].norm_sqr() * w).sum();
                row[m] = e.max(f32::EPSILON).ln();
            }
            out.push(row);
        }
        let n = out.len() as f32;
        let mut means = [0f32; MELS];
        for r in &out {
            for (m, v) in r.iter().enumerate() {
                means[m] += v / n;
            }
        }
        for r in &mut out {
            for (m, v) in r.iter_mut().enumerate() {
                *v -= means[m];
            }
        }
        out
    }
}

pub struct Embedder {
    session: Session,
    fbank: Fbank,
}

impl Embedder {
    pub fn load(model: &Path) -> Result<Self> {
        let err = |e: &dyn std::fmt::Display| anyhow!("Model mówców: {e}");
        let session = Session::builder()
            .map_err(|e| err(&e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| err(&e))?
            .with_intra_threads(2)
            .map_err(|e| err(&e))?
            .commit_from_file(model)
            .map_err(|e| err(&e))?;
        Ok(Self { session, fbank: Fbank::default() })
    }

    /// Znormalizowany wektor głosu (długość 1) albo `None` dla zbyt krótkiego fragmentu.
    pub fn embed(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>> {
        let feats = self.fbank.compute(samples);
        if feats.len() < 20 {
            return Ok(None);
        }
        let mut arr = Array3::<f32>::zeros((1, feats.len(), MELS));
        for (t, row) in feats.iter().enumerate() {
            for (m, v) in row.iter().enumerate() {
                arr[[0, t, m]] = *v;
            }
        }
        let input = TensorRef::from_array_view(arr.view().into_dyn()).map_err(|e| anyhow!("{e}"))?;
        let outputs = self.session.run(inputs!["feats" => input]).map_err(|e| anyhow!("Model mówców: {e}"))?;
        let emb = outputs
            .get("embs")
            .ok_or_else(|| anyhow!("Model mówców: brak wyjścia embs"))?
            .try_extract_array::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        let v: Vec<f32> = emb.iter().copied().collect();
        Ok(Some(normalize(v)))
    }
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Grupowanie: `items` = (czas trwania w s, wektor lub brak). Zwraca numer grupy dla każdej
/// pozycji (`None`, gdy brak wektora i nie ma żadnej grupy). Numery w kolejności pojawienia się.
pub fn cluster(items: &[(f64, Option<Vec<f32>>)], threshold: f32) -> Vec<Option<usize>> {
    // 1) aglomeracja (średnie wiązanie) tylko dla wypowiedzi ≥ 1 s
    let long: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, (d, e))| *d >= MIN_SECONDS_FOR_CLUSTER && e.is_some())
        .map(|(i, _)| i)
        .collect();
    let mut groups: Vec<Vec<usize>> = long.iter().map(|&i| vec![i]).collect();
    let emb = |i: usize| items[i].1.as_ref().expect("wektor");
    let avg_sim = |a: &Vec<usize>, b: &Vec<usize>| -> f32 {
        let mut s = 0.0;
        for &i in a {
            for &j in b {
                s += cosine(emb(i), emb(j));
            }
        }
        s / (a.len() * b.len()) as f32
    };
    loop {
        let mut best: Option<(usize, usize, f32)> = None;
        for i in 0..groups.len() {
            for j in i + 1..groups.len() {
                let s = avg_sim(&groups[i], &groups[j]);
                if s >= threshold && best.is_none_or(|(_, _, b)| s > b) {
                    best = Some((i, j, s));
                }
            }
        }
        let Some((i, j, _)) = best else { break };
        let merged = groups.remove(j);
        groups[i].extend(merged);
    }
    // 2) centroidy i przypisanie wszystkich (także krótkich) do najbliższej grupy
    let centroids: Vec<Vec<f32>> = groups
        .iter()
        .map(|g| {
            let mut c = vec![0.0; emb(g[0]).len()];
            for &i in g {
                for (k, v) in emb(i).iter().enumerate() {
                    c[k] += v;
                }
            }
            normalize(c)
        })
        .collect();
    let mut raw: Vec<Option<usize>> = vec![None; items.len()];
    for (g, members) in groups.iter().enumerate() {
        for &i in members {
            raw[i] = Some(g);
        }
    }
    for (i, (_, e)) in items.iter().enumerate() {
        if raw[i].is_none() {
            if let Some(e) = e {
                raw[i] = centroids
                    .iter()
                    .enumerate()
                    .map(|(g, c)| (g, cosine(e, c)))
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(g, _)| g);
            }
        }
    }
    // 3) numeracja w kolejności pojawienia się
    let mut order: Vec<usize> = Vec::new();
    raw.iter()
        .map(|g| {
            g.map(|g| match order.iter().position(|&x| x == g) {
                Some(p) => p,
                None => {
                    order.push(g);
                    order.len() - 1
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: &[f32]) -> Option<Vec<f32>> {
        Some(normalize(x.to_vec()))
    }

    #[test]
    fn clusters_by_similarity_and_numbers_by_appearance() {
        let items = vec![
            (3.0, v(&[0.0, 1.0, 0.1])),
            (2.0, v(&[1.0, 0.0, 0.0])),
            (4.0, v(&[0.05, 1.0, 0.0])),
            (0.5, v(&[0.9, 0.1, 0.0])), // krótka → do najbliższej grupy
            (2.0, None),
        ];
        assert_eq!(cluster(&items, 0.5), vec![Some(0), Some(1), Some(0), Some(1), None]);
    }

    #[test]
    fn fbank_shape_and_cmn() {
        let fb = Fbank::default();
        let tone: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.1).sin() * 0.3).collect();
        let f = fb.compute(&tone);
        assert_eq!(f.len(), 98);
        let mean0: f32 = f.iter().map(|r| r[0]).sum::<f32>() / f.len() as f32;
        assert!(mean0.abs() < 1e-3);
    }

    fn load(name: &str) -> Vec<f32> {
        let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let mut r = hound::WavReader::open(p).unwrap();
        r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
    }

    /// Wymaga modelu (pobierany przez aplikację albo test `models`): `cargo test -- --ignored speakers`.
    #[test]
    #[ignore]
    fn speakers_same_voice_closer_than_different() {
        let asset = crate::models::asset(crate::models::AssetId::SpeakerModel);
        if !asset.is_installed() {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(asset.download(&std::sync::atomic::AtomicBool::new(false), |_, _| {}))
                .unwrap();
        }
        let mut e = Embedder::load(&asset.file_path(0)).unwrap();
        let w = load("fleurs_kobieta.wav");
        let m = load("fleurs_mezczyzna.wav");
        let half = w.len() / 2;
        let w1 = e.embed(&w[..half]).unwrap().unwrap();
        let w2 = e.embed(&w[half..]).unwrap().unwrap();
        let m1 = e.embed(&m[..m.len() / 2]).unwrap().unwrap();
        let m2 = e.embed(&m[m.len() / 2..]).unwrap().unwrap();
        let same_w = cosine(&w1, &w2);
        let same_m = cosine(&m1, &m2);
        let diff = cosine(&w1, &m1).max(cosine(&w2, &m2));
        println!("ta sama kobieta {same_w:.3}, ten sam mężczyzna {same_m:.3}, różne {diff:.3}");
        assert!(same_w > SAME_SPEAKER && same_m > SAME_SPEAKER, "ten sam głos poniżej progu");
        assert!(diff < SAME_SPEAKER, "różne głosy powyżej progu");
    }
}
