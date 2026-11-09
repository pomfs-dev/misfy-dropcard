use std::path::PathBuf;

use anyhow::{Context, Result};

/// Runtime configuration, read from environment variables (see `.env.example`).
#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    /// Directory with the built frontend (`web/dist`).
    pub web_dir: PathBuf,
    /// Send the session cookie with `Secure` (true when served over HTTPS, e.g. on Render).
    pub cookie_secure: bool,
    /// Optional PayPal sandbox receiver accounts for payouts (override the sample addresses).
    pub sandbox_receivers: SandboxReceivers,
    /// PayPal sandbox app. `None` until the credentials are configured.
    pub paypal: Option<PayPalConfig>,
    /// LLM for the booking agent. `None` until an API key is configured.
    pub llm: Option<LlmConfig>,
    /// Background jobs (settlement, payouts, reconciliation) in the web process.
    pub run_jobs: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SandboxReceivers {
    pub venue: Option<String>,
    pub artist: Option<String>,
    pub promoter: Option<String>,
}

#[derive(Clone)]
pub struct PayPalConfig {
    pub base_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub webhook_id: Option<String>,
}

impl std::fmt::Debug for PayPalConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayPalConfig")
            .field("base_url", &self.base_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("webhook_id", &self.webhook_id)
            .finish()
    }
}

#[derive(Clone)]
pub struct LlmConfig {
    pub api_key: String,
    pub model: String,
    /// Safety cap on agent turns per day across all visitors (abuse protection, not a judge limit).
    pub daily_turn_cap: i32,
}

impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfig")
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("daily_turn_cap", &self.daily_turn_cap)
            .finish()
    }
}

fn opt(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url = opt("DATABASE_URL").context("DATABASE_URL is not set")?;
        let port = opt("PORT")
            .map(|p| p.parse::<u16>())
            .transpose()
            .context("PORT must be a number")?
            .unwrap_or(8080);
        let web_dir = opt("WEB_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("web/dist"));
        let cookie_secure = opt("COOKIE_SECURE").map(|v| v == "1" || v == "true").unwrap_or(false);

        let paypal = match (opt("PAYPAL_CLIENT_ID"), opt("PAYPAL_CLIENT_SECRET")) {
            (Some(client_id), Some(client_secret)) => Some(PayPalConfig {
                base_url: opt("PAYPAL_BASE_URL")
                    .unwrap_or_else(|| "https://api-m.sandbox.paypal.com".to_string()),
                client_id,
                client_secret,
                webhook_id: opt("PAYPAL_WEBHOOK_ID"),
            }),
            _ => None,
        };
        if let Some(p) = &paypal {
            anyhow::ensure!(
                p.base_url.contains("sandbox"),
                "PAYPAL_BASE_URL must point to the PayPal sandbox; this project never uses live keys"
            );
        }

        let llm = opt("ANTHROPIC_API_KEY").map(|api_key| LlmConfig {
            api_key,
            model: opt("LLM_MODEL").unwrap_or_else(|| "claude-sonnet-5-5".to_string()),
            daily_turn_cap: opt("LLM_DAILY_TURN_CAP").and_then(|v| v.parse().ok()).unwrap_or(3000),
        });

        Ok(Self {
            database_url,
            port,
            web_dir,
            cookie_secure,
            sandbox_receivers: SandboxReceivers {
                venue: opt("SANDBOX_VENUE_EMAIL"),
                artist: opt("SANDBOX_ARTIST_EMAIL"),
                promoter: opt("SANDBOX_PROMOTER_EMAIL"),
            },
            paypal,
            llm,
            run_jobs: opt("RUN_JOBS").map(|v| v != "0" && v != "false").unwrap_or(true),
        })
    }
}
