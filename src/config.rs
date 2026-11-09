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
}

#[derive(Clone, Debug, Default)]
pub struct SandboxReceivers {
    pub venue: Option<String>,
    pub artist: Option<String>,
    pub promoter: Option<String>,
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
        })
    }
}
