//! Handlers for conversation management endpoints.
//!
//! | Method | Path                     | Action                                  |
//! |--------|--------------------------|-----------------------------------------|
//! | POST   | `/conversation/new`      | Create a new conversation via forge     |
//! | GET    | `/conversation/list`     | Return HTML fragment of history         |
//! | POST   | `/conversation/resume`   | Resume a past conversation by ID        |

use std::sync::Arc;

use axum::{
    Form,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use tracing::{info, warn};

use crate::{
    agent,
    state::{AppState, ConvEntry},
    templates,
};

// ── POST /conversation/new ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct NewConvForm {
    /// Optional short description for the new conversation.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub password: String,
}

/// `POST /conversation/new` — run `forge conversation new`, record the ID,
/// and redirect to `/`.
pub async fn post_conversation_new(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NewConvForm>,
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

    // ── Reject if a run is already in progress ────────────────────────────
    {
        let run = state.run.lock().expect("run mutex poisoned");
        if run.status.is_running() {
            return (
                StatusCode::CONFLICT,
                Html(templates::render_error(
                    409,
                    "Cannot start a new conversation while a run is in progress.",
                )),
            )
                .into_response();
        }
    }

    let forge_binary = state.config.forge.binary.clone();

    match agent::create_new_conversation(&forge_binary).await {
        Ok(id) => {
            info!(conv_id = %id, "Created new forge conversation");
            let label = if form.label.trim().is_empty() {
                String::new()
            } else {
                form.label.trim().to_string()
            };
            let entry = ConvEntry::new(id.clone(), label);
            let mut conv = state.conv.lock().expect("conv mutex poisoned");
            conv.active_id = Some(id);
            conv.history.push(entry);
        }
        Err(e) => {
            warn!(error = %e, "Failed to create new forge conversation");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(templates::render_error(
                    500,
                    &format!("Failed to create new conversation: {e}"),
                )),
            )
                .into_response();
        }
    }

    Redirect::to("/").into_response()
}

// ── GET /conversation/list ─────────────────────────────────────────────────────

/// `GET /conversation/list` — return the conversation history as an HTML
/// fragment (HTMX swap target).
pub async fn get_conversation_list(State(state): State<Arc<AppState>>) -> Html<String> {
    let conv = state.conv.lock().expect("conv mutex poisoned");
    Html(templates::render_conv_list(&conv))
}

// ── POST /conversation/resume ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ResumeConvForm {
    /// The forge conversation ID to resume.
    pub id: String,
    #[serde(default)]
    pub password: String,
}

/// `POST /conversation/resume` — set the active conversation to a previously
/// recorded one and redirect to `/`.
pub async fn post_conversation_resume(
    State(state): State<Arc<AppState>>,
    Form(form): Form<ResumeConvForm>,
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

    // ── Reject if a run is already in progress ────────────────────────────
    {
        let run = state.run.lock().expect("run mutex poisoned");
        if run.status.is_running() {
            return (
                StatusCode::CONFLICT,
                Html(templates::render_error(
                    409,
                    "Cannot switch conversations while a run is in progress.",
                )),
            )
                .into_response();
        }
    }

    // ── Validate the ID exists in history ────────────────────────────────
    let id = form.id.trim().to_string();
    {
        let conv = state.conv.lock().expect("conv mutex poisoned");
        if !conv.history.iter().any(|e| e.id == id) {
            return (
                StatusCode::BAD_REQUEST,
                Html(templates::render_error(
                    400,
                    "Conversation ID not found in history.",
                )),
            )
                .into_response();
        }
    }

    info!(conv_id = %id, "Resuming conversation");

    {
        let mut conv = state.conv.lock().expect("conv mutex poisoned");
        conv.active_id = Some(id);
    }

    Redirect::to("/").into_response()
}
