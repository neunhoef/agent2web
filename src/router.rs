use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
};

use crate::{handlers, state::AppState};

/// Build and return the axum `Router` with all routes registered.
pub fn build(state: Arc<AppState>) -> Router {
    Router::new()
        // ── Read-only / informational ─────────────────────────────────────
        .route("/", get(handlers::index::get_index))
        .route("/health", get(handlers::health::get_health))
        // ── Agent run & live stream ───────────────────────────────────────
        .route("/run", post(handlers::run::post_run))
        .route("/stream", get(handlers::stream::get_stream))
        // ── Diff views ────────────────────────────────────────────────────
        .route("/diff", get(handlers::diff::get_diff))
        .route("/diff/commit", get(handlers::diff::get_diff_commit))
        .route("/diff/range", get(handlers::diff::get_diff_range))
        // ── Commit page & action ──────────────────────────────────────────
        .route(
            "/commit",
            get(handlers::commit::get_commit).post(handlers::commit::post_commit),
        )
        // ── Conversation management ───────────────────────────────────────
        .route(
            "/conversation/new",
            post(handlers::conversation::post_conversation_new),
        )
        .route(
            "/conversation/list",
            get(handlers::conversation::get_conversation_list),
        )
        .route(
            "/conversation/resume",
            post(handlers::conversation::post_conversation_resume),
        )
        // ── Share application state with all handlers ─────────────────────
        .with_state(state)
}
