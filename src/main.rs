//! MiSFY DropCard — a link card for live-music venues where an AI agent handles booking
//! and PayPal handles the deposit, ticket payments and the payout after the show.
//!
//! Commands:
//!   misfy-dropcard serve   run the web server (default)
//!   misfy-dropcard seed    create sample venues/artists and rebuild sample bookings around today

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing_subscriber::EnvFilter;

mod clock;
mod config;
mod error;
mod routes;
mod session;
mod settlement;

use config::Config;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub cfg: Arc<Config>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info")),
        )
        .init();

    let cfg = Config::from_env()?;
    let db = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&cfg.database_url)
        .await
        .context("connecting to Postgres")?;
    sqlx::migrate!("./migrations").run(&db).await.context("running migrations")?;

    let command = std::env::args().nth(1).unwrap_or_else(|| "serve".to_string());
    match command.as_str() {
        "serve" => serve(db, cfg).await,
        "seed" => seed(&db, &cfg).await,
        other => anyhow::bail!("unknown command `{other}` (expected: serve | seed)"),
    }
}

async fn serve(db: PgPool, cfg: Config) -> Result<()> {
    let port = cfg.port;
    let state = AppState { db, cfg: Arc::new(cfg) };
    let app = routes::router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

/// Sample data: idempotent. Rebuilds only the shared sample bookings; visitors' bookings are kept.
async fn seed(db: &PgPool, cfg: &Config) -> Result<()> {
    sqlx::query("SELECT seed_entities()").execute(db).await?;
    let r = &cfg.sandbox_receivers;
    if let Some(email) = &r.venue {
        sqlx::query("UPDATE venues SET payout_email = $1").bind(email).execute(db).await?;
    }
    if let Some(email) = &r.artist {
        sqlx::query("UPDATE artists SET payout_email = $1").bind(email).execute(db).await?;
    }
    if let Some(email) = &r.promoter {
        sqlx::query("UPDATE promoters SET payout_email = $1").bind(email).execute(db).await?;
    }
    let summary: String = sqlx::query_scalar("SELECT refresh_sample_data()").fetch_one(db).await?;
    tracing::info!("{summary}");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
