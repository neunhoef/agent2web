//! Local whisper.cpp subprocess provider (Milestone 8 stub).
//!
//! This provider will invoke `whisper-cli` as a subprocess per request.
//! It is intentionally left unimplemented for now; `whisper_server` (the
//! persistent HTTP server) is the recommended local provider (see M8).
//!
//! To enable this provider set `[stt] provider = "whisper_cpp"` in the
//! configuration file.  Attempting to use it before M8 is implemented will
//! return an error.

use async_trait::async_trait;
use bytes::Bytes;

use super::SttProvider;

pub struct WhisperCppProvider;

#[async_trait]
impl SttProvider for WhisperCppProvider {
    async fn transcribe(&self, _audio: Bytes, _mime_type: &str) -> anyhow::Result<String> {
        anyhow::bail!(
            "The whisper_cpp provider is not yet implemented (planned for M8). \
             Use the whisper_server provider instead."
        )
    }
}
