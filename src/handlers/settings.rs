//! Handler for `POST /settings/run-timeout` — update the agent run timeout
//! at runtime from the web UI.

use std::sync::Arc;

use axum::{
    Form,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use tracing::info;

use crate::{auth, state::AppState, templates};

/// Maximum accepted timeout: 24 hours.
const MAX_TIMEOUT_SECS: f64 = 24.0 * 60.0 * 60.0;

#[derive(Debug, Deserialize)]
pub struct RunTimeoutForm {
    /// New run timeout in minutes (fractional values allowed).
    pub timeout_minutes: f64,
    /// Password (only required when `server.password` is set).
    #[serde(default)]
    pub password: String,
}

/// `POST /settings/run-timeout` — validate the new timeout, store it in the
/// shared application state, then redirect back to the main page.
///
/// The value applies to the next agent run; a run already in progress keeps
/// the timeout it was started with.
pub async fn post_run_timeout(
    State(state): State<Arc<AppState>>,
    Form(form): Form<RunTimeoutForm>,
) -> Response {
    // ── Password check ────────────────────────────────────────────────────
    if let Some(err) = auth::check_password(&state.config.server, &form.password) {
        return err;
    }

    // ── Validate the requested timeout ────────────────────────────────────
    let minutes = form.timeout_minutes;
    if !minutes.is_finite() || minutes <= 0.0 || minutes * 60.0 > MAX_TIMEOUT_SECS {
        return (
            StatusCode::BAD_REQUEST,
            Html(templates::render_error(
                400,
                "Run timeout must be between 0 (exclusive) and 1440 minutes (24 hours).",
            )),
        )
            .into_response();
    }

    let secs = ((minutes * 60.0).round() as u64).max(1);
    state.set_run_timeout(secs);

    info!(timeout_secs = secs, "Run timeout updated from web UI");

    // ── Redirect to main page (the new value is shown in the form) ────────
    Redirect::to("/").into_response()
}
