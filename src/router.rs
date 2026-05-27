use std::sync::Arc;

use axum::{Router, routing::get};

use crate::{handlers, state::AppState};

/// Build and return the axum `Router` with all routes registered.
///
/// Future milestones will add POST routes for `/run`, `/audio`, `/commit`,
/// `/conversation/*`, and GET routes for `/stream`, `/diff`.
pub fn build(state: Arc<AppState>) -> Router {
    Router::new()
        // Read-only / informational
        .route("/", get(handlers::index::get_index))
        .route("/health", get(handlers::health::get_health))
        // Share application state with all handlers.
        .with_state(state)
}
