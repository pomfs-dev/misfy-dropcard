//! PayPal webhooks: `POST /api/paypal/webhook`.
//!
//! 1. Verify the signature with PayPal (the raw body is forwarded byte for byte).
//! 2. Store the event; `paypal_event_id` is unique, so a redelivered event is processed only once.
//! 3. Apply it: captures confirm payments, payout item events settle payouts.
//! Unverified events are stored (for the console) but never applied.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde_json::Value;
use uuid::Uuid;

use crate::error::AppResult;
use crate::paypal::{parse_money, WebhookHeaders};
use crate::{payments, payouts, AppState};

fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
}

fn webhook_headers(h: &HeaderMap) -> Option<WebhookHeaders> {
    Some(WebhookHeaders {
        auth_algo: header(h, "paypal-auth-algo")?,
        cert_url: header(h, "paypal-cert-url")?,
        transmission_id: header(h, "paypal-transmission-id")?,
        transmission_sig: header(h, "paypal-transmission-sig")?,
        transmission_time: header(h, "paypal-transmission-time")?,
    })
}

fn s<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(|x| x.as_str())
}

pub async fn receive(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> (StatusCode, &'static str) {
    let Ok(raw) = std::str::from_utf8(&body) else {
        return (StatusCode::BAD_REQUEST, "body is not UTF-8");
    };
    let Ok(event) = serde_json::from_str::<Value>(raw) else {
        return (StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let (Some(event_id), Some(event_type)) = (s(&event, "/id"), s(&event, "/event_type")) else {
        return (StatusCode::BAD_REQUEST, "not a PayPal event");
    };
    let resource_id = s(&event, "/resource/id").map(str::to_string);
    let Some(pp) = st.paypal.as_deref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "PayPal not configured");
    };

    // Already fully processed? (PayPal redelivers until it gets a 2xx.)
    let seen: Option<(i64, Option<chrono::DateTime<chrono::Utc>>)> =
        match sqlx::query_as("SELECT id, processed_at FROM webhook_events WHERE paypal_event_id = $1")
            .bind(event_id)
            .fetch_optional(&st.db)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "webhook lookup failed");
                return (StatusCode::SERVICE_UNAVAILABLE, "try again");
            }
        };
    if let Some((_, Some(_))) = seen {
        return (StatusCode::OK, "duplicate");
    }

    let valid = match webhook_headers(&headers) {
        None => false,
        Some(h) => match pp.verify_webhook(&h, raw).await {
            Ok(v) => v,
            Err(e) if e.is_transient() => {
                tracing::warn!(error = %e, "webhook verification unavailable; PayPal will retry");
                return (StatusCode::SERVICE_UNAVAILABLE, "verification unavailable");
            }
            Err(e) => {
                tracing::warn!(error = %e, "webhook verification error");
                false
            }
        },
    };

    let row_id: i64 = match seen {
        Some((id, None)) => id,
        _ => match sqlx::query_scalar(
            "INSERT INTO webhook_events (paypal_event_id, event_type, resource_id, signature_valid, payload)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (paypal_event_id) DO UPDATE SET signature_valid = EXCLUDED.signature_valid
             RETURNING id",
        )
        .bind(event_id)
        .bind(event_type)
        .bind(&resource_id)
        .bind(valid)
        .bind(&event)
        .fetch_one(&st.db)
        .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(error = %e, "webhook insert failed");
                return (StatusCode::SERVICE_UNAVAILABLE, "try again");
            }
        },
    };

    if !valid {
        let _ = sqlx::query(
            "UPDATE webhook_events SET processed_at = now(), outcome = 'rejected: signature verification failed'
              WHERE id = $1",
        )
        .bind(row_id)
        .execute(&st.db)
        .await;
        tracing::warn!(%event_id, %event_type, "rejected webhook with an invalid signature");
        return (StatusCode::BAD_REQUEST, "invalid signature");
    }

    match apply(&st, event_type, &event["resource"]).await {
        Ok((outcome, booking_id)) => {
            let _ = sqlx::query(
                "UPDATE webhook_events SET processed_at = now(), outcome = $2, booking_id = $3, error = NULL WHERE id = $1",
            )
            .bind(row_id)
            .bind(&outcome)
            .bind(booking_id)
            .execute(&st.db)
            .await;
            tracing::info!(%event_id, %event_type, %outcome, "webhook processed");
            (StatusCode::OK, "ok")
        }
        Err(e) => {
            tracing::error!(%event_id, %event_type, error = %e, "webhook processing failed; PayPal will retry");
            let _ = sqlx::query("UPDATE webhook_events SET error = $2 WHERE id = $1")
                .bind(row_id)
                .bind(e.to_string())
                .execute(&st.db)
                .await;
            (StatusCode::INTERNAL_SERVER_ERROR, "processing failed")
        }
    }
}

