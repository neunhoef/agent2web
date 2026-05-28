//! ForgeCode subprocess management.
//!
//! `spawn_agent_run` is the single public entry point: it spawns a tokio task
//! that runs `forge -p "<prompt>"` and streams output lines to the SSE broadcast
//! channel.  The agent never commits automatically; the user initiates commits
//! from the dedicated commit page.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use tracing::{debug, info, warn};

use crate::state::{AppState, ConvEntry, RunStatus};

/// Spawn the agent run as a background tokio task.
///
/// Returns immediately; the actual work runs concurrently.
pub fn spawn_agent_run(state: Arc<AppState>, prompt: String) {
    tokio::spawn(run_agent(state, prompt));
}

// ── Main task ────────────────────────────────────────────────────────────────

async fn run_agent(state: Arc<AppState>, prompt: String) {
    let timeout_secs = state.config.server.run_timeout;
    let project_dir = state.config.server.project_dir.clone();
    let forge_binary = state.config.forge.binary.clone();

    // Note whether this is the very first run (no active conversation yet).
    let is_first_run = state.conv.lock().expect("conv mutex").active_id.is_none();

    info!(
        prompt = %prompt,
        timeout_secs,
        is_first_run,
        "Starting agent run"
    );

    // Timeout handling is done inside `do_run` so the subprocess can be
    // explicitly killed when the deadline expires.
    match do_run(&state, &prompt, &project_dir, &forge_binary, timeout_secs).await {
        Err(e) => {
            warn!(error = %e, "Agent run failed or timed out");
            let mut run = state.run.lock().expect("run mutex");
            run.status = RunStatus::Failed {
                reason: e.to_string(),
            };
        }

        Ok(exit_success) => {
            info!(exit_success, "forge subprocess exited");

            // Update conversation state.
            update_conv_state(&state, &forge_binary, &prompt, is_first_run).await;

            let mut run = state.run.lock().expect("run mutex");
            run.status = if exit_success {
                RunStatus::Done
            } else {
                RunStatus::Failed {
                    reason: "forge exited with non-zero status".to_string(),
                }
            };
        }
    }

    // Notify SSE clients that the run has ended.
    let _ = state.sse_tx.send("__DONE__".to_string());
    debug!("Sent __DONE__ to SSE subscribers");
}

// ── Subprocess execution ─────────────────────────────────────────────────────

/// Spawn `forge -p "<prompt>"` and stream its stdout/stderr into the shared
/// output buffer and the SSE broadcast channel.
///
/// The timeout is enforced inside this function: if `timeout_secs` elapses
/// before the subprocess exits, the subprocess is killed with SIGKILL and
/// an error is returned.
///
/// Returns `Ok(true)` if forge exited with status 0, `Ok(false)` on non-zero
/// exit, or `Err` on spawn failure or timeout.
async fn do_run(
    state: &Arc<AppState>,
    prompt: &str,
    project_dir: &str,
    forge_binary: &str,
    timeout_secs: u64,
) -> anyhow::Result<bool> {
    debug!(forge_binary, prompt, "Spawning forge subprocess");

    let mut child = Command::new(forge_binary)
        .current_dir(project_dir)
        .args(["-p", prompt])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn '{}': {}", forge_binary, e))?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // Read stdout in a background task, forwarding each line to the output
    // buffer and the SSE channel.
    let state_stdout = Arc::clone(state);
    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            debug!("[forge stdout] {}", line);
            {
                let mut run = state_stdout.run.lock().expect("run mutex");
                run.output_buf.push(line.clone());
            }
            let _ = state_stdout.sse_tx.send(line);
        }
    });

    // Read stderr similarly, prefixing lines so the UI can style them.
    let state_stderr = Arc::clone(state);
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            debug!("[forge stderr] {}", line);
            let formatted = format!("[stderr] {line}");
            {
                let mut run = state_stderr.run.lock().expect("run mutex");
                run.output_buf.push(formatted.clone());
            }
            let _ = state_stderr.sse_tx.send(formatted);
        }
    });

    // ── Wait for the subprocess with a hard deadline ──────────────────────
    //
    // `tokio::time::timeout` wraps `child.wait()`.  When the deadline fires
    // the inner future is dropped (releasing the borrow on `child`) and we
    // can immediately call `child.kill()`.
    let wait_result = timeout(Duration::from_secs(timeout_secs), child.wait()).await;

    match wait_result {
        // ── Timeout ───────────────────────────────────────────────────────
        Err(_elapsed) => {
            // Announce timeout in the live stream.
            let msg = format!("[agent2web] Run timed out after {timeout_secs}s — killing process");
            {
                let mut run = state.run.lock().expect("run mutex");
                run.output_buf.push(msg.clone());
            }
            let _ = state.sse_tx.send(msg);

            // Kill the subprocess and wait for it to reap so no zombie is left.
            if let Err(e) = child.kill().await {
                warn!(error = %e, "Failed to send SIGKILL to timed-out forge process");
            }
            // Reap the process to release OS resources.
            child.wait().await.ok();

            // Drain the reader tasks (pipes are now closed; they exit quickly).
            stdout_task.await.ok();
            stderr_task.await.ok();

            Err(anyhow::anyhow!("Timed out after {timeout_secs} seconds"))
        }

        // ── Subprocess error (spawn / I/O failure) ────────────────────────
        Ok(Err(io_err)) => {
            stdout_task.await.ok();
            stderr_task.await.ok();
            Err(io_err.into())
        }

        // ── Normal exit ───────────────────────────────────────────────────
        Ok(Ok(status)) => {
            // Wait for both reader tasks to drain their pipes fully.
            stdout_task.await.ok();
            stderr_task.await.ok();
            Ok(status.success())
        }
    }
}

