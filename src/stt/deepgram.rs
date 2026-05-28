//! Deepgram Nova-3 STT provider.
//!
//! Deepgram accepts raw audio bytes (including `audio/webm`) with the MIME
//! type in the `Content-Type` header.  No format conversion is required.

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::SttProvider;

pub struct DeepgramProvider {
    client: Client,
    api_key: String,
}

impl DeepgramProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }
}

// ── Deepgram response schema (subset) ─────────────────────────────────────────

#[derive(Deserialize)]
struct DgResponse {
    results: DgResults,
}

#[derive(Deserialize)]
struct DgResults {
    channels: Vec<DgChannel>,
}

#[derive(Deserialize)]
struct DgChannel {
    alternatives: Vec<DgAlternative>,
}

#[derive(Deserialize)]
struct DgAlternative {
    transcript: String,
}

#[async_trait]
impl SttProvider for DeepgramProvider {
    async fn transcribe(&self, audio: Bytes, mime_type: &str) -> anyhow::Result<String> {
        debug!(
            mime_type = %mime_type,
            bytes = audio.len(),
            "Sending audio to Deepgram"
        );

        let resp = self
            .client
            .post("https://api.deepgram.com/v1/listen?model=nova-3&smart_format=true")
            .header("Authorization", format!("Token {}", self.api_key))
            .header("Content-Type", mime_type)
            .body(audio)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Deepgram API returned {}: {}", status, body);
        }

        let data: DgResponse = resp.json().await?;
        let transcript = data
            .results
            .channels
            .first()
            .and_then(|c| c.alternatives.first())
            .map(|a| a.transcript.trim().to_string())
            .unwrap_or_default();

        Ok(transcript)
    }
}
