//! `primeview-server`: server-side price alerts for PrimeView.
//!
//! Runs the shared `primeview-alerts` engine against live prices (Binance
//! websocket, Twelve Data polling), exposes a small REST + SSE API and posts
//! fired alerts to webhooks. See `server/README.md`.

mod api;
mod feeds;
mod state;

#[cfg(test)]
mod tests;

use clap::Parser;
use state::{Settings, State};
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::sync::watch;

#[derive(Debug, Parser)]
#[command(
    name = "primeview-server",
    version,
    about = "PrimeView server-side price alerts"
)]
struct Cli {
    /// Address to listen on.
    #[arg(long, env = "PRIMEVIEW_BIND", default_value = "127.0.0.1:8787")]
    bind: SocketAddr,
    /// JSON file the alerts are stored in.
    #[arg(
        long,
        env = "PRIMEVIEW_DATA",
        default_value = "./primeview-data/alerts.json"
    )]
    data: PathBuf,
    /// Webhook URLs (comma separated) that receive every fired alert as JSON.
    #[arg(long, env = "PRIMEVIEW_WEBHOOKS", value_delimiter = ',')]
    webhooks: Vec<String>,
    /// Bearer token required on /api/alerts* and /api/events when set.
    #[arg(long, env = "PRIMEVIEW_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// Twelve Data API key (the `demo` key only covers a few symbols).
    #[arg(
        long,
        env = "PRIMEVIEW_TWELVEDATA_KEY",
        default_value = "demo",
        hide_env_values = true
    )]
    twelvedata_key: String,
    /// Seconds between Twelve Data polls.
    #[arg(long, env = "PRIMEVIEW_POLL_SECS", default_value_t = 60)]
    poll_secs: u64,
    /// Do not start price feeds (API only).
    #[arg(long, env = "PRIMEVIEW_NO_FEEDS")]
    no_feeds: bool,
}

fn settings_from(cli: &Cli) -> Result<Settings, String> {
    let mut webhooks = Vec::new();
    for w in cli
        .webhooks
        .iter()
        .map(|w| w.trim())
        .filter(|w| !w.is_empty())
    {
        let url = reqwest::Url::parse(w).map_err(|e| format!("invalid webhook URL {w:?}: {e}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("webhook URL must be http(s): {w:?}"));
        }
        webhooks.push(w.to_string());
    }
    Ok(Settings {
        data_path: cli.data.clone(),
        webhooks,
        token: cli.token.clone().filter(|t| !t.trim().is_empty()),
        twelvedata_key: cli.twelvedata_key.clone(),
        poll_secs: cli.poll_secs.max(1),
    })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let settings = match settings_from(&cli) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("{e}");
            std::process::exit(2);
        }
    };
    let engine = match state::load_engine(&settings.data_path) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("{e}");
            std::process::exit(1);
        }
    };
    tracing::info!(
        data = %settings.data_path.display(),
        alerts = engine.alerts().len(),
        webhooks = settings.webhooks.len(),
        auth = settings.token.is_some(),
        "loaded alerts"
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let state = State::new(settings, engine, shutdown_rx);
    // Write the file once so a bad data path fails fast.
    if let Err(e) = state.persist().await {
        tracing::error!(path = %state.settings.data_path.display(), "cannot write data file: {e}");
        std::process::exit(1);
    }

    if cli.no_feeds {
        tracing::warn!("price feeds disabled");
    } else {
        feeds::spawn(state.clone());
    }

    let listener = match tokio::net::TcpListener::bind(cli.bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("cannot bind {}: {e}", cli.bind);
            std::process::exit(1);
        }
    };
    tracing::info!("listening on http://{}", cli.bind);

    let app = api::router(state.clone());
    let shutdown = async move {
        wait_for_signal().await;
        tracing::info!("shutting down");
        let _ = shutdown_tx.send(true);
    };
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        tracing::error!("server error: {e}");
    }
    state.persist_logged().await;
    tracing::info!("bye");
}

/// Ctrl-C, or SIGTERM on Unix (systemd/Docker stop).
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
