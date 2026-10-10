//! AI meeting summaries: providers (OpenAI, OpenRouter, Z.AI, Anthropic), API keys
//! in the system keychain, and map-reduce of long transcripts.

pub mod keys;
pub mod provider;
pub mod summarizer;
