//! Handler for `POST /commit` — manually commit the current working tree.
//!
//! The commit message is composed as:
//!   - **Subject**: the value of the `message` form field.
//!   - **Body**: a bulleted list of all prompts accumulated since the last commit.

use std::sync::Arc;

use axum::{
    Form,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::{state::AppState, templates};

#[derive(Debug, Deserialize)]
pub struct CommitForm {
    /// Commit subject line (required).
    pub message: String,
    #[serde(default)]
    pub password: String,
}

/// `POST /commit` — stage all changes and commit them with the supplied subject
/// and the accumulated-prompt body.
pub async fn post_commit(
    State(state): State<Arc<AppState>>,
    Form(form): Form<CommitForm>,
) -> Response {
    // ── Password check ────────────────────────────────────────────────────
    let expected = &state.config.server.password;
    if !expected.is_empty() && form.password != *expected {
        return (
            StatusCode::FORBIDDEN,
            Html(templates::render_error(403, "Incorrect password.")),
        )
            .into_response();
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

    let project_dir = state.config.server.project_dir.clone();

    // ── Build commit message ──────────────────────────────────────────────
    let commit_message = {
        let prompts = state
            .prompts_since_commit
            .lock()
            .expect("prompts mutex poisoned");
        build_commit_message(&subject, &prompts)
    };

    debug!(message = %commit_message, "Running manual commit");

    // ── git add -A ────────────────────────────────────────────────────────
    if let Err(e) = Command::new("git")
        .current_dir(&project_dir)
        .args(["add", "-A"])
        .output()
        .await
    {
        warn!(error = %e, "git add -A failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(templates::render_error(
                500,
                &format!("git add -A failed: {e}"),
            )),
        )
            .into_response();
    }

    // ── git commit ────────────────────────────────────────────────────────
    let commit_out = Command::new("git")
        .current_dir(&project_dir)
        .args(["commit", "-m", &commit_message])
        .output()
        .await;

    match commit_out {
        Err(e) => {
            warn!(error = %e, "git commit failed to execute");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(templates::render_error(
                    500,
                    &format!("git commit failed: {e}"),
                )),
            )
                .into_response()
        }
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let details = format!("{}\n{}", stdout.trim(), stderr.trim());
            warn!(details = %details, "git commit exited with non-zero status");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(templates::render_error(
                    500,
                    &format!("git commit failed: {}", details.trim()),
                )),
            )
                .into_response()
        }
        Ok(_) => {
            info!(subject = %subject, "Manual commit succeeded");
            // Clear the accumulated prompts.
            let mut prompts = state
                .prompts_since_commit
                .lock()
                .expect("prompts mutex poisoned");
            prompts.clear();
            drop(prompts);

            Redirect::to("/").into_response()
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the full commit message from a subject line and accumulated prompts.
fn build_commit_message(subject: &str, prompts: &[String]) -> String {
    if prompts.is_empty() {
        return subject.to_string();
    }
    let mut msg = format!("{subject}\n\nPrompts since last commit:\n");
    for p in prompts {
        msg.push_str(&format!("- {p}\n"));
    }
    msg
}
