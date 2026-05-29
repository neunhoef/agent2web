// Future-milestone code (state fields, helper methods, template functions) is
// defined now so later milestones can build on a clean foundation. Suppress
// dead-code warnings for these intentional stubs.
#![allow(dead_code)]

mod agent;
mod auth;
mod config;
mod diff;
mod handlers;
mod router;
mod state;
mod stt;
mod templates;

use clap::Parser;
use tracing::info;

/// `agent2web` — mobile-friendly web control panel for a local ForgeCode agent.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Path to the TOML configuration file.
    /// When omitted the program runs with built-in defaults (TLS enabled,
    /// certificate and key at ~/.tls/cert.pem and ~/.tls/key.pem).
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise structured logging.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agent2web=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();

    let config = match args.config {
        Some(ref path) => {
            info!(config = %path, "Loading configuration from file");
            config::Config::load(path)?
        }
        None => {
            info!("No configuration file specified, using built-in defaults");
            config::Config::load_defaults()
        }
    };

    let bind_addr = config.server.bind.clone();
    let project_dir = config.server.project_dir.clone();
    let max_history = config.diff.max_history;
    let tls_config = config.server.tls.clone();

    info!(
        bind = %bind_addr,
        project_dir = %project_dir,
        "Starting agent2web"
    );

    // Build shared state.
    let state = state::AppState::new(config);

    // Populate recent commit history from git log so the diff history nav
    // is populated immediately on first page load.
    match diff::get_recent_commits(&project_dir, max_history).await {
        Ok(commits) => {
            let count = commits.len();
            *state.commit_history.lock().expect("commit_history mutex") = commits;
            info!(count, "Loaded initial commit history");
        }
        Err(e) => {
            // Not fatal — project_dir may not be a git repository yet, or
            // the repository may have no commits.
            tracing::warn!(error = %e, "Could not load initial commit history (non-fatal)");
        }
    }

    // Build the axum router.
    let app = router::build(state);

    // Bind and serve — TLS when configured, plain HTTP otherwise.
    let addr: std::net::SocketAddr = bind_addr
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid bind address '{}': {}", bind_addr, e))?;

    match tls_config {
        Some(ref tls) if tls.enabled => {
            // ── TLS listener (axum-server + rustls) ──────────────────────────
            if tls.cert.is_empty() || tls.key.is_empty() {
                anyhow::bail!(
                    "TLS is enabled but [server.tls] cert or key path is empty. \
                     Run `bash tls/generate.sh` to create a self-signed certificate."
                );
            }

            info!(
                addr = %addr,
                cert = %tls.cert,
                key  = %tls.key,
                "Listening on https://{} (TLS)", addr
            );

            let rustls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "Failed to load TLS certificate '{}' / key '{}': {}",
                            tls.cert,
                            tls.key,
                            e
                        )
                    })?;

            axum_server::bind_rustls(addr, rustls_config)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            // ── Plain HTTP listener ────────────────────────────────────────────
            let listener = tokio::net::TcpListener::bind(addr).await?;
            info!("Listening on http://{}", listener.local_addr()?);
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}
