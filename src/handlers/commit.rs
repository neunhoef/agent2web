//! Handlers for `GET /commit` (commit page) and `POST /commit` (execute commit).
//!
//! The commit flow:
//!   1. `GET /commit` — renders a page listing changed files (checkboxes),
//!      a subject input, and an optional prompt-body preview.
//!   2. `POST /commit` — stages the selected files, builds the commit message,
//!      runs `git commit`, clears the prompt accumulator, and redirects to `/`.

use std::sync::Arc;

use axum::{
    Form,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::{auth, diff as git_diff, state::AppState, templates};

// ── GET /commit ───────────────────────────────────────────────────────────────

/// `GET /commit` — render the commit page with file selector, subject input,
/// and prompt preview.
pub async fn get_commit(State(state): State<Arc<AppState>>) -> Response {
    let project_dir = state.config.server.project_dir.clone();
    let password_enabled = !state.config.server.password.is_empty();

    // Enumerate changed files from `git status --porcelain`.
    let changed_files = match list_changed_files(&project_dir).await {
        Ok(files) => files,
        Err(e) => {
            warn!(error = %e, "git status --porcelain failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(templates::render_error(
                    500,
                    &format!("git status failed: {e}"),
                )),
            )
                .into_response();
        }
    };

    // Collect prompts for preview.
    let prompts: Vec<String> = {
        state
            .prompts_since_commit
            .lock()
            .expect("prompts mutex poisoned")
            .clone()
    };

    Html(templates::render_commit_page(
        &changed_files,
        &prompts,
        password_enabled,
    ))
    .into_response()
}

// ── POST /commit ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CommitForm {
    /// Commit subject line (required).
    pub message: String,
    /// Password (only required when `server.password` is set).
    #[serde(default)]
    pub password: String,
    /// Selected file paths to stage (may be empty — validated below).
    ///
    /// When multiple checkboxes are checked the browser sends repeated keys
    /// (`files=a&files=b`) which serde_urlencoded maps to `Vec<String>`.
    /// When exactly one checkbox is checked the browser sends a bare string
    /// (`files=a`), which serde_urlencoded cannot coerce into a sequence on
    /// its own — the custom deserializer below handles both cases.
    #[serde(default, deserialize_with = "string_or_seq")]
    pub files: Vec<String>,
    /// Whether to append prompts to the commit body.
    /// An HTML checkbox sends `"on"` when checked and nothing when unchecked;
    /// we treat any non-empty value as true.
    #[serde(default)]
    pub include_prompts: String,
}

/// Deserialize a form field that may arrive as a single string or as a
/// repeated-key sequence.  Needed because HTML forms submit `field=value`
/// (string) when only one checkbox is ticked, and `field=v1&field=v2`
/// (sequence) when several are ticked.
fn string_or_seq<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{SeqAccess, Visitor};
    use std::fmt;

    struct StringOrSeq;

    impl<'de> Visitor<'de> for StringOrSeq {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a string or a sequence of strings")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(vec![v.to_owned()])
        }

        fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(vec![v])
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(v) = seq.next_element::<String>()? {
                out.push(v);
            }
            Ok(out)
        }
    }

    deserializer.deserialize_any(StringOrSeq)
}

