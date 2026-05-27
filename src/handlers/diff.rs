//! Handlers for `GET /diff` and `GET /diff/range`.
//!
//! Both endpoints return the complete, self-contained HTML page that
//! diff2html-cli generates (including its own CSS and styling).  The user
//! navigates to the diff page and uses the browser's Back button to return
//! to the main UI — no embedding or HTMX swapping needed.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use tracing::warn;

use crate::{diff, state::AppState, templates};

// ── GET /diff ─────────────────────────────────────────────────────────────

/// `GET /diff` — render the diff of the most recent commit (`HEAD~1..HEAD`).
pub async fn get_diff(State(state): State<Arc<AppState>>) -> Response {
    let project_dir = &state.config.server.project_dir;
    let context_lines = state.config.diff.context_lines;

    match diff::render_last_commit(project_dir, context_lines).await {
        Ok(result) if result.is_empty => empty_diff_page(),
        Ok(result) => Html(result.html).into_response(),
        Err(e) => {
            warn!(error = %e, "Failed to render diff");
            error_page(&e.to_string())
        }
    }
}

// ── GET /diff/range ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DiffRangeQuery {
    /// Starting revision (exclusive), e.g. `HEAD~3` or a commit SHA.
    pub from: Option<String>,
    /// Ending revision (inclusive), e.g. `HEAD` or a commit SHA.
    pub to: Option<String>,
}

/// `GET /diff/range?from=SHA&to=SHA` — render the diff between two commits.
pub async fn get_diff_range(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DiffRangeQuery>,
) -> Response {
    let project_dir = &state.config.server.project_dir;
    let context_lines = state.config.diff.context_lines;

    let from = params.from.as_deref().unwrap_or("HEAD~1");
    let to = params.to.as_deref().unwrap_or("HEAD");

    match diff::render_range(project_dir, from, to, context_lines).await {
        Ok(result) if result.is_empty => empty_diff_page(),
        Ok(result) => Html(result.html).into_response(),
        Err(e) => {
            warn!(error = %e, "Failed to render diff range");
            error_page(&e.to_string())
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn empty_diff_page() -> Response {
    (
        StatusCode::OK,
        Html(templates::render_error(
            200,
            "No changes — the repository has no previous commit or there is nothing to diff.",
        )),
    )
        .into_response()
}

fn error_page(message: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(templates::render_error(500, message)),
    )
        .into_response()
}
