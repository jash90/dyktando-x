pub mod capture;
pub mod resample;

/// Average level (RMS) — for the meter in the bubble and detecting digital silence.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}

/// All zeros = no microphone access (without permission macOS returns silence without asking).
pub fn is_digital_silence(samples: &[f32]) -> bool {
    !samples.is_empty() && samples.iter().all(|x| *x == 0.0)
}
