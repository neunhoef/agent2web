//! Handler for `POST /audio` — receive a recorded audio blob, convert it to
//! 16 kHz mono WAV via `ffmpeg`, send it to the configured STT provider, and
//! return a JSON transcript.
//!
//! Expected multipart fields:
//! - `audio`    — the recorded audio blob (typically `audio/webm;codecs=opus`)
//! - `password` — shared password (required when authentication is enabled)
//!
//! Success response (`200 OK`):
//! ```json
//! { "transcript": "the recognised text" }
//! ```
//!
//! Error response (`4xx` / `5xx`):
//! ```json
//! { "error": "human-readable reason" }
//! ```

use std::sync::Arc;

use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::{auth, state::AppState};

// Maximum audio upload size: 25 MB.
const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

/// `POST /audio` — transcribe uploaded audio.
pub async fn post_audio(State(state): State<Arc<AppState>>, mut multipart: Multipart) -> Response {
    let mut audio_bytes: Option<Bytes> = None;
    let mut audio_mime: String = "audio/webm".to_string();
    let mut password: String = String::new();

    // ── Parse multipart fields ────────────────────────────────────────────
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let field_name = field.name().unwrap_or("").to_string();
                match field_name.as_str() {
                    "audio" => {
                        // Extract MIME type from field content-type header.
                        if let Some(ct) = field.content_type() {
                            audio_mime = ct.to_string();
                        }
                        match field.bytes().await {
                            Ok(b) => {
                                if b.len() > MAX_AUDIO_BYTES {
                                    return error_response(
                                        StatusCode::PAYLOAD_TOO_LARGE,
                                        "Audio upload exceeds the 25 MB limit.",
                                    );
                                }
                                audio_bytes = Some(b);
                            }
                            Err(e) => {
                                warn!(error = %e, "Failed to read audio field bytes");
                                return error_response(
                                    StatusCode::BAD_REQUEST,
                                    "Failed to read audio field.",
                                );
                            }
                        }
                    }
                    "password" => {
                        if let Ok(b) = field.bytes().await {
                            password = String::from_utf8_lossy(&b).into_owned();
                        }
                    }
                    _ => {
                        // Drain unknown fields so multipart parsing stays in sync.
                        let _ = field.bytes().await;
                    }
                }
            }
            Ok(None) => break, // all fields consumed
            Err(e) => {
                warn!(error = %e, "Multipart parse error");
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("Multipart parse error: {e}"),
                );
            }
        }
    }

    // ── Password check ────────────────────────────────────────────────────
    if !auth::is_password_valid(&state.config.server, &password) {
        return error_response(StatusCode::FORBIDDEN, "Incorrect password.");
    }

    // ── Validate audio presence ───────────────────────────────────────────
    let audio_bytes = match audio_bytes {
        Some(b) if !b.is_empty() => b,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Missing or empty `audio` field in multipart upload.",
            );
        }
    };

    debug!(
        mime = %audio_mime,
        bytes = audio_bytes.len(),
        "Audio received for transcription"
    );

    // ── Convert audio via ffmpeg (webm → 16 kHz mono WAV) ─────────────────
    // whisper-server and the OpenAI-compatible API both handle WAV reliably.
    // Deepgram accepts webm directly; the conversion is a no-op cost so we
    // convert in all cases for simplicity.
    let (audio_to_send, mime_to_send) = match convert_to_wav(audio_bytes).await {
        Ok(wav) => (wav, "audio/wav".to_string()),
        Err(e) => {
            // ffmpeg not available or failed — send the raw audio and hope the
            // provider can handle it.
            warn!(error = %e, "ffmpeg conversion failed; sending raw audio");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Audio conversion failed: {e}"),
            );
        }
    };

    // ── Transcribe ────────────────────────────────────────────────────────
    match state.stt.transcribe(audio_to_send, &mime_to_send).await {
        Ok(transcript) => {
            info!(chars = transcript.len(), "Transcription successful");
            (StatusCode::OK, Json(json!({ "transcript": transcript }))).into_response()
        }
        Err(e) => {
            warn!(error = %e, "Transcription failed");
            error_response(
                StatusCode::BAD_GATEWAY,
                &format!("Transcription failed: {e}"),
            )
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Convert `input` audio bytes to 16 kHz mono WAV using `ffmpeg`.
///
/// The input is written to a temp file, ffmpeg processes it, and the WAV bytes
/// are read back.  Both temp files are cleaned up before returning.
async fn convert_to_wav(input: Bytes) -> anyhow::Result<Bytes> {
    // Use a short unique suffix to avoid collisions under concurrent uploads.
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();

    let in_path = std::env::temp_dir().join(format!("agent2web_{id}.webm"));
    let out_path = std::env::temp_dir().join(format!("agent2web_{id}.wav"));

    // Write raw input to temp file.
    {
        let mut f = tokio::fs::File::create(&in_path).await?;
        f.write_all(&input).await?;
        f.flush().await?;
    }

    // Run ffmpeg.
    let status = Command::new("ffmpeg")
        .args([
            "-y", // overwrite output without prompting
            "-i",
            in_path.to_str().unwrap_or(""),
            "-ar",
            "16000", // 16 kHz sample rate
            "-ac",
            "1", // mono
            "-f",
            "wav",
            out_path.to_str().unwrap_or(""),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;

    // Clean up input file regardless of ffmpeg result.
    let _ = tokio::fs::remove_file(&in_path).await;

    if !status.success() {
        let _ = tokio::fs::remove_file(&out_path).await;
        anyhow::bail!("ffmpeg exited with status {}", status);
    }

    // Read WAV output.
    let wav_bytes = tokio::fs::read(&out_path).await?;
    let _ = tokio::fs::remove_file(&out_path).await;

    Ok(Bytes::from(wav_bytes))
}

/// Build a JSON error response.
fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json::<Value>(json!({ "error": message }))).into_response()
}
