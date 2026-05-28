//! Password authentication helper.
//!
//! All mutating endpoints (`POST /run`, `POST /audio`, `POST /commit`,
//! `POST /conversation/new`, `POST /conversation/resume`) require a
//! `password` field whose value must match `server.password` in the
//! configuration.  When that value is empty, authentication is disabled
//! entirely and every request is accepted.
//!
//! The check is intentionally simple: a shared secret compared in constant
//! time to avoid timing side-channels.  No sessions, no tokens, no hashing.
//! The expectation is that the connection itself is encrypted (TLS or a
//! reverse-proxy) so the secret is not exposed on the wire in plaintext.
//!
//! # Usage (inside a handler)
//!
//! ```rust,ignore
//! if let Some(err) = auth::check_password(&state.config.server, &form.password) {
//!     return err;
//! }
//! ```

use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use crate::{config::ServerConfig, templates};

/// Low-level validity check that returns `true` when the request should be
/// allowed.
///
/// Use this in handlers whose error format is not plain HTML (e.g. the audio
/// endpoint returns JSON, so it cannot use [`check_password`] directly).
///
/// Returns `true` when:
/// - Authentication is disabled (`server.password` is empty), OR
/// - `provided` exactly matches the configured password.
pub fn is_password_valid(config: &ServerConfig, provided: &str) -> bool {
    let expected = config.password.as_str();
    if expected.is_empty() {
        return true;
    }
    constant_time_eq(expected.as_bytes(), provided.as_bytes())
}

/// Check `provided` against the configured password.
///
/// Returns `None` when:
/// - Authentication is disabled (`server.password` is empty), OR
/// - `provided` exactly matches the configured password.
///
/// Returns `Some(403 Forbidden response)` when the password is set but
/// the provided value does not match.
///
/// Use the return value with an early-return guard in POST handlers:
/// ```rust,ignore
/// if let Some(err) = auth::check_password(&state.config.server, &form.password) {
///     return err;
/// }
/// ```
pub fn check_password(config: &ServerConfig, provided: &str) -> Option<Response> {
    let expected = config.password.as_str();
    if expected.is_empty() {
        // Auth disabled — always allow.
        return None;
    }

    // Constant-time comparison to avoid leaking the secret via timing.
    if !constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                Html(templates::render_error(403, "Incorrect password.")),
            )
                .into_response(),
        );
    }

    None
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Compare two byte slices in constant time to prevent timing attacks.
///
/// Returns `true` iff the slices are identical in length and content.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Early length mismatch — but still touch every byte of `a` so the
        // compiler cannot optimise away the work and leak timing via branch
        // prediction.
        let mut _dummy: u8 = 0;
        for byte in a {
            _dummy |= byte;
        }
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;

    fn make_config(password: &str) -> ServerConfig {
        ServerConfig {
            bind: "0.0.0.0:8080".into(),
            project_dir: ".".into(),
            run_timeout: 600,
            password: password.to_string(),
            tls: None,
        }
    }

    #[test]
    fn auth_disabled_when_password_empty() {
        let cfg = make_config("");
        assert!(check_password(&cfg, "anything").is_none());
        assert!(check_password(&cfg, "").is_none());
    }

    #[test]
    fn correct_password_accepted() {
        let cfg = make_config("s3cr3t");
        assert!(check_password(&cfg, "s3cr3t").is_none());
    }

    #[test]
    fn wrong_password_rejected() {
        let cfg = make_config("s3cr3t");
        let resp = check_password(&cfg, "wrong");
        assert!(resp.is_some());
        let r = resp.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn empty_provided_rejected_when_password_set() {
        let cfg = make_config("s3cr3t");
        assert!(check_password(&cfg, "").is_some());
    }
}
