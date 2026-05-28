//! whisper.cpp subprocess provider (Milestone 8).
//!
//! Invokes `whisper-cli` (the command-line tool from whisper.cpp) as a
//! subprocess for each transcription request.  The audio handler has already
//! converted the browser recording to a 16 kHz mono WAV by the time this
//! provider receives it, so the file can be passed directly to `whisper-cli`.
//!
//! ## Trade-offs vs. `whisper_server`
//!
//! This provider is simpler to set up — just `whisper-cli` on `$PATH` and a
//! model file — but it pays the full model-load penalty on every request
//! (~2–4 s on CPU, ~0.5 s on a mid-range GPU).  For interactive dictation the
//! latency is noticeable.  If you have a GPU and can run `whisper-server` as a
//! persistent service, prefer the `whisper_server` provider instead.
//!
//! ## Configuration (`agent2web.toml`)
//!
//! ```toml
//! [stt]
//! provider = "whisper_cpp"
//!
//! [stt.whisper_cpp]
//! binary   = "whisper-cli"                           # name or path
//! model    = "/path/to/ggml-large-v3-turbo.bin"     # required
//! language = "auto"                                  # or "en", "de", …
//! threads  = 4                                       # CPU threads
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, warn};

use super::SttProvider;

// ── Provider struct ────────────────────────────────────────────────────────────

/// STT provider that invokes `whisper-cli` as a subprocess.
pub struct WhisperCppProvider {
    /// Path or name of the `whisper-cli` binary.
    binary: String,
    /// Path to the GGML model file.
    model: String,
    /// Language hint (BCP-47 code or `"auto"`).
    language: String,
    /// Number of CPU threads forwarded to `whisper-cli` via `-t`.
    threads: u32,
}

impl WhisperCppProvider {
    pub fn new(binary: String, model: String, language: String, threads: u32) -> Self {
        Self {
            binary,
            model,
            language,
            threads,
        }
    }
}

// ── SttProvider implementation ─────────────────────────────────────────────────

#[async_trait]
impl SttProvider for WhisperCppProvider {
    async fn transcribe(&self, audio: Bytes, _mime_type: &str) -> anyhow::Result<String> {
        // Build a unique temp-file path so concurrent requests don't collide.
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let wav_path = std::env::temp_dir().join(format!("agent2web_cpp_{id}.wav"));

        // Write audio bytes to the temp file.
        {
            let mut f = tokio::fs::File::create(&wav_path).await?;
            f.write_all(&audio).await?;
            f.flush().await?;
        }

        let threads_str = self.threads.to_string();
        let wav_str = wav_path.to_string_lossy();

        debug!(
            binary = %self.binary,
            model  = %self.model,
            lang   = %self.language,
            threads = %self.threads,
            wav    = %wav_str,
            bytes  = audio.len(),
            "Invoking whisper-cli"
        );

        // Invoke whisper-cli.
        //
        // Key flags:
        //   -m <model>       GGML model file
        //   -f <file>        input WAV
        //   --no-timestamps  omit timestamp annotations from the output
        //   -np              no-prints: suppress progress/spinner lines
        //   -l <lang>        language hint; "auto" triggers autodetect
        //   -t <n>           number of CPU threads
        //
        // Stderr (info/progress) is inherited by the server process so it
        // appears in the server log; stdout carries only the transcript.
        let output = Command::new(&self.binary)
            .args([
                "-m",
                &self.model,
                "-f",
                &wav_str,
                "--no-timestamps",
                "-np",
                "-l",
                &self.language,
                "-t",
                &threads_str,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await;

        // Remove the temp file regardless of subprocess outcome.
        let _ = tokio::fs::remove_file(&wav_path).await;

        let output = output
            .map_err(|e| anyhow::anyhow!("Failed to spawn whisper-cli '{}': {}", self.binary, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "whisper-cli exited with {}: {}",
                output.status,
                stderr.trim()
            );
        }

        // Collect the transcript from stdout.
        //
        // whisper-cli prints one segment per line (with `--no-timestamps`
        // and `-np` there should be no noise lines).  We trim each line,
        // drop empty ones, and join the rest with a single space so the
        // caller gets a clean single-paragraph string.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let transcript = stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");

        if transcript.is_empty() {
            warn!(
                binary = %self.binary,
                model  = %self.model,
                "whisper-cli produced no transcript — audio may be silent or too short"
            );
        }

        Ok(transcript)
    }
}
