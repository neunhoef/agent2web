//! Handler for `POST /run` — submit a prompt and start an agent run.

use std::sync::Arc;

use axum::{
    Form,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use tracing::info;

use crate::{
    agent, auth,
    state::{AppState, RunStatus},
    templates,
};
#[derive(Debug, Deserialize)]
pub struct RunForm {
    /// The prompt text to send to forge.
    pub prompt: String,
    /// Password (only required when `server.password` is set).
    #[serde(default)]
    pub password: String,
}

/// `POST /run` — validate the prompt, reject if already running, spawn the
/// agent task, then redirect to `/`.
pub async fn post_run(State(state): State<Arc<AppState>>, Form(form): Form<RunForm>) -> Response {
    // ── Password check ────────────────────────────────────────────────────
    if let Some(err) = auth::check_password(&state.config.server, &form.password) {
        return err;
    }

    // ── Validate prompt ───────────────────────────────────────────────────
    let prompt = form.prompt.trim().to_string();
    if prompt.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Html(templates::render_error(400, "Prompt must not be empty.")),
        )
            .into_response();
    }

    // ── Reject if a run is already in progress ────────────────────────────
    {
        let run = state.run.lock().expect("run mutex poisoned");
        if run.status.is_running() {
            return (
                StatusCode::CONFLICT,
                Html(templates::render_error(
                    409,
                    "A run is already in progress. Please wait for it to finish.",
                )),
            )
                .into_response();
        }
    }

    // ── Transition to Running ─────────────────────────────────────────────
    {
        let mut run = state.run.lock().expect("run mutex poisoned");
        run.status = RunStatus::Running;
        run.output_buf.clear();
    }

    // ── Record prompt for the commit body ─────────────────────────────────
    {
        let mut prompts = state
            .prompts_since_commit
            .lock()
            .expect("prompts mutex poisoned");
        prompts.push(prompt.clone());
    }

    info!(prompt = %prompt, "Dispatching agent run");

    // ── Spawn the agent task ──────────────────────────────────────────────
    agent::spawn_agent_run(Arc::clone(&state), prompt);

    // ── Redirect to main page ─────────────────────────────────────────────
    Redirect::to("/").into_response()
}
