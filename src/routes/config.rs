use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::AppState;

#[derive(Debug, Serialize)]
pub struct PublicConfig {
    /// Public client id for the PayPal JS SDK (sandbox). `null` until configured.
    pub paypal_client_id: Option<String>,
    pub paypal_env: &'static str,
    pub currency: &'static str,
    pub llm_enabled: bool,
    pub webhook_verification: bool,
}

/// GET /api/config — only public values.
pub async fn config(State(st): State<AppState>) -> Json<PublicConfig> {
    Json(PublicConfig {
        paypal_client_id: st.paypal.as_ref().map(|p| p.client_id().to_string()),
        paypal_env: "sandbox",
        currency: "USD",
        llm_enabled: st.cfg.llm.is_some(),
        webhook_verification: st.paypal.as_ref().map(|p| p.webhook_configured()).unwrap_or(false),
    })
}
