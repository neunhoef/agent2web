// Future-milestone code (state fields, helper methods, template functions) is
// defined now so later milestones can build on a clean foundation. Suppress
// dead-code warnings for these intentional stubs.
#![allow(dead_code)]

mod config;
mod handlers;
mod router;
mod state;
mod templates;

use clap::Parser;
use tracing::info;

/// `agent2web` — mobile-friendly web control panel for a local ForgeCode agent.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Path to the TOML configuration file.
    #[arg(short, long, default_value = "agent2web.toml")]
    config: String,
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

    info!(config = %args.config, "Loading configuration");
    let config = config::Config::load(&args.config)?;

    let bind_addr = config.server.bind.clone();

    info!(
        bind = %bind_addr,
        project_dir = %config.server.project_dir,
        "Starting agent2web"
    );

    // Build shared state.
    let state = state::AppState::new(config);

    // Build the axum router.
    let app = router::build(state);

    // Bind and serve.
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!("Listening on http://{}", listener.local_addr()?);

    axum::serve(listener, app).await?;

    Ok(())
}
