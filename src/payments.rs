//! PayPal Orders v2 for deposits (and tickets).
//!
//! The rule: **a payment counts only when the server has confirmed it with PayPal** — through a
//! verified webhook (`PAYMENT.CAPTURE.COMPLETED`) or, if the webhook is late or missing, through
//! the reconciliation job asking the PayPal API directly. The browser's "approved" callback never
//! confirms anything; it only asks the server to capture.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::ledger::{self, Entry};
use crate::paypal::PayPal;
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct DepositOrderResponse {
    pub paypal_order_id: String,
    pub amount_cents: i64,
    pub currency: String,
}

#[derive(Debug, FromRow)]
struct OrderRow {
    id: Uuid,
    booking_id: Uuid,
    request_id: Uuid,
    paypal_order_id: Option<String>,
    amount_cents: i64,
    currency: String,
    status: String,
}

const ORDER_COLS: &str = "o.id, o.booking_id, o.request_id, o.paypal_order_id, o.amount_cents, o.currency, o.status";

fn paypal(st: &AppState) -> AppResult<&PayPal> {
    st.paypal
        .as_deref()
        .ok_or_else(|| AppError::Unavailable("The PayPal sandbox is not configured on this server yet.".into()))
}

/// Creates (or returns the existing) PayPal order for a booking's deposit.
/// Safe to call twice: the same booking never gets two live deposit orders.
pub async fn create_deposit_order(
    st: &AppState,
    session: Uuid,
    booking_id: Uuid,
    idempotency_key: Uuid,
) -> AppResult<DepositOrderResponse> {
    let mut tx = st.db.begin().await?;
    let booking: Option<(String, i64, String, String)> = sqlx::query_as(
        "SELECT status, deposit_cents, 'USD'::text AS currency, title FROM bookings
          WHERE id = $1 AND demo_session_id = $2 FOR UPDATE",
    )
    .bind(booking_id)
    .bind(session)
    .fetch_optional(&mut *tx)
    .await?;
    let (status, deposit_cents, currency, title) = booking.ok_or(AppError::NotFound)?;
    match status.as_str() {
        "approved" => {}
        "paid" | "completed" => return Err(AppError::Conflict("The deposit for this booking is already paid.".into())),
        _ => {
            return Err(AppError::Conflict(
                "The venue has not approved this booking yet, so no payment can be taken.".into(),
            ))
        }
    }
    if deposit_cents <= 0 {
        return Err(AppError::Conflict("This booking has no deposit to pay.".into()));
    }
    // Business rules first (so the agent and the UI get the real reason), then the PayPal check.
    let pp = paypal(st)?;

    let existing = sqlx::query_as::<_, OrderRow>(&format!(
        "SELECT {ORDER_COLS} FROM orders o
          WHERE o.booking_id = $1 AND o.kind = 'deposit' AND o.status IN ('created','approved','captured')
          FOR UPDATE"
    ))
    .bind(booking_id)
    .fetch_optional(&mut *tx)
    .await?;

    let order = match existing {
        Some(o) if o.status == "captured" => {
            return Err(AppError::Conflict("The deposit for this booking is already paid.".into()))
        }
        Some(o) => {
            if let Some(pid) = &o.paypal_order_id {
                tx.commit().await?;
                return Ok(DepositOrderResponse {
                    paypal_order_id: pid.clone(),
                    amount_cents: o.amount_cents,
                    currency: o.currency,
                });
            }
            o // a previous attempt stopped before PayPal answered: retry with the same request id
        }
        None => sqlx::query_as::<_, OrderRow>(
            "INSERT INTO orders AS o (booking_id, kind, request_id, amount_cents, currency)
             VALUES ($1, 'deposit', $2, $3, $4)
             RETURNING o.id, o.booking_id, o.request_id, o.paypal_order_id, o.amount_cents, o.currency, o.status",
        )
        .bind(booking_id)
        .bind(idempotency_key)
        .bind(deposit_cents)
        .bind(&currency)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            crate::bookings::conflict_on_constraint(e, "A payment is already being prepared for this booking — try again.")
        })?,
    };
    tx.commit().await?;

    let description = format!("Deposit (30%) — {title}");
    match pp
        .create_order(&order.request_id.to_string(), &order.id.to_string(), &description, order.amount_cents, &order.currency)
        .await
    {
        Ok((paypal_order_id, _status)) => {
            sqlx::query("UPDATE orders SET paypal_order_id = $2 WHERE id = $1")
                .bind(order.id)
                .bind(&paypal_order_id)
                .execute(&st.db)
                .await?;
            Ok(DepositOrderResponse { paypal_order_id, amount_cents: order.amount_cents, currency: order.currency })
        }
        Err(e) => {
            tracing::warn!(order = %order.id, error = %e, "PayPal create order failed");
            if !e.is_transient() {
                sqlx::query("UPDATE orders SET status = 'failed', last_error = $2 WHERE id = $1")
                    .bind(order.id)
                    .bind(e.to_string())
                    .execute(&st.db)
                    .await?;
            }
            Err(AppError::Upstream("PayPal could not create the order. Please try again.".into()))
        }
    }
}

