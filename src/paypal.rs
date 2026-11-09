//! Minimal PayPal REST client for the sandbox: OAuth 2.0 client credentials, Orders v2,
//! webhook signature verification and Payouts.
//!
//! Idempotency: every state-changing call sends a `PayPal-Request-Id`, so a retried request
//! (after a timeout, a double click or a crash) returns the original result instead of
//! creating a second order, capture or payout batch.

use std::time::{Duration, Instant};

use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::config::PayPalConfig;

pub struct PayPal {
    http: Client,
    cfg: PayPalConfig,
    token: Mutex<Option<(String, Instant)>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PayPalError {
    #[error("PayPal API {status}: {name} {message} {issues:?} (debug_id {debug_id:?})")]
    Api { status: u16, name: String, message: String, issues: Vec<String>, debug_id: Option<String> },
    #[error("PayPal request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("unexpected PayPal response: {0}")]
    Unexpected(String),
}

impl PayPalError {
    /// True if PayPal reported this issue code (e.g. `ORDER_ALREADY_CAPTURED`, `INSTRUMENT_DECLINED`).
    pub fn has_issue(&self, code: &str) -> bool {
        matches!(self, PayPalError::Api { issues, name, .. } if name == code || issues.iter().any(|i| i == code))
    }
    /// Network errors and 5xx are worth retrying; 4xx are not.
    pub fn is_transient(&self) -> bool {
        match self {
            PayPalError::Http(_) => true,
            PayPalError::Api { status, .. } => *status >= 500 || *status == 429,
            PayPalError::Unexpected(_) => false,
        }
    }
}

type PResult<T> = Result<T, PayPalError>;

/// What we need from an order after a capture (from the capture response or GET order).
#[derive(Debug, Clone, Default, Serialize)]
pub struct CaptureInfo {
    pub order_status: String,
    pub capture_id: Option<String>,
    pub capture_status: Option<String>,
    pub gross_cents: Option<i64>,
    pub fee_cents: Option<i64>,
    pub payer_email: Option<String>,
    pub payer_country: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PayoutItemRequest {
    pub sender_item_id: String,
    pub receiver: String,
    pub amount_cents: i64,
    pub note: String,
}

#[derive(Debug, Clone, Default)]
pub struct PayoutItemStatus {
    pub sender_item_id: String,
    pub payout_item_id: Option<String>,
    pub transaction_status: String,
    pub error: Option<String>,
    pub fee_cents: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct PayoutBatchStatus {
    pub payout_batch_id: String,
    pub batch_status: String,
    pub items: Vec<PayoutItemStatus>,
}

/// Headers PayPal sends with a webhook; all are needed for signature verification.
#[derive(Debug, Clone)]
pub struct WebhookHeaders {
    pub auth_algo: String,
    pub cert_url: String,
    pub transmission_id: String,
    pub transmission_sig: String,
    pub transmission_time: String,
}

/// Cents to a PayPal amount string ("1234" -> "12.34"). Amounts are never negative here.
pub fn format_money(cents: i64) -> String {
    debug_assert!(cents >= 0);
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// A PayPal amount string to cents ("12.3" -> 1230, "7" -> 700). Rejects more than 2 decimals.
pub fn parse_money(value: &str) -> Option<i64> {
    let value = value.trim();
    let (whole, frac) = match value.split_once('.') {
        Some((w, f)) => (w, f),
        None => (value, ""),
    };
    if whole.is_empty() || !whole.chars().all(|c| c.is_ascii_digit()) || frac.len() > 2 || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let whole: i64 = whole.parse().ok()?;
    let frac: i64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().ok()? * 10,
        _ => frac.parse().ok()?,
    };
    whole.checked_mul(100)?.checked_add(frac)
}

fn str_at(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(|x| x.as_str()).map(str::to_string)
}

fn money_at(v: &Value, ptr: &str) -> Option<i64> {
    v.pointer(ptr).and_then(|x| x.as_str()).and_then(parse_money)
}

/// Pulls payer and first capture out of an order (capture response or GET order).
pub fn capture_info_from_order(order: &Value) -> CaptureInfo {
    let cap = "/purchase_units/0/payments/captures/0";
    CaptureInfo {
        order_status: str_at(order, "/status").unwrap_or_default(),
        capture_id: str_at(order, &format!("{cap}/id")),
        capture_status: str_at(order, &format!("{cap}/status")),
        gross_cents: money_at(order, &format!("{cap}/amount/value")),
        fee_cents: money_at(order, &format!("{cap}/seller_receivable_breakdown/paypal_fee/value")),
        payer_email: str_at(order, "/payer/email_address"),
        payer_country: str_at(order, "/payer/address/country_code"),
    }
}

impl PayPal {
    pub fn new(cfg: PayPalConfig) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(25))
            .user_agent("misfy-dropcard/0.1")
            .build()
            .expect("http client");
        Self { http, cfg, token: Mutex::new(None) }
    }

