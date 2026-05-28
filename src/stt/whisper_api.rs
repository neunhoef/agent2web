//! OpenAI-compatible Whisper API provider.
//!
//! Works with both:
//! - The hosted OpenAI API (`https://api.openai.com/v1/audio/transcriptions`)
//! - A local `whisper-server` built from `whisper.cpp` with
//!   `--inference-path /v1/audio/transcriptions` (OpenAI-compatible endpoint).
//!
//! When `api_key` is empty (local server case), the `Authorization` header is
//! omitted so the request works without authentication.

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::SttProvider;

/// An STT provider that talks to any OpenAI-compatible Whisper transcription
/// endpoint.
pub struct WhisperApiProvider {
    client: Client,
    /// Full URL of the transcription endpoint.
    url: String,
    /// Bearer token.  Empty string means no `Authorization` header is sent
    /// (appropriate for the local whisper-server).
    api_key: String,
    /// Model name forwarded to the API (e.g. `"whisper-1"`).
    model: String,
}

impl WhisperApiProvider {
    pub fn new(url: String, api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            url,
            api_key,
            model,
        }
    }
}

/// Subset of the OpenAI transcription response we care about.
#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[async_trait]
impl SttProvider for WhisperApiProvider {
    async fn transcribe(&self, audio: Bytes, mime_type: &str) -> anyhow::Result<String> {
        // Choose a filename extension that matches the MIME type so the server
        // can identify the format.
        let filename = if mime_type.contains("wav") {
            "audio.wav"
        } else if mime_type.contains("mp4") || mime_type.contains("m4a") {
            "audio.mp4"
        } else {
            "audio.webm"
        };

        debug!(
            url = %self.url,
            mime_type = %mime_type,
            filename = %filename,
            bytes = audio.len(),
            "Sending audio to Whisper API"
        );

        let part = reqwest::multipart::Part::bytes(audio.to_vec())
            .file_name(filename)
            .mime_str(mime_type)?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.model.clone());

        let mut builder = self.client.post(&self.url).multipart(form);

        if !self.api_key.is_empty() {
            builder = builder.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let resp = builder.send().await?;
        let status = resp.status();

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Whisper API returned {}: {}", status, body);
        }

        let data: TranscriptionResponse = resp.json().await?;
        Ok(data.text.trim().to_string())
    }
}