/// Creates a PayPal order for tickets to a confirmed show of this visitor's session.
/// The `Idempotency-Key` identifies one purchase: sending it again returns the same order.
pub async fn create_ticket_order(
    st: &AppState,
    session: Uuid,
    booking_id: Uuid,
    quantity: i32,
    idempotency_key: Uuid,
) -> AppResult<DepositOrderResponse> {
    if !(1..=6).contains(&quantity) {
        return Err(AppError::BadRequest("you can buy 1 to 6 tickets at a time".into()));
    }
    let now = crate::clock::session_now(&st.db, session).await?;
    let show: Option<(String, Option<i64>, String, chrono::DateTime<Utc>)> = sqlx::query_as(
        "SELECT status, ticket_price_cents, title, starts_at FROM bookings WHERE id = $1 AND demo_session_id = $2",
    )
    .bind(booking_id)
    .bind(session)
    .fetch_optional(&st.db)
    .await?;
    let (status, price, title, starts_at) = show.ok_or(AppError::NotFound)?;
    if status != "paid" {
        return Err(AppError::Conflict("Tickets go on sale once the venue booking is confirmed (deposit paid).".into()));
    }
    if starts_at <= now {
        return Err(AppError::Conflict("This show has already started.".into()));
    }
    let price = price.ok_or_else(|| AppError::Conflict("This show has no ticket price.".into()))?;

    if let Some(o) = sqlx::query_as::<_, OrderRow>(&format!("SELECT {ORDER_COLS} FROM orders o WHERE o.request_id = $1"))
        .bind(idempotency_key)
        .fetch_optional(&st.db)
        .await?
    {
        if o.booking_id != booking_id {
            return Err(AppError::Conflict("This Idempotency-Key was used for another purchase.".into()));
        }
        if let Some(pid) = o.paypal_order_id {
            return Ok(DepositOrderResponse { paypal_order_id: pid, amount_cents: o.amount_cents, currency: o.currency });
        }
    }
    let pp = paypal(st)?;
    let order = sqlx::query_as::<_, OrderRow>(
        "INSERT INTO orders AS o (booking_id, kind, request_id, quantity, amount_cents)
         VALUES ($1, 'ticket', $2, $3, $4)
         ON CONFLICT (request_id) DO UPDATE SET request_id = EXCLUDED.request_id
         RETURNING o.id, o.booking_id, o.request_id, o.paypal_order_id, o.amount_cents, o.currency, o.status",
    )
    .bind(booking_id)
    .bind(idempotency_key)
    .bind(quantity)
    .bind(price * quantity as i64)
    .fetch_one(&st.db)
    .await?;
    let description = format!("{quantity} × ticket — {title}");
    match pp
        .create_order(&order.request_id.to_string(), &order.id.to_string(), &description, order.amount_cents, &order.currency)
        .await
    {
        Ok((paypal_order_id, _)) => {
            sqlx::query("UPDATE orders SET paypal_order_id = $2 WHERE id = $1")
                .bind(order.id)
                .bind(&paypal_order_id)
                .execute(&st.db)
                .await?;
            Ok(DepositOrderResponse { paypal_order_id, amount_cents: order.amount_cents, currency: order.currency })
        }
        Err(e) => {
            tracing::warn!(order = %order.id, error = %e, "PayPal create ticket order failed");
            if !e.is_transient() {
                mark_failed(&st.db, order.id, &e.to_string()).await?;
            }
            Err(AppError::Upstream("PayPal could not create the order. Please try again.".into()))
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CaptureOutcome {
    /// "pending_webhook" (normal), "captured" (already confirmed), "failed"
    pub status: &'static str,
    pub message: Option<String>,
    pub booking_id: Uuid,
}

/// Called after the buyer approved in the PayPal window. Asks PayPal to capture; the booking is
/// confirmed later by the verified webhook (or reconciliation), never by this call.
pub async fn capture(st: &AppState, session: Uuid, paypal_order_id: &str) -> AppResult<CaptureOutcome> {
    let pp = paypal(st)?;
    let order = sqlx::query_as::<_, OrderRow>(&format!(
        "SELECT {ORDER_COLS} FROM orders o JOIN bookings b ON b.id = o.booking_id
          WHERE o.paypal_order_id = $1 AND b.demo_session_id = $2"
    ))
    .bind(paypal_order_id)
    .bind(session)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;

    match order.status.as_str() {
        "captured" => return Ok(CaptureOutcome { status: "captured", message: None, booking_id: order.booking_id }),
        "created" | "approved" => {}
        other => return Err(AppError::Conflict(format!("This PayPal order is {other}."))),
    }
    sqlx::query(
        "UPDATE orders SET status = 'approved', capture_requested_at = COALESCE(capture_requested_at, now())
          WHERE id = $1 AND status IN ('created','approved')",
    )
    .bind(order.id)
    .execute(&st.db)
    .await?;

    match pp.capture_order(paypal_order_id, &format!("capture-{}", order.request_id)).await {
        Ok(info) => {
            // Payer details are metadata for the dashboard; they do not confirm the payment.
            sqlx::query(
                "UPDATE orders SET payer_email = COALESCE($2, payer_email), payer_country = COALESCE($3, payer_country),
                        paypal_capture_id = COALESCE(paypal_capture_id, $4)
                  WHERE id = $1",
            )
            .bind(order.id)
            .bind(&info.payer_email)
            .bind(&info.payer_country)
            .bind(&info.capture_id)
            .execute(&st.db)
            .await?;
            match info.capture_status.as_deref() {
                Some("DECLINED") | Some("FAILED") => {
                    mark_failed(&st.db, order.id, "PayPal declined the capture").await?;
                    Ok(CaptureOutcome {
                        status: "failed",
                        message: Some("PayPal declined the payment. Please try another payment method.".into()),
                        booking_id: order.booking_id,
                    })
                }
                _ => Ok(CaptureOutcome { status: "pending_webhook", message: None, booking_id: order.booking_id }),
            }
        }
        Err(e) if e.has_issue("ORDER_ALREADY_CAPTURED") => {
            Ok(CaptureOutcome { status: "pending_webhook", message: None, booking_id: order.booking_id })
        }
        Err(e) if e.has_issue("INSTRUMENT_DECLINED") => Ok(CaptureOutcome {
            status: "failed",
            message: Some("The card or account was declined. Choose another way to pay.".into()),
            booking_id: order.booking_id,
        }),
        Err(e) => {
            tracing::warn!(order = %order.id, error = %e, "PayPal capture failed");
            sqlx::query("UPDATE orders SET last_error = $2 WHERE id = $1")
                .bind(order.id)
                .bind(e.to_string())
                .execute(&st.db)
                .await?;
            Err(AppError::Upstream("PayPal could not capture the payment. Please try again.".into()))
        }
    }
}

async fn mark_failed(db: &PgPool, order_id: Uuid, reason: &str) -> AppResult<()> {
    sqlx::query("UPDATE orders SET status = 'failed', last_error = $2 WHERE id = $1 AND status <> 'captured'")
        .bind(order_id)
        .bind(reason)
        .execute(db)
        .await?;
    Ok(())
}

/// Finds our order from a capture: by `custom_id` (our order id) or by the PayPal order id.
pub async fn find_order(db: &PgPool, custom_id: Option<&str>, paypal_order_id: Option<&str>) -> AppResult<Option<(Uuid, Uuid)>> {
    if let Some(id) = custom_id.and_then(|c| Uuid::parse_str(c).ok()) {
        if let Some(row) = sqlx::query_as::<_, (Uuid, Uuid)>("SELECT id, booking_id FROM orders WHERE id = $1")
            .bind(id)
            .fetch_optional(db)
            .await?
        {
            return Ok(Some(row));
        }
    }
    if let Some(pid) = paypal_order_id {
        return Ok(sqlx::query_as::<_, (Uuid, Uuid)>("SELECT id, booking_id FROM orders WHERE paypal_order_id = $1")
            .bind(pid)
            .fetch_optional(db)
            .await?);
    }
    Ok(None)
}

/// Records a confirmed capture exactly once: order -> captured, balanced ledger entry,
/// deposit -> booking paid. Returns false if it was already recorded.
pub async fn apply_capture(
    db: &PgPool,
    order_id: Uuid,
    capture_id: &str,
    gross_cents: i64,
    fee_cents: i64,
    confirmed_by: &'static str,
) -> AppResult<bool> {
    let mut tx = db.begin().await?;
    let row: Option<(Uuid, String, i64)> = sqlx::query_as(
        "UPDATE orders SET status = 'captured', paypal_capture_id = $2, paypal_fee_cents = $3,
                captured_at = now(), confirmed_by = $4
          WHERE id = $1 AND status <> 'captured' AND status <> 'refunded'
          RETURNING booking_id, kind, amount_cents",
    )
    .bind(order_id)
    .bind(capture_id)
    .bind(fee_cents)
    .bind(confirmed_by)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((booking_id, kind, expected)) = row else {
        return Ok(false);
    };
    if gross_cents != expected {
        tracing::warn!(order = %order_id, gross_cents, expected, "captured amount differs from the order amount");
        sqlx::query("UPDATE orders SET last_error = $2 WHERE id = $1")
            .bind(order_id)
            .bind(format!("captured {gross_cents} but expected {expected}"))
            .execute(&mut *tx)
            .await?;
    }
    ledger::post(
        &mut tx,
        booking_id,
        Some(order_id),
        None,
        &[
            Entry::new("payer", "payment", -gross_cents, format!("PayPal {kind} capture {capture_id}")),
            Entry::new("paypal_fees", "paypal_fee", fee_cents, "PayPal fee"),
            Entry::new("platform_cash", "payment", gross_cents - fee_cents, format!("{kind} net of PayPal fee")),
        ],
    )
    .await?;
    if kind == "deposit" {
        let updated = sqlx::query(
            "UPDATE bookings SET status = 'paid', paid_at = now(), updated_at = now()
              WHERE id = $1 AND status = 'approved'",
        )
        .bind(booking_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            tracing::warn!(booking = %booking_id, "deposit captured for a booking that is no longer approved — needs a refund");
            sqlx::query("UPDATE orders SET last_error = 'captured after the booking left the approved state; refund needed' WHERE id = $1")
                .bind(order_id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(true)
}

/// Records a refund (from a webhook): money goes back to the payer, out of the platform's cash.
pub async fn apply_refund(db: &PgPool, capture_id: &str, refund_cents: i64, refund_id: &str) -> AppResult<Option<Uuid>> {
    let mut tx = db.begin().await?;
    let row: Option<(Uuid, Uuid, i64)> = sqlx::query_as(
        "SELECT id, booking_id, amount_cents FROM orders WHERE paypal_capture_id = $1 FOR UPDATE",
    )
    .bind(capture_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((order_id, booking_id, amount)) = row else { return Ok(None) };
    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ledger_entries WHERE order_id = $1 AND kind = 'refund' AND memo = $2)",
    )
    .bind(order_id)
    .bind(format!("refund {refund_id}"))
    .fetch_one(&mut *tx)
    .await?;
    if !already {
        ledger::post(
            &mut tx,
            booking_id,
            Some(order_id),
            None,
            &[
                Entry::new("platform_cash", "refund", -refund_cents, format!("refund {refund_id}")),
                Entry::new("payer", "refund", refund_cents, format!("refund {refund_id}")),
            ],
        )
        .await?;
        if refund_cents >= amount {
            sqlx::query("UPDATE orders SET status = 'refunded' WHERE id = $1").bind(order_id).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(Some(booking_id))
}

#[derive(Debug, FromRow)]
struct StaleOrder {
    id: Uuid,
    request_id: Uuid,
    paypal_order_id: String,
    status: String,
    created_at: DateTime<Utc>,
    capture_requested_at: Option<DateTime<Utc>>,
}

/// Reconciliation: asks PayPal about orders whose webhook has not arrived.
/// * captured at PayPal -> record it (confirmed_by = reconciliation)
/// * approved by the buyer but never captured (window closed) -> capture server-side
/// * never approved after 30 minutes -> expire
pub async fn reconcile(db: &PgPool, pp: &PayPal) -> AppResult<usize> {
    let wait_secs: i64 = if pp.webhook_configured() { 90 } else { 15 };
    let rows = sqlx::query_as::<_, StaleOrder>(
        "SELECT o.id, o.request_id, o.paypal_order_id, o.status, o.created_at, o.capture_requested_at
           FROM orders o JOIN bookings b ON b.id = o.booking_id
          WHERE o.source = 'paypal_sandbox' AND o.paypal_order_id IS NOT NULL
            AND o.status IN ('created','approved') AND b.demo_session_id IS NOT NULL
            AND COALESCE(o.capture_requested_at, o.created_at) < now() - make_interval(secs => $1)
          ORDER BY o.created_at LIMIT 20",
    )
    .bind(wait_secs as f64)
    .fetch_all(db)
    .await?;
    let mut changed = 0;
    for o in rows {
        let info = match pp.get_order(&o.paypal_order_id).await {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(order = %o.id, error = %e, "reconcile: get order failed");
                continue;
            }
        };
        let capture_done = info.capture_status.as_deref() == Some("COMPLETED");
        if capture_done {
            if let (Some(cid), Some(gross)) = (info.capture_id.as_deref(), info.gross_cents) {
                if apply_capture(db, o.id, cid, gross, info.fee_cents.unwrap_or(0), "reconciliation").await? {
                    tracing::info!(order = %o.id, "reconciled a capture without webhook");
                    changed += 1;
                }
            }
        } else if info.order_status == "APPROVED" {
            // Buyer approved but the browser never asked us to capture (closed the window).
            if let Err(e) = pp.capture_order(&o.paypal_order_id, &format!("capture-{}", o.request_id)).await {
                tracing::warn!(order = %o.id, error = %e, "reconcile: server-side capture failed");
            } else {
                sqlx::query("UPDATE orders SET status = 'approved', capture_requested_at = now() WHERE id = $1")
                    .bind(o.id)
                    .execute(db)
                    .await?;
                changed += 1;
            }
        } else if o.status == "created"
            && o.capture_requested_at.is_none()
            && o.created_at < Utc::now() - chrono::Duration::minutes(30)
        {
            sqlx::query("UPDATE orders SET status = 'expired' WHERE id = $1 AND status = 'created'")
                .bind(o.id)
                .execute(db)
                .await?;
            changed += 1;
        }
    }
    Ok(changed)
}