    pub fn client_id(&self) -> &str {
        &self.cfg.client_id
    }

    pub fn webhook_configured(&self) -> bool {
        self.cfg.webhook_id.is_some()
    }

    async fn access_token(&self) -> PResult<String> {
        let mut guard = self.token.lock().await;
        if let Some((token, expires)) = guard.as_ref() {
            if Instant::now() < *expires {
                return Ok(token.clone());
            }
        }
        #[derive(Deserialize)]
        struct Token {
            access_token: String,
            expires_in: u64,
        }
        let res = self
            .http
            .post(format!("{}/v1/oauth2/token", self.cfg.base_url))
            .basic_auth(&self.cfg.client_id, Some(&self.cfg.client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await?;
        let token: Token = Self::check(res).await?.json().await?;
        let expires = Instant::now() + Duration::from_secs(token.expires_in.saturating_sub(120).max(60));
        *guard = Some((token.access_token.clone(), expires));
        Ok(token.access_token)
    }

    async fn check(res: reqwest::Response) -> PResult<reqwest::Response> {
        if res.status().is_success() {
            return Ok(res);
        }
        let status = res.status().as_u16();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        let issues = body
            .get("details")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|d| d.get("issue").and_then(|i| i.as_str()).map(str::to_string)).collect())
            .unwrap_or_default();
        Err(PayPalError::Api {
            status,
            name: str_at(&body, "/name").or_else(|| str_at(&body, "/error")).unwrap_or_default(),
            message: str_at(&body, "/message").or_else(|| str_at(&body, "/error_description")).unwrap_or_default(),
            issues,
            debug_id: str_at(&body, "/debug_id"),
        })
    }

    /// Sends a request with a bearer token. `body = None` sends an empty JSON request (as PayPal expects
    /// for capture). `representation` asks Orders v2 to return the full resource.
    async fn send(
        &self,
        method: Method,
        path: &str,
        request_id: Option<&str>,
        body: Option<String>,
        representation: bool,
    ) -> PResult<Value> {
        let token = self.access_token().await?;
        let mut req = self
            .http
            .request(method, format!("{}{}", self.cfg.base_url, path))
            .bearer_auth(token)
            .header(CONTENT_TYPE, "application/json");
        if representation {
            req = req.header("Prefer", "return=representation");
        }
        if let Some(id) = request_id {
            req = req.header("PayPal-Request-Id", id);
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        let res = Self::check(req.send().await?).await?;
        let text = res.text().await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| PayPalError::Unexpected(format!("invalid JSON: {e}")))
    }

    // ---------------------------------------------------------------- Orders v2

    /// Creates a CAPTURE order for one purchase unit. `custom_id` carries our internal order id and
    /// comes back on the capture (and its webhook), which is how webhooks are matched to orders.
    pub async fn create_order(
        &self,
        request_id: &str,
        custom_id: &str,
        description: &str,
        amount_cents: i64,
        currency: &str,
    ) -> PResult<(String, String)> {
        let body = json!({
            "intent": "CAPTURE",
            "purchase_units": [{
                "reference_id": custom_id,
                "custom_id": custom_id,
                "description": description,
                "amount": { "currency_code": currency, "value": format_money(amount_cents) }
            }],
            "application_context": {
                "brand_name": "MiSFY DropCard",
                "shipping_preference": "NO_SHIPPING",
                "user_action": "PAY_NOW"
            }
        });
        let v = self
            .send(Method::POST, "/v2/checkout/orders", Some(request_id), Some(body.to_string()), true)
            .await?;
        let id = str_at(&v, "/id").ok_or_else(|| PayPalError::Unexpected("order without id".into()))?;
        Ok((id, str_at(&v, "/status").unwrap_or_default()))
    }

