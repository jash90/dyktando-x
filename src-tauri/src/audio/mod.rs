pub mod capture;
pub mod resample;

/// Średni poziom (RMS) — do wskaźnika w dymku i wykrywania cyfrowej ciszy.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Same zera = system nie dał dostępu do mikrofonu (macOS bez zgody zwraca ciszę bez pytania).
pub fn is_digital_silence(samples: &[f32]) -> bool {
    !samples.is_empty() && samples.iter().all(|x| *x == 0.0)
}