/// Applies a verified event. Returns a human-readable outcome and the booking it concerns.
async fn apply(st: &AppState, event_type: &str, r: &Value) -> AppResult<(String, Option<Uuid>)> {
    match event_type {
        "PAYMENT.CAPTURE.COMPLETED" => {
            let capture_id = s(r, "/id").unwrap_or_default();
            let gross = s(r, "/amount/value").and_then(parse_money);
            let fee = s(r, "/seller_receivable_breakdown/paypal_fee/value").and_then(parse_money).unwrap_or(0);
            let order = payments::find_order(
                &st.db,
                s(r, "/custom_id"),
                s(r, "/supplementary_data/related_ids/order_id"),
            )
            .await?;
            match (order, gross) {
                (Some((order_id, booking_id)), Some(gross)) => {
                    let changed = payments::apply_capture(&st.db, order_id, capture_id, gross, fee, "webhook").await?;
                    let outcome = if changed {
                        "capture recorded; ledger updated; booking confirmed".to_string()
                    } else {
                        "capture already recorded".to_string()
                    };
                    Ok((outcome, Some(booking_id)))
                }
                (None, _) => Ok(("ignored: capture for an unknown order".into(), None)),
                (_, None) => Ok(("ignored: capture without amount".into(), None)),
            }
        }
        "CHECKOUT.ORDER.APPROVED" => {
            let order = payments::find_order(&st.db, s(r, "/purchase_units/0/custom_id"), s(r, "/id")).await?;
            match order {
                Some((order_id, booking_id)) => {
                    sqlx::query("UPDATE orders SET status = 'approved' WHERE id = $1 AND status = 'created'")
                        .bind(order_id)
                        .execute(&st.db)
                        .await?;
                    Ok(("buyer approved; waiting for capture".into(), Some(booking_id)))
                }
                None => Ok(("ignored: unknown order".into(), None)),
            }
        }
        "PAYMENT.CAPTURE.DENIED" | "PAYMENT.CAPTURE.DECLINED" => {
            let order = payments::find_order(
                &st.db,
                s(r, "/custom_id"),
                s(r, "/supplementary_data/related_ids/order_id"),
            )
            .await?;
            match order {
                Some((order_id, booking_id)) => {
                    sqlx::query("UPDATE orders SET status = 'failed', last_error = $2 WHERE id = $1 AND status <> 'captured'")
                        .bind(order_id)
                        .bind(event_type)
                        .execute(&st.db)
                        .await?;
                    Ok(("capture denied; the promoter can pay again".into(), Some(booking_id)))
                }
                None => Ok(("ignored: unknown order".into(), None)),
            }
        }
        "PAYMENT.CAPTURE.PENDING" => Ok(("capture pending at PayPal".into(), None)),
        "PAYMENT.CAPTURE.REFUNDED" | "PAYMENT.CAPTURE.REVERSED" => {
            // The refund resource links "up" to the capture it refunds.
            let capture_id = r
                .get("links")
                .and_then(|l| l.as_array())
                .and_then(|links| {
                    links.iter().find(|l| l.get("rel").and_then(|x| x.as_str()) == Some("up"))
                })
                .and_then(|l| l.get("href").and_then(|h| h.as_str()))
                .and_then(|href| href.rsplit('/').next())
                .map(str::to_string);
            let amount = s(r, "/amount/value").and_then(parse_money);
            match (capture_id, amount) {
                (Some(cid), Some(cents)) => {
                    let booking = payments::apply_refund(&st.db, &cid, cents, s(r, "/id").unwrap_or("?")).await?;
                    Ok(("refund recorded".into(), booking))
                }
                _ => Ok(("ignored: refund without capture link".into(), None)),
            }
        }
        t if t.starts_with("PAYMENT.PAYOUTS-ITEM.") => {
            let sender_item_id = s(r, "/payout_item/sender_item_id").unwrap_or_default();
            let status = s(r, "/transaction_status").unwrap_or_default();
            let error = s(r, "/errors/name");
            let booking = payouts::apply_item_status(&st.db, sender_item_id, s(r, "/payout_item_id"), status, error).await?;
            Ok((format!("payout item {status}"), booking))
        }
        t if t.starts_with("PAYMENT.PAYOUTSBATCH.") => {
            // Item events carry the details; poll the batch now so the console updates quickly.
            if let (Some(pp), Some(batch)) = (st.paypal.as_deref(), s(r, "/batch_header/payout_batch_id")) {
                if let Ok(status) = pp.get_payout(batch).await {
                    for item in status.items {
                        payouts::apply_item_status(
                            &st.db,
                            &item.sender_item_id,
                            item.payout_item_id.as_deref(),
                            &item.transaction_status,
                            item.error.as_deref(),
                        )
                        .await?;
                    }
                }
            }
            Ok((format!("payout batch {}", s(r, "/batch_header/batch_status").unwrap_or("?")), None))
        }
        _ => Ok(("ignored: event type not used".into(), None)),
    }
}
