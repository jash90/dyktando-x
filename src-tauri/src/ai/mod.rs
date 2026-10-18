//! AI meeting summaries: providers (OpenAI, OpenRouter, Z.AI, Anthropic), API keys
//! in the system keychain, and map-reduce of long transcripts; optional correction of dictations.

pub mod keys;
pub mod polish;
pub mod provider;
pub mod summarizer;