/// `POST /commit` — stage selected files and commit with the supplied subject.
pub async fn post_commit(
    State(state): State<Arc<AppState>>,
    Form(form): Form<CommitForm>,
) -> Response {
    // ── Password check ────────────────────────────────────────────────────
    if let Some(err) = auth::check_password(&state.config.server, &form.password) {
        return err;
    }

    // ── Validate subject ──────────────────────────────────────────────────
    let subject = form.message.trim().to_string();
    if subject.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Html(templates::render_error(
                400,
                "Commit subject must not be empty.",
            )),
        )
            .into_response();
    }

    // ── Reject if a run is in progress ────────────────────────────────────
    {
        let run = state.run.lock().expect("run mutex poisoned");
        if run.status.is_running() {
            return (
                StatusCode::CONFLICT,
                Html(templates::render_error(
                    409,
                    "Cannot commit while an agent run is in progress.",
                )),
            )
                .into_response();
        }
    }

    // ── Validate submitted file paths against current git status ──────────
    if form.files.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Html(templates::render_error(
                400,
                "No files selected. Please select at least one file to commit.",
            )),
        )
            .into_response();
    }

    let project_dir = state.config.server.project_dir.clone();

    let allowed_files = match list_changed_files(&project_dir).await {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(templates::render_error(
                    500,
                    &format!("git status failed: {e}"),
                )),
            )
                .into_response();
        }
    };

    let allowed_paths: std::collections::HashSet<&str> =
        allowed_files.iter().map(|e| e.path.as_str()).collect();

    for submitted in &form.files {
        if !allowed_paths.contains(submitted.as_str()) {
            let msg = format!("Invalid or unknown file path: {submitted}");
            return (
                StatusCode::BAD_REQUEST,
                Html(templates::render_error(400, &msg)),
            )
                .into_response();
        }
    }

    // ── Build commit message ──────────────────────────────────────────────
    let include_prompts = !form.include_prompts.is_empty();
    let commit_message = {
        let prompts = state
            .prompts_since_commit
            .lock()
            .expect("prompts mutex poisoned")
            .clone();
        if include_prompts {
            build_commit_message(&subject, &prompts).await
        } else {
            subject.clone()
        }
    };

    debug!(message = %commit_message, files = ?form.files, "Running selective commit");

    // ── git add -- <files> ────────────────────────────────────────────────
    let mut add_cmd = Command::new("git");
    add_cmd.current_dir(&project_dir).arg("add").arg("--");
    for f in &form.files {
        add_cmd.arg(f);
    }

    let add_out = add_cmd.output().await;
    if let Err(e) = add_out {
        warn!(error = %e, "git add failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(templates::render_error(
                500,
                &format!("git add failed: {e}"),
            )),
        )
            .into_response();
    }
    let add_result = add_out.unwrap();
    if !add_result.status.success() {
        let stderr = String::from_utf8_lossy(&add_result.stderr);
        warn!(stderr = %stderr, "git add exited with non-zero status");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(templates::render_error(
                500,
                &format!("git add failed: {}", stderr.trim()),
            )),
        )
            .into_response();
    }

    // ── git commit -F - (message via stdin) ───────────────────────────────
    let mut child = match Command::new("git")
        .current_dir(&project_dir)
        .args(["commit", "-F", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "Failed to spawn git commit");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(templates::render_error(
                    500,
                    &format!("git commit failed to start: {e}"),
                )),
            )
                .into_response();
        }
    };

    if let Some(mut stdin) = child.stdin.take()
        && let Err(e) = stdin.write_all(commit_message.as_bytes()).await
    {
        warn!(error = %e, "Failed to write commit message to git stdin");
    }

    let output = match child.wait_with_output().await {
        Ok(o) => o,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(templates::render_error(
                    500,
                    &format!("git commit failed: {e}"),
                )),
            )
                .into_response();
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let details = format!("{}\n{}", stdout.trim(), stderr.trim());
        warn!(details = %details, "git commit exited with non-zero status");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(templates::render_error(
                500,
                &format!("git commit failed: {}", details.trim()),
            )),
        )
            .into_response();
    }

    info!(subject = %subject, "Commit succeeded");

    // Clear the accumulated prompts.
    {
        let mut prompts = state
            .prompts_since_commit
            .lock()
            .expect("prompts mutex poisoned");
        prompts.clear();
    }

    // Record the new commit in the in-memory history so the diff nav reflects
    // it immediately without requiring a server restart.
    let max_history = state.config.diff.max_history;
    match git_diff::get_latest_commit(&project_dir).await {
        Ok(commit) => {
            let mut history = state
                .commit_history
                .lock()
                .expect("commit_history mutex poisoned");
            history.insert(0, commit);
            history.truncate(max_history);
        }
        Err(e) => {
            warn!(error = %e, "Could not read new commit SHA after git commit (non-fatal)");
        }
    }

    Redirect::to("/").into_response()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// A single entry from `git status --porcelain`.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    /// Two-character status code from porcelain output (e.g. `" M"`, `"??"`, `"A "`).
    pub status: String,
    /// File path relative to the repository root.
    pub path: String,
}

/// Run `git status --porcelain` and return the list of changed files.
pub async fn list_changed_files(project_dir: &str) -> anyhow::Result<Vec<ChangedFile>> {
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain", "-uall"])
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git status --porcelain failed: {}", stderr.trim());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let files = text
        .lines()
        .filter(|l| l.len() >= 4)
        .map(|l| {
            // Porcelain format: two-char status, one space, then path.
            let status = l[..2].to_string();
            let path = l[3..].to_string();
            ChangedFile { status, path }
        })
        .collect();

    Ok(files)
}

/// Build the full commit message from a subject line and accumulated prompts.
///
/// The prompt body is piped through `par 72` for line-wrapping to 72 columns.
/// If `par` is not installed or fails the body is included without reflowing.
async fn build_commit_message(subject: &str, prompts: &[String]) -> String {
    if prompts.is_empty() {
        return subject.to_string();
    }

    // Join all prompts with a blank line between each, as the raw body to reflow.
    let raw_body = prompts.join("\n\n");

    // Attempt to reflow through `par 72`.
    let reflowed_body = reflow_with_par(&raw_body).await;

    format!("{subject}\n\nPrompts since last commit:\n\n{reflowed_body}")
}

/// Pipe `text` through `par 72` and return the reflowed output.
/// Falls back to the original text if `par` is not available or exits non-zero.
async fn reflow_with_par(text: &str) -> String {
    use tokio::io::AsyncWriteExt as _;

    let child = Command::new("par")
        .arg("72")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            debug!(error = %e, "par not available; using plain commit body");
            return text.to_string();
        }
    };

    // Write stdin concurrently with reading stdout to avoid pipe-buffer deadlock.
    let input = text.as_bytes().to_vec();
    let mut stdin_handle = child.stdin.take();
    let write_task: tokio::task::JoinHandle<std::io::Result<()>> = tokio::spawn(async move {
        if let Some(ref mut stdin) = stdin_handle {
            stdin.write_all(&input).await?;
        }
        Ok(()) // drop closes stdin → signals EOF to par
    });

    let output = match child.wait_with_output().await {
        Ok(o) => o,
        Err(e) => {
            debug!(error = %e, "par wait failed; using plain commit body");
            return text.to_string();
        }
    };

    let _ = write_task.await; // ignore stdin write errors

    if output.status.success() && !output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        debug!("par exited non-zero or produced no output; using plain commit body");
        text.to_string()
    }
}
