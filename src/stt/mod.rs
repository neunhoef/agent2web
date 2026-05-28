//! Speech-to-text provider abstraction.
//!
//! The [`SttProvider`] trait is the single interface the audio handler uses.
//! Concrete implementations live in their respective sub-modules.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;

use crate::config::SttConfig;

pub mod deepgram;
pub mod whisper_api;
pub mod whisper_cpp;

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Abstraction over an STT backend.
///
/// Implementations must be `Send + Sync` so they can be held in `AppState`.
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Transcribe `audio` bytes (already in the correct format for this
    /// provider) and return the transcript string.
    ///
    /// `mime_type` is the MIME type of the audio data as submitted by the
    /// browser (e.g. `"audio/wav"` after ffmpeg conversion, or
    /// `"audio/webm"` for raw uploads to providers that accept it).
    async fn transcribe(&self, audio: Bytes, mime_type: &str) -> anyhow::Result<String>;
}

// ── Factory ───────────────────────────────────────────────────────────────────

/// Build the configured [`SttProvider`] from the application configuration.
pub fn build_provider(config: &SttConfig) -> Arc<dyn SttProvider> {
    match config.provider.as_str() {
        "whisper_server" => {
            let url = config
                .whisper_server
                .as_ref()
                .map(|c| c.url.clone())
                .unwrap_or_else(|| "http://127.0.0.1:8090/v1/audio/transcriptions".to_string());
            Arc::new(whisper_api::WhisperApiProvider::new(
                url,
                String::new(), // no API key for local server
                "whisper-1".to_string(),
            ))
        }
        "openai" => {
            let api_key = config.api_key.clone();
            let model = config
                .openai
                .as_ref()
                .map(|c| c.model.clone())
                .unwrap_or_else(|| "whisper-1".to_string());
            Arc::new(whisper_api::WhisperApiProvider::new(
                "https://api.openai.com/v1/audio/transcriptions".to_string(),
                api_key,
                model,
            ))
        }
        "deepgram" => {
            let api_key = config.api_key.clone();
            Arc::new(deepgram::DeepgramProvider::new(api_key))
        }
        other => {
            // Unknown provider: fall back to whisper_server defaults and log a warning.
            tracing::warn!(provider = %other, "Unknown STT provider; falling back to whisper_server");
            let url = config
                .whisper_server
                .as_ref()
                .map(|c| c.url.clone())
                .unwrap_or_else(|| "http://127.0.0.1:8090/v1/audio/transcriptions".to_string());
            Arc::new(whisper_api::WhisperApiProvider::new(
                url,
                String::new(),
                "whisper-1".to_string(),
            ))
        }
    }
}