// ── Conversation state update ─────────────────────────────────────────────────

/// After a successful forge run, update `ConvState` with the conversation that
/// was used.
///
/// - First run (`is_first_run == true`): query `forge conversation list` to
///   discover which conversation forge created/used, then record it.
/// - Subsequent runs: increment the active entry's `run_count`.
async fn update_conv_state(
    state: &Arc<AppState>,
    forge_binary: &str,
    prompt: &str,
    is_first_run: bool,
) {
    if is_first_run {
        // Try to detect the conversation ID forge just used.
        match detect_latest_conversation(forge_binary).await {
            Some(id) => {
                info!(conv_id = %id, "Detected new conversation from first run");
                let label = truncate_label(prompt);
                let mut entry = ConvEntry::new(id.clone(), label);
                entry.run_count = 1;
                let mut conv = state.conv.lock().expect("conv mutex");
                conv.active_id = Some(id);
                conv.history.push(entry);
            }
            None => {
                warn!("Could not detect conversation ID after first forge run");
            }
        }
    } else {
        // Increment run_count on the current active conversation entry.
        let mut conv = state.conv.lock().expect("conv mutex");
        if let Some(entry) = conv.active_entry_mut() {
            entry.run_count += 1;
            debug!(conv_id = %entry.id, run_count = entry.run_count, "Incremented run_count");
        }
    }
}

/// Query `forge conversation list` and return the ID of the most recently
/// created conversation (the first entry in the list output).
///
/// Returns `None` if the command fails or produces no parseable output.
pub async fn detect_latest_conversation(forge_binary: &str) -> Option<String> {
    let output = Command::new(forge_binary)
        .args(["conversation", "list"])
        .output()
        .await
        .map_err(|e| warn!(error = %e, "Failed to run forge conversation list"))
        .ok()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(stderr = %stderr, "forge conversation list exited with non-zero status");
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    // The list is assumed to be most-recent-first, one entry per line.
    // We take the first non-empty line and its first whitespace-delimited token
    // as the conversation ID.
    text.lines()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| l.split_whitespace().next())
        .map(|s| s.to_string())
}

/// Run `forge conversation new` and return the newly created conversation ID.
pub async fn create_new_conversation(forge_binary: &str) -> anyhow::Result<String> {
    let output = Command::new(forge_binary)
        .args(["conversation", "new"])
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn '{}': {}", forge_binary, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("forge conversation new failed: {}", stderr.trim());
    }

    // The command prints the new ID to stdout (possibly with trailing newline).
    let id = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("forge conversation new produced no output"))?;

    Ok(id)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Truncate a prompt to a reasonable label length for display.
fn truncate_label(prompt: &str) -> String {
    const MAX: usize = 60;
    if prompt.len() <= MAX {
        prompt.to_string()
    } else {
        format!("{}…", &prompt[..MAX])
    }
}
