//! Meeting recording: two tracks → transcription with speakers → AI summary.
pub mod detector;
pub mod diarize;
#[cfg(test)]
mod eval;
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
