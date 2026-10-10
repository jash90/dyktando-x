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

/// Error text of a cancelled job. A fixed token, not translated: the meetings window compares
/// against it to stay silent about a cancellation (it's never shown).
pub const CANCELLED: &str = "Przerwano";
