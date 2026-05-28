//! Diff rendering: invokes `git diff` and pipes through `diff2html-cli`.
//!
//! The implementation is isolated here so it can be swapped for a pure-Rust
//! renderer (e.g. the `similar` crate) in a future milestone without touching
//! any handler code.

use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, warn};

/// Output of a diff rendering operation.
pub struct DiffResult {
    /// The full HTML page emitted by diff2html-cli.
    /// Empty only when `git diff` produced no patch (nothing changed).
    pub html: String,
    /// True when `git diff` produced no output (nothing changed).
    pub is_empty: bool,
}

/// Render the working-tree diff (`git diff HEAD`) — all uncommitted changes.
///
/// This is the primary diff view after an agent run.
///
/// `context_lines` controls the `-U<n>` flag passed to `git diff`.
pub async fn render_working_tree(project_dir: &str, context_lines: usize) -> Result<DiffResult> {
    let context_flag = format!("-U{context_lines}");
    run_diff(
        project_dir,
        &["diff", "--stat", "-p", &context_flag, "HEAD"],
    )
    .await
}

/// Render the diff introduced by a single commit (`<ref>~1..<ref>`).
///
/// `rev` defaults to `HEAD` if an empty string is passed, which shows the
/// most recent commit.
///
/// `context_lines` controls the `-U<n>` flag passed to `git diff`.
pub async fn render_single_commit(
    project_dir: &str,
    rev: &str,
    context_lines: usize,
) -> Result<DiffResult> {
    validate_revision(rev)?;
    let range = format!("{rev}~1..{rev}");
    let context_flag = format!("-U{context_lines}");
    run_diff(
        project_dir,
        &["diff", "--stat", "-p", &context_flag, &range],
    )
    .await
}

/// Render the diff introduced by the most recent commit (`HEAD~1..HEAD`).
///
/// `context_lines` controls the `-U<n>` flag passed to `git diff`.
pub async fn render_last_commit(project_dir: &str, context_lines: usize) -> Result<DiffResult> {
    let context_flag = format!("-U{context_lines}");
    run_diff(
        project_dir,
        &["diff", "--stat", "-p", &context_flag, "HEAD~1"],
    )
    .await
}

/// Render the diff between two arbitrary git revisions.
pub async fn render_range(
    project_dir: &str,
    from: &str,
    to: &str,
    context_lines: usize,
) -> Result<DiffResult> {
    validate_revision(from)?;
    validate_revision(to)?;
    let range = format!("{from}..{to}");
    let context_flag = format!("-U{context_lines}");
    run_diff(
        project_dir,
        &["diff", "--stat", "-p", &context_flag, &range],
    )
    .await
}

// ── Internal helpers ───────────────────────────────────────────────────────

/// Reject revision strings that contain shell-unsafe characters.
///
/// Revisions are validated here even though we never pass them through a
/// shell; this provides defence-in-depth.
fn validate_revision(rev: &str) -> Result<()> {
    if rev.is_empty() {
        anyhow::bail!("Revision must not be empty");
    }
    // Allow alphanumeric, `.`, `/`, `-`, `_`, `~`, `^`, `@`, `{`, `}`.
    // Reject anything that looks like an option flag (`-`) at the start.
    if rev.starts_with('-') {
        anyhow::bail!("Revision must not start with '-'");
    }
    let allowed = |c: char| c.is_alphanumeric() || ".-/_~^@{}".contains(c);
    if !rev.chars().all(allowed) {
        anyhow::bail!("Revision contains disallowed characters: {rev}");
    }
    Ok(())
}

/// Core helper: run `git diff <git_args>`, pipe the output through
/// `diff2html-cli`, and return the rendered HTML page.
async fn run_diff(project_dir: &str, git_args: &[&str]) -> Result<DiffResult> {
    // ── Step 1: git diff ─────────────────────────────────────────────────
    debug!(
        dir = project_dir,
        args = ?git_args,
        "Running git diff"
    );

    let git_output = Command::new("git")
        .current_dir(project_dir)
        .args(git_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to spawn git — is git installed and is project_dir valid?")?;

    if !git_output.status.success() {
        let stderr = String::from_utf8_lossy(&git_output.stderr);
        warn!(
            status = ?git_output.status,
            stderr = %stderr,
            "git diff exited with non-zero status"
        );
        anyhow::bail!("git diff failed: {}", stderr.trim());
    }

    if git_output.stdout.is_empty() {
        debug!("git diff produced no output — nothing to diff");
        return Ok(DiffResult {
            html: String::new(),
            is_empty: true,
        });
    }

    // ── Step 2: diff2html-cli ────────────────────────────────────────────
    debug!("Piping diff output through diff2html");

    let mut child = Command::new("diff2html")
        .args([
            "--style", "line", // line-by-line diff view
            "-i", "stdin", // read patch from stdin
            "-o", "stdout", // write rendered HTML to stdout (not browser preview)
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(
            "Failed to spawn diff2html — is diff2html-cli installed? \
             Run: npm install -g diff2html-cli",
        )?;

    // Write stdin and collect stdout **concurrently**.
    //
    // If we write all of stdin first and only then read stdout, the OS pipe
    // buffer (~64 KB) can fill up: diff2html blocks trying to write more
    // stdout while we are blocked trying to write more stdin → deadlock →
    // eventual EPIPE on our stdin write.  The fix is to hand stdin off to a
    // background task so that `wait_with_output()` (which drains stdout) runs
    // at the same time.
    let stdin_bytes = git_output.stdout;
    let mut stdin_handle = child.stdin.take(); // must take before wait_with_output() moves child

    let write_task: tokio::task::JoinHandle<std::io::Result<()>> = tokio::spawn(async move {
        if let Some(ref mut stdin) = stdin_handle {
            stdin.write_all(&stdin_bytes).await?;
        }
        // `stdin_handle` is dropped here → EOF is signalled to diff2html.
        Ok(())
    });

    let output = child
        .wait_with_output()
        .await
        .context("Failed to wait for diff2html to finish")?;

    // Check the stdin writer; a BrokenPipe here just means diff2html exited
    // before consuming all input, which is not necessarily fatal.
    match write_task.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!(error = %e, "Writing to diff2html stdin failed (broken pipe?)"),
        Err(e) => warn!(error = %e, "diff2html stdin writer task panicked"),
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(
            status = ?output.status,
            stderr = %stderr,
            "diff2html exited with non-zero status"
        );
    }

    // diff2html emits a complete standalone HTML page; serve it as-is.
    let html = String::from_utf8_lossy(&output.stdout).into_owned();
    debug!(bytes = html.len(), "diff2html produced HTML");

    Ok(DiffResult {
        is_empty: html.trim().is_empty(),
        html,
    })
}
