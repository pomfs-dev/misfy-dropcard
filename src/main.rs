//! MiSFY DropCard — a link card for live-music venues where an AI agent handles booking
//! and PayPal handles the deposit, ticket payments and the payout after the show.
//!
//! Commands:
//!   misfy-dropcard serve   run the web server and background jobs (default)
//!   misfy-dropcard seed    create sample venues/artists and rebuild sample bookings around today
//!   misfy-dropcard cycle   run one settlement/payout/reconciliation cycle and exit (for cron)
//!   misfy-dropcard paypal-webhook https://<host>   create (or find) the PayPal sandbox webhook, print its id

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing_subscriber::EnvFilter;

mod agent;
mod bookings;
mod clock;
mod config;
mod error;
mod jobs;
mod ledger;
mod payments;
mod paypal;
mod payouts;
mod routes;
mod session;
mod settlement;
mod webhooks;

use config::Config;
use paypal::PayPal;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub cfg: Arc<Config>,
    pub paypal: Option<Arc<PayPal>>,
    pub llm: Option<Arc<agent::llm::Llm>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info")),
        )
        // Colors only in a terminal; plain text for Docker / Render log collectors.
        .with_ansi(std::io::stdout().is_terminal())
        .init();

    let cfg = Config::from_env()?;
    let db = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&cfg.database_url)
        .await
        .context("connecting to Postgres")?;
    sqlx::migrate!("./migrations").run(&db).await.context("running migrations")?;

    let paypal = cfg.paypal.clone().map(|p| Arc::new(PayPal::new(p)));
    let llm = cfg
        .llm
        .as_ref()
        .map(|l| Arc::new(agent::llm::Llm::new(&l.base_url, l.api_key.clone(), l.model.clone(), l.effort.clone())));
    tracing::info!(
        paypal = paypal.is_some(),
        webhook_verification = paypal.as_ref().map(|p| p.webhook_configured()).unwrap_or(false),
        llm = cfg.llm.as_ref().map(|l| format!("{} (effort {})", l.model, l.effort)).unwrap_or_else(|| "off".into()),
        "configuration"
    );
    let state = AppState { db, cfg: Arc::new(cfg), paypal, llm };

    let command = std::env::args().nth(1).unwrap_or_else(|| "serve".to_string());
    match command.as_str() {
        "serve" => serve(state).await,
        "seed" => seed(&state).await,
        "cycle" => jobs::run_cycle(&state, None).await,
        "paypal-webhook" => {
            let base = std::env::args()
                .nth(2)
                .context("usage: misfy-dropcard paypal-webhook https://<public-host>")?;
            anyhow::ensure!(base.starts_with("https://"), "PayPal webhooks need an https:// URL");
            let pp = state.paypal.as_deref().context("PAYPAL_CLIENT_ID / PAYPAL_CLIENT_SECRET are not set")?;
            let url = format!("{}/api/paypal/webhook", base.trim_end_matches('/'));
            let (id, created) = pp.ensure_webhook(&url).await?;
            println!("{} webhook for {url}", if created { "Created" } else { "Found existing" });
            println!("PAYPAL_WEBHOOK_ID={id}");
            Ok(())
        }
        other => anyhow::bail!("unknown command `{other}` (expected: serve | seed | cycle | paypal-webhook <url>)"),
    }
}

async fn serve(state: AppState) -> Result<()> {
    if state.cfg.run_jobs {
        jobs::spawn(state.clone());
    }
    let addr = SocketAddr::from(([0, 0, 0, 0], state.cfg.port));
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

/// Sample data: idempotent. Rebuilds only the shared sample bookings; visitors' bookings are kept.
async fn seed(state: &AppState) -> Result<()> {
    let db = &state.db;
    sqlx::query("SELECT seed_entities()").execute(db).await?;
    let r = &state.cfg.sandbox_receivers;
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
    // Visitors who have not been back for 30 days: forget their demo data.
    let purged = sqlx::query("DELETE FROM demo_sessions WHERE last_seen_at < now() - interval '30 days'")
        .execute(db)
        .await?
        .rows_affected();
    tracing::info!(purged_sessions = purged, "{summary}");
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