    pub async fn capture_order(&self, paypal_order_id: &str, request_id: &str) -> PResult<CaptureInfo> {
        let v = self
            .send(
                Method::POST,
                &format!("/v2/checkout/orders/{paypal_order_id}/capture"),
                Some(request_id),
                None,
                true,
            )
            .await?;
        Ok(capture_info_from_order(&v))
    }

    pub async fn get_order(&self, paypal_order_id: &str) -> PResult<CaptureInfo> {
        let v = self
            .send(Method::GET, &format!("/v2/checkout/orders/{paypal_order_id}"), None, None, false)
            .await?;
        Ok(capture_info_from_order(&v))
    }

    // ---------------------------------------------------------------- Webhooks

    /// Asks PayPal whether a webhook really came from PayPal for our webhook id.
    /// The event is passed through **byte for byte** (re-serialising it would break the signature).
    pub async fn verify_webhook(&self, h: &WebhookHeaders, raw_body: &str) -> PResult<bool> {
        let Some(webhook_id) = self.cfg.webhook_id.as_deref() else {
            return Ok(false);
        };
        let event: &RawValue = serde_json::from_str(raw_body)
            .map_err(|e| PayPalError::Unexpected(format!("webhook body is not JSON: {e}")))?;
        #[derive(Serialize)]
        struct VerifyRequest<'a> {
            auth_algo: &'a str,
            cert_url: &'a str,
            transmission_id: &'a str,
            transmission_sig: &'a str,
            transmission_time: &'a str,
            webhook_id: &'a str,
            webhook_event: &'a RawValue,
        }
        let body = serde_json::to_string(&VerifyRequest {
            auth_algo: &h.auth_algo,
            cert_url: &h.cert_url,
            transmission_id: &h.transmission_id,
            transmission_sig: &h.transmission_sig,
            transmission_time: &h.transmission_time,
            webhook_id,
            webhook_event: event,
        })
        .map_err(|e| PayPalError::Unexpected(e.to_string()))?;
        let v = self
            .send(Method::POST, "/v1/notifications/verify-webhook-signature", None, Some(body), false)
            .await?;
        Ok(str_at(&v, "/verification_status").as_deref() == Some("SUCCESS"))
    }

    /// Makes sure the app has a webhook pointing at `url` (all event types; the server ignores the ones it
    /// does not use). Returns the webhook id for `PAYPAL_WEBHOOK_ID`, and whether it was created now.
    pub async fn ensure_webhook(&self, url: &str) -> PResult<(String, bool)> {
        let list = self.send(Method::GET, "/v1/notifications/webhooks", None, None, false).await?;
        if let Some(existing) = list
            .get("webhooks")
            .and_then(|w| w.as_array())
            .and_then(|hooks| hooks.iter().find(|h| h.get("url").and_then(|u| u.as_str()) == Some(url)))
        {
            let id = str_at(existing, "/id").ok_or_else(|| PayPalError::Unexpected("webhook without id".into()))?;
            return Ok((id, false));
        }
        let body = json!({ "url": url, "event_types": [{ "name": "*" }] });
        let created = self
            .send(Method::POST, "/v1/notifications/webhooks", None, Some(body.to_string()), false)
            .await?;
        let id = str_at(&created, "/id").ok_or_else(|| PayPalError::Unexpected("webhook without id".into()))?;
        Ok((id, true))
    }

    // ---------------------------------------------------------------- Payouts

    /// Sends one payout batch. `sender_batch_id` is unique per batch and doubles as the request id,
    /// so retrying the same batch can never pay twice.
    pub async fn create_payout(
        &self,
        sender_batch_id: &str,
        email_subject: &str,
        items: &[PayoutItemRequest],
        currency: &str,
    ) -> PResult<(String, String)> {
        let body = json!({
            "sender_batch_header": {
                "sender_batch_id": sender_batch_id,
                "email_subject": email_subject,
                "email_message": "Your share of the show, paid out by MiSFY DropCard (PayPal sandbox)."
            },
            "items": items.iter().map(|i| json!({
                "recipient_type": "EMAIL",
                "receiver": i.receiver,
                "amount": { "value": format_money(i.amount_cents), "currency": currency },
                "note": i.note,
                "sender_item_id": i.sender_item_id
            })).collect::<Vec<_>>()
        });
        let v = self
            .send(Method::POST, "/v1/payments/payouts", Some(sender_batch_id), Some(body.to_string()), false)
            .await?;
        let id = str_at(&v, "/batch_header/payout_batch_id")
            .ok_or_else(|| PayPalError::Unexpected("payout without batch id".into()))?;
        Ok((id, str_at(&v, "/batch_header/batch_status").unwrap_or_default()))
    }

