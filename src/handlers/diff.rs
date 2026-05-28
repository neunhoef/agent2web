//! Handlers for `GET /diff`, `GET /diff/commit`, and `GET /diff/range`.
//!
//! All three endpoints return the complete, self-contained HTML page that
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

/// `GET /diff` — render the working-tree diff (`git diff HEAD`), showing all
/// uncommitted changes since the last commit.
pub async fn get_diff(State(state): State<Arc<AppState>>) -> Response {
    let project_dir = &state.config.server.project_dir;
    let context_lines = state.config.diff.context_lines;

    match diff::render_working_tree(project_dir, context_lines).await {
        Ok(result) if result.is_empty => {
            empty_diff_page("No uncommitted changes in the working tree.")
        }
        Ok(result) => Html(result.html).into_response(),
        Err(e) => {
            warn!(error = %e, "Failed to render working-tree diff");
            error_page(&e.to_string())
        }
    }
}

// ── GET /diff/commit ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DiffCommitQuery {
    /// The commit ref to diff (defaults to `HEAD`).
    pub r#ref: Option<String>,
}

/// `GET /diff/commit?ref=REF` — render the diff introduced by a single commit.
/// Defaults to `HEAD` (the most recent commit) when `ref` is absent.
pub async fn get_diff_commit(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DiffCommitQuery>,
) -> Response {
    let project_dir = &state.config.server.project_dir;
    let context_lines = state.config.diff.context_lines;
    let rev = params.r#ref.as_deref().unwrap_or("HEAD");

    match diff::render_single_commit(project_dir, rev, context_lines).await {
        Ok(result) if result.is_empty => empty_diff_page("No changes in that commit."),
        Ok(result) => Html(result.html).into_response(),
        Err(e) => {
            warn!(error = %e, "Failed to render single-commit diff");
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
        Ok(result) if result.is_empty => empty_diff_page("No changes in the specified range."),
        Ok(result) => Html(result.html).into_response(),
        Err(e) => {
            warn!(error = %e, "Failed to render diff range");
            error_page(&e.to_string())
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Render a lightweight "nothing to show" page for empty diffs.
///
/// Uses an info-styled notice (not an error) with a back link to the main page.
fn empty_diff_page(msg: &str) -> Response {
    (StatusCode::OK, Html(templates::render_no_diff(msg))).into_response()
}

fn error_page(message: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(templates::render_error(500, message)),
    )
        .into_response()
}
