use axum::response::IntoResponse;

/// `GET /health` — simple liveness probe for uptime monitoring.
pub async fn get_health() -> impl IntoResponse {
    "OK\n"
}