    pub async fn get_payout(&self, payout_batch_id: &str) -> PResult<PayoutBatchStatus> {
        let v = self
            .send(
                Method::GET,
                &format!("/v1/payments/payouts/{payout_batch_id}?page_size=100"),
                None,
                None,
                false,
            )
            .await?;
        let items = v
            .get("items")
            .and_then(|i| i.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|it| PayoutItemStatus {
                        sender_item_id: str_at(it, "/payout_item/sender_item_id").unwrap_or_default(),
                        payout_item_id: str_at(it, "/payout_item_id"),
                        transaction_status: str_at(it, "/transaction_status").unwrap_or_default(),
                        error: str_at(it, "/errors/name").map(|n| match str_at(it, "/errors/message") {
                            Some(m) => format!("{n}: {m}"),
                            None => n,
                        }),
                        fee_cents: money_at(it, "/payout_item_fee/value"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(PayoutBatchStatus {
            payout_batch_id: payout_batch_id.to_string(),
            batch_status: str_at(&v, "/batch_header/batch_status").unwrap_or_default(),
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_round_trip() {
        assert_eq!(format_money(0), "0.00");
        assert_eq!(format_money(5), "0.05");
        assert_eq!(format_money(123_456), "1234.56");
        assert_eq!(parse_money("1234.56"), Some(123_456));
        assert_eq!(parse_money("12.3"), Some(1_230));
        assert_eq!(parse_money("7"), Some(700));
        assert_eq!(parse_money("0.05"), Some(5));
        assert_eq!(parse_money("1.234"), None);
        assert_eq!(parse_money("-1.00"), None);
        assert_eq!(parse_money("abc"), None);
        assert_eq!(parse_money(""), None);
    }

    #[test]
    fn reads_capture_from_order() {
        let order = json!({
            "id": "5O190127TN364715T",
            "status": "COMPLETED",
            "payer": { "email_address": "buyer@example.com", "address": { "country_code": "JP" } },
            "purchase_units": [{ "payments": { "captures": [{
                "id": "3C679366HH908993F", "status": "COMPLETED",
                "amount": { "currency_code": "USD", "value": "720.00" },
                "seller_receivable_breakdown": { "paypal_fee": { "currency_code": "USD", "value": "25.62" } }
            }]}}]
        });
        let c = capture_info_from_order(&order);
        assert_eq!(c.order_status, "COMPLETED");
        assert_eq!(c.capture_id.as_deref(), Some("3C679366HH908993F"));
        assert_eq!(c.gross_cents, Some(72_000));
        assert_eq!(c.fee_cents, Some(2_562));
        assert_eq!(c.payer_country.as_deref(), Some("JP"));
    }

    #[test]
    fn webhook_event_is_forwarded_byte_for_byte() {
        // Odd spacing and key order must survive, or PayPal's signature check fails.
        let raw = r#"{"id":"WH-1",  "event_type":"PAYMENT.CAPTURE.COMPLETED","resource":{"b":1,"a":2}}"#;
        let event: &RawValue = serde_json::from_str(raw).unwrap();
        #[derive(Serialize)]
        struct Wrap<'a> {
            webhook_event: &'a RawValue,
        }
        let out = serde_json::to_string(&Wrap { webhook_event: event }).unwrap();
        assert_eq!(out, format!(r#"{{"webhook_event":{raw}}}"#));
    }

    #[test]
    fn issue_detection() {
        let e = PayPalError::Api {
            status: 422,
            name: "UNPROCESSABLE_ENTITY".into(),
            message: "".into(),
            issues: vec!["ORDER_ALREADY_CAPTURED".into()],
            debug_id: None,
        };
        assert!(e.has_issue("ORDER_ALREADY_CAPTURED"));
        assert!(!e.has_issue("INSTRUMENT_DECLINED"));
        assert!(!e.is_transient());
    }
}
