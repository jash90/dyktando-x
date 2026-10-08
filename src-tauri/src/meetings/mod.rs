//! Nagrywanie spotkań: dwie ścieżki → transkrypcja z mówcami → podsumowanie AI.
pub mod detector;
pub mod diarize;
pub mod import;
pub mod live;
pub mod processing;
pub mod recorder;
pub mod store;
pub mod system;
pub mod transcriber;
pub mod transcript;
pub mod vad;
pub mod writer;
