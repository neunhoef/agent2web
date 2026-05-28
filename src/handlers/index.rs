use std::sync::Arc;

use axum::{extract::State, response::Html};

use crate::{state::AppState, templates};

/// `GET /` — render the full UI shell.
pub async fn get_index(State(state): State<Arc<AppState>>) -> Html<String> {
    let run = state.run.lock().expect("run mutex poisoned");
    let conv = state.conv.lock().expect("conv mutex poisoned");
    let prompts = state
        .prompts_since_commit
        .lock()
        .expect("prompts mutex poisoned");
    let commit_history = state
        .commit_history
        .lock()
        .expect("commit_history mutex poisoned");

    let password_enabled = !state.config.server.password.is_empty();
    let project_dir = &state.config.server.project_dir;

    let html = templates::render_index(
        &run,
        &conv,
        project_dir,
        password_enabled,
        prompts.len(),
        &commit_history,
    );

    Html(html)
}
