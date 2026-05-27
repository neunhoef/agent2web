//! Handler for `GET /stream` — Server-Sent Events live output stream.
//!
//! Each event carries one line of agent stdout/stderr.  The special sentinel
//! `"__DONE__"` signals run completion and causes the browser to reload the
//! page.
//!
//! If the run has already finished when the client connects, a single
//! `__DONE__` event is sent immediately so the client reloads right away.

use std::sync::Arc;

use axum::{
    extract::State,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures::stream::{self, StreamExt};
use tokio_stream::wrappers::BroadcastStream;
use tracing::debug;

use crate::state::AppState;

/// `GET /stream` — subscribe to the SSE broadcast channel and forward events
/// to the client.
pub async fn get_stream(State(state): State<Arc<AppState>>) -> Response {
    let is_running = state
        .run
        .lock()
        .expect("run mutex poisoned")
        .status
        .is_running();

    if !is_running {
        // Run is already done (or hasn't started): send a single __DONE__ so
        // the page reloads immediately if it was waiting.
        debug!("SSE connection: run not active — sending immediate __DONE__");
        let s =
            stream::once(async { Ok::<Event, anyhow::Error>(Event::default().data("__DONE__")) });
        return Sse::new(s).into_response();
    }

    debug!("SSE connection: subscribing to broadcast channel");

    let rx = state.sse_tx.subscribe();
    let broadcast_stream = BroadcastStream::new(rx).map(|result| match result {
        Ok(line) => Ok(Event::default().data(line)),
        // Lagged: some messages were dropped from the ring buffer.  This is
        // non-fatal; just skip the dropped events.
        Err(e) => Err(anyhow::anyhow!("SSE broadcast lag: {e}")),
    });

    Sse::new(broadcast_stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}
