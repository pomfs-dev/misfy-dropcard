//! Settlement and PayPal Payouts.
//!
//! 1. A paid show becomes `completed` once it has ended (session clock).
//! 2. At 09:00 venue time on the day after the show, a settlement is planned from the ledger:
//!    net money per show (deposit / tickets) -> platform fee -> shares (see `settlement.rs`).
//! 3. The batch is sent to PayPal Payouts. `sender_batch_id` is unique per show and is also the
//!    `PayPal-Request-Id`, so the same show can never be paid twice.
//! 4. Item status comes from webhooks and from polling; only a SUCCESS writes the payout to the ledger.
//! 5. Failed items are retried (in a new batch, max 3 attempts). Unclaimed items are left to the receiver.
//!
//! Sample data (no demo session) is never sent to PayPal.

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::error::AppResult;
use crate::ledger::{self, Entry};
use crate::paypal::{PayPal, PayoutItemRequest};
use crate::settlement::{self, Rule};

#[derive(Debug, Serialize, FromRow)]
pub struct SettlementItem {
    pub role: String,
    pub recipient_email: String,
    pub amount_cents: i64,
    pub status: String,
    pub paypal_item_id: Option<String>,
    pub attempts: i32,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Settlement {
    pub id: Uuid,
    pub booking_id: Uuid,
    pub booking_title: String,
    pub sender_batch_id: String,
    pub paypal_batch_id: Option<String>,
    pub status: String,
    pub scheduled_for: DateTime<Utc>,
    pub net_cents: i64,
    pub platform_fee_cents: i64,
    pub sample: bool,
    pub submitted_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    #[sqlx(skip)]
    pub items: Vec<SettlementItem>,
}

const BATCH_SQL: &str = "SELECT pb.id, pb.booking_id, b.title AS booking_title, pb.sender_batch_id, pb.paypal_batch_id,
        pb.status, pb.scheduled_for, pb.net_cents, pb.platform_fee_cents, (b.demo_session_id IS NULL) AS sample,
        pb.submitted_at, pb.completed_at, pb.last_error
   FROM payout_batches pb JOIN bookings b ON b.id = pb.booking_id";

async fn with_items(db: &PgPool, mut batches: Vec<Settlement>) -> AppResult<Vec<Settlement>> {
    for b in &mut batches {
        b.items = sqlx::query_as::<_, SettlementItem>(
            "SELECT role, recipient_email, amount_cents, status, paypal_item_id, attempts, error
               FROM payout_items WHERE batch_id = $1
              ORDER BY CASE role WHEN 'venue' THEN 1 WHEN 'artist' THEN 2 ELSE 3 END",
        )
        .bind(b.id)
        .fetch_all(db)
        .await?;
    }
    Ok(batches)
}

pub async fn settlement_for_booking(db: &PgPool, booking_id: Uuid) -> AppResult<Option<Settlement>> {
    let rows = sqlx::query_as::<_, Settlement>(&format!("{BATCH_SQL} WHERE pb.booking_id = $1"))
        .bind(booking_id)
        .fetch_all(db)
        .await?;
    Ok(with_items(db, rows).await?.into_iter().next())
}

/// The visitor's own settlements first, then the shared sample history.
pub async fn settlements_for_venue(db: &PgPool, session: Uuid, venue_id: Uuid) -> AppResult<Vec<Settlement>> {
    let rows = sqlx::query_as::<_, Settlement>(&format!(
        "{BATCH_SQL} WHERE b.venue_id = $1 AND (b.demo_session_id IS NULL OR b.demo_session_id = $2)
          ORDER BY (b.demo_session_id IS NULL), pb.scheduled_for DESC LIMIT 40"
    ))
    .bind(venue_id)
    .bind(session)
    .fetch_all(db)
    .await?;
    with_items(db, rows).await
}

/// 09:00 venue time on the day after the show ended.
pub fn payout_time(ends_at: DateTime<Utc>, tz_offset_minutes: i32) -> DateTime<Utc> {
    let tz = FixedOffset::east_opt(tz_offset_minutes * 60).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    let next_day = ends_at.with_timezone(&tz).date_naive() + Duration::days(1);
    tz.from_local_datetime(&next_day.and_hms_opt(9, 0, 0).unwrap())
        .single()
        .unwrap()
        .with_timezone(&Utc)
}

fn role_account(role: &str) -> &'static str {
    match role {
        "venue" => "venue",
        "artist" => "artist",
        _ => "promoter",
    }
}

/// Paid shows that have ended (by each session's clock) become `completed`.
pub async fn complete_shows(db: &PgPool, session: Option<Uuid>) -> AppResult<u64> {
    Ok(sqlx::query(
        "UPDATE bookings b SET status = 'completed', completed_at = now(), updated_at = now()
           FROM demo_sessions s
          WHERE b.demo_session_id = s.id AND b.status = 'paid'
            AND b.ends_at <= now() + make_interval(secs => s.clock_offset_seconds)
            AND ($1::uuid IS NULL OR s.id = $1)",
    )
    .bind(session)
    .execute(db)
    .await?
    .rows_affected())
}

#[derive(Debug, FromRow)]
struct DueRow {
    id: Uuid,
    title: String,
    ends_at: DateTime<Utc>,
    tz_offset_minutes: i32,
    clock_offset_seconds: i64,
    has_artist: bool,
    split_venue_bps: i32,
    split_artist_bps: i32,
    split_promoter_bps: i32,
    platform_fee_bps: i32,
    venue_email: String,
    artist_email: Option<String>,
    promoter_email: String,
}

/// Plans settlements that are due. Returns how many were planned.
pub async fn plan_due(db: &PgPool, session: Option<Uuid>) -> AppResult<usize> {
    let rows = sqlx::query_as::<_, DueRow>(
        "SELECT b.id, b.title, b.ends_at, v.tz_offset_minutes, s.clock_offset_seconds,
                (b.artist_id IS NOT NULL) AS has_artist, b.split_venue_bps, b.split_artist_bps,
                b.split_promoter_bps, b.platform_fee_bps, v.payout_email AS venue_email,
                a.payout_email AS artist_email, p.payout_email AS promoter_email
           FROM bookings b
           JOIN demo_sessions s ON s.id = b.demo_session_id
           JOIN venues v ON v.id = b.venue_id
           JOIN promoters p ON p.id = b.promoter_id
           LEFT JOIN artists a ON a.id = b.artist_id
          WHERE b.status = 'completed'
            AND NOT EXISTS (SELECT 1 FROM payout_batches pb WHERE pb.booking_id = b.id)
            AND ($1::uuid IS NULL OR s.id = $1)",
    )
    .bind(session)
    .fetch_all(db)
    .await?;

    let mut planned = 0;
    for r in rows {
        let due = payout_time(r.ends_at, r.tz_offset_minutes);
        let session_now = Utc::now() + Duration::seconds(r.clock_offset_seconds);
        if session_now < due {
            continue;
        }
        let mut tx = db.begin().await?;
        let (deposit_net, tickets_net): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(sum(l.amount_cents) FILTER (WHERE o.kind = 'deposit'), 0)::bigint,
                    COALESCE(sum(l.amount_cents) FILTER (WHERE o.kind = 'ticket'), 0)::bigint
               FROM ledger_entries l JOIN orders o ON o.id = l.order_id
              WHERE l.booking_id = $1 AND l.account = 'platform_cash'",
        )
        .bind(r.id)
        .fetch_one(&mut *tx)
        .await?;
        let rule = Rule {
            platform_fee_bps: r.platform_fee_bps as i64,
            venue_bps: r.split_venue_bps as i64,
            artist_bps: r.split_artist_bps as i64,
            promoter_bps: r.split_promoter_bps as i64,
            has_artist: r.has_artist,
        };
        let split = match settlement::compute(deposit_net, tickets_net, rule) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(booking = %r.id, error = %e, "cannot settle show");
                continue;
            }
        };
        let batch_id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO payout_batches (booking_id, sender_batch_id, status, scheduled_for, net_cents, platform_fee_cents)
             VALUES ($1, $2, 'planned', $3, $4, $5)
             ON CONFLICT (booking_id) DO NOTHING RETURNING id",
        )
        .bind(r.id)
        .bind(format!("DC-{}", r.id.simple()))
        .bind(due)
        .bind(deposit_net + tickets_net)
        .bind(split.platform_fee)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(batch_id) = batch_id else { continue };

        let mut shares = vec![("venue", r.venue_email.clone(), split.venue)];
        if let Some(email) = &r.artist_email {
            shares.push(("artist", email.clone(), split.artist));
        }
        shares.push(("promoter", r.promoter_email.clone(), split.promoter));
        for (role, email, amount) in shares.into_iter().filter(|(_, _, a)| *a > 0) {
            sqlx::query(
                "INSERT INTO payout_items (batch_id, role, recipient_email, amount_cents, sender_item_id)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(batch_id)
            .bind(role)
            .bind(email)
            .bind(amount)
            .bind(format!("{}-{role}", r.id.simple()))
            .execute(&mut *tx)
            .await?;
        }
        if split.platform_fee > 0 {
            ledger::post(
                &mut tx,
                r.id,
                None,
                None,
                &[
                    Entry::new("platform_cash", "platform_fee", -split.platform_fee, "platform fee"),
                    Entry::new("platform_revenue", "platform_fee", split.platform_fee, "platform fee"),
                ],
            )
            .await?;
        }
        tx.commit().await?;
        tracing::info!(booking = %r.id, title = %r.title, net = deposit_net + tickets_net, "settlement planned");
        planned += 1;
    }
    Ok(planned)
}

#[derive(Debug, FromRow)]
struct PlannedBatch {
    id: Uuid,
    sender_batch_id: String,
    currency: String,
    title: String,
}

#[derive(Debug, FromRow)]
struct ItemToSend {
    sender_item_id: String,
    recipient_email: String,
    amount_cents: i64,
    role: String,
}

/// Sends planned batches to PayPal.
pub async fn submit_planned(db: &PgPool, pp: &PayPal, session: Option<Uuid>) -> AppResult<usize> {
    let batches = sqlx::query_as::<_, PlannedBatch>(
        "SELECT pb.id, pb.sender_batch_id, pb.currency, b.title
           FROM payout_batches pb JOIN bookings b ON b.id = pb.booking_id
          WHERE pb.status = 'planned' AND b.demo_session_id IS NOT NULL AND pb.attempts < 5
            AND ($1::uuid IS NULL OR b.demo_session_id = $1)
          ORDER BY pb.scheduled_for LIMIT 10",
    )
    .bind(session)
    .fetch_all(db)
    .await?;
    let mut sent = 0;
    for b in batches {
        let items = sqlx::query_as::<_, ItemToSend>(
            "SELECT sender_item_id, recipient_email, amount_cents, role
               FROM payout_items WHERE batch_id = $1 AND status = 'planned'",
        )
        .bind(b.id)
        .fetch_all(db)
        .await?;
        if items.is_empty() {
            sqlx::query("UPDATE payout_batches SET status = 'succeeded', completed_at = now() WHERE id = $1")
                .bind(b.id)
                .execute(db)
                .await?;
            continue;
        }
        let requests: Vec<PayoutItemRequest> = items
            .iter()
            .map(|i| PayoutItemRequest {
                sender_item_id: i.sender_item_id.clone(),
                receiver: i.recipient_email.clone(),
                amount_cents: i.amount_cents,
                note: format!("{} share — {}", i.role, b.title),
            })
            .collect();
        match pp
            .create_payout(&b.sender_batch_id, &format!("Your share of {}", b.title), &requests, &b.currency)
            .await
        {
            Ok((paypal_batch_id, status)) => {
                let mut tx = db.begin().await?;
                sqlx::query(
                    "UPDATE payout_batches SET status = 'submitted', paypal_batch_id = $2, submitted_at = now(),
                            attempts = attempts + 1, last_error = NULL
                      WHERE id = $1",
                )
                .bind(b.id)
                .bind(&paypal_batch_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE payout_items SET status = 'pending', paypal_batch_id = $2, last_sender_batch_id = $3,
                            attempts = attempts + 1, updated_at = now()
                      WHERE batch_id = $1 AND status = 'planned'",
                )
                .bind(b.id)
                .bind(&paypal_batch_id)
                .bind(&b.sender_batch_id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                tracing::info!(batch = %b.sender_batch_id, %paypal_batch_id, %status, "payout batch submitted");
                sent += 1;
            }
            Err(e) => {
                tracing::warn!(batch = %b.sender_batch_id, error = %e, "payout batch failed to submit");
                sqlx::query(
                    "UPDATE payout_batches SET attempts = attempts + 1, last_error = $2,
                            status = CASE WHEN attempts + 1 >= 5 THEN 'failed' ELSE status END
                      WHERE id = $1",
                )
                .bind(b.id)
                .bind(e.to_string())
                .execute(db)
                .await?;
            }
        }
    }
    Ok(sent)
}

fn map_item_status(paypal_status: &str) -> &'static str {
    match paypal_status {
        "SUCCESS" => "succeeded",
        "UNCLAIMED" => "unclaimed",
        "RETURNED" => "returned",
        "FAILED" | "BLOCKED" | "DENIED" | "REFUNDED" | "REVERSED" | "CANCELED" => "failed",
        _ => "pending", // PENDING, ONHOLD, NEW, ...
    }
}

/// Applies a PayPal item status (from a webhook or from polling). Writes the payout to the ledger
/// exactly once, on the transition to SUCCESS. Returns the booking id if the item is ours.
pub async fn apply_item_status(
    db: &PgPool,
    sender_item_id: &str,
    paypal_item_id: Option<&str>,
    paypal_status: &str,
    error: Option<&str>,
) -> AppResult<Option<Uuid>> {
    let new_status = map_item_status(paypal_status);
    let mut tx = db.begin().await?;
    let row: Option<(Uuid, String, i64, String, Uuid, Uuid)> = sqlx::query_as(
        "SELECT pi.id, pi.status, pi.amount_cents, pi.role, pb.booking_id, pb.id
           FROM payout_items pi JOIN payout_batches pb ON pb.id = pi.batch_id
          WHERE pi.sender_item_id = $1 FOR UPDATE OF pi",
    )
    .bind(sender_item_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((item_id, old_status, amount, role, booking_id, batch_id)) = row else {
        return Ok(None);
    };
    if old_status == "succeeded" {
        return Ok(Some(booking_id));
    }
    sqlx::query(
        "UPDATE payout_items SET status = $2, paypal_item_id = COALESCE($3, paypal_item_id),
                error = $4, updated_at = now()
          WHERE id = $1",
    )
    .bind(item_id)
    .bind(new_status)
    .bind(paypal_item_id)
    .bind(error)
    .execute(&mut *tx)
    .await?;
    if new_status == "succeeded" {
        ledger::post(
            &mut tx,
            booking_id,
            None,
            Some(item_id),
            &[
                Entry::new("platform_cash", "payout", -amount, format!("PayPal payout to {role}")),
                Entry::new(role_account(&role), "payout", amount, format!("PayPal payout to {role}")),
            ],
        )
        .await?;
    }
    tx.commit().await?;
    refresh_batch_status(db, batch_id).await?;
    Ok(Some(booking_id))
}

pub async fn refresh_batch_status(db: &PgPool, batch_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "UPDATE payout_batches pb
            SET status = x.new_status,
                completed_at = CASE WHEN x.new_status = 'succeeded' THEN COALESCE(pb.completed_at, now())
                                    ELSE pb.completed_at END
           FROM (SELECT batch_id,
                        CASE WHEN bool_and(status = 'succeeded') THEN 'succeeded'
                             WHEN bool_or(status IN ('pending','planned')) THEN 'submitted'
                             WHEN bool_or(status IN ('succeeded','unclaimed')) THEN 'partial'
                             ELSE 'failed' END AS new_status
                   FROM payout_items WHERE batch_id = $1 GROUP BY batch_id) x
          WHERE pb.id = x.batch_id AND pb.status <> 'planned'",
    )
    .bind(batch_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Polls PayPal for items still pending (in case a webhook is late or missing).
pub async fn poll_pending(db: &PgPool, pp: &PayPal) -> AppResult<usize> {
    let batch_ids: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT pi.paypal_batch_id
           FROM payout_items pi JOIN payout_batches pb ON pb.id = pi.batch_id JOIN bookings b ON b.id = pb.booking_id
          WHERE pi.status = 'pending' AND pi.paypal_batch_id IS NOT NULL AND b.demo_session_id IS NOT NULL
          LIMIT 20",
    )
    .fetch_all(db)
    .await?;
    let mut updated = 0;
    for id in batch_ids {
        match pp.get_payout(&id).await {
            Ok(status) => {
                for item in status.items {
                    if apply_item_status(
                        db,
                        &item.sender_item_id,
                        item.payout_item_id.as_deref(),
                        &item.transaction_status,
                        item.error.as_deref(),
                    )
                    .await?
                    .is_some()
                    {
                        updated += 1;
                    }
                }
            }
            Err(e) => tracing::warn!(paypal_batch_id = %id, error = %e, "payout poll failed"),
        }
    }
    Ok(updated)
}

#[derive(Debug, FromRow)]
struct FailedItem {
    batch_id: Uuid,
    sender_batch_id: String,
    title: String,
    currency: String,
    item_id: Uuid,
    sender_item_id: String,
    recipient_email: String,
    amount_cents: i64,
    role: String,
    attempts: i32,
}

/// Retries failed items (not unclaimed ones) in a new batch, at most 3 attempts, 10 minutes apart.
pub async fn retry_failed(db: &PgPool, pp: &PayPal) -> AppResult<usize> {
    let items = sqlx::query_as::<_, FailedItem>(
        "SELECT pb.id AS batch_id, pb.sender_batch_id, b.title, pb.currency, pi.id AS item_id, pi.sender_item_id,
                pi.recipient_email, pi.amount_cents, pi.role, pi.attempts
           FROM payout_items pi JOIN payout_batches pb ON pb.id = pi.batch_id JOIN bookings b ON b.id = pb.booking_id
          WHERE pi.status = 'failed' AND pi.attempts < 3 AND pi.updated_at < now() - interval '10 minutes'
            AND b.demo_session_id IS NOT NULL
          ORDER BY pb.id LIMIT 30",
    )
    .fetch_all(db)
    .await?;
    let mut retried = 0;
    let mut by_batch: std::collections::BTreeMap<Uuid, Vec<FailedItem>> = Default::default();
    for i in items {
        by_batch.entry(i.batch_id).or_default().push(i);
    }
    for (batch_id, items) in by_batch {
        let attempt = items.iter().map(|i| i.attempts).max().unwrap_or(1) + 1;
        let retry_batch_id = format!("{}-R{attempt}", items[0].sender_batch_id);
        let requests: Vec<PayoutItemRequest> = items
            .iter()
            .map(|i| PayoutItemRequest {
                sender_item_id: i.sender_item_id.clone(),
                receiver: i.recipient_email.clone(),
                amount_cents: i.amount_cents,
                note: format!("{} share — {} (retry)", i.role, i.title),
            })
            .collect();
        let ids: Vec<Uuid> = items.iter().map(|i| i.item_id).collect();
        match pp
            .create_payout(&retry_batch_id, &format!("Your share of {}", items[0].title), &requests, &items[0].currency)
            .await
        {
            Ok((paypal_batch_id, _)) => {
                sqlx::query(
                    "UPDATE payout_items SET status = 'pending', paypal_batch_id = $2, last_sender_batch_id = $3,
                            attempts = attempts + 1, error = NULL, updated_at = now()
                      WHERE id = ANY($1)",
                )
                .bind(&ids)
                .bind(&paypal_batch_id)
                .bind(&retry_batch_id)
                .execute(db)
                .await?;
                retried += ids.len();
            }
            Err(e) => {
                sqlx::query(
                    "UPDATE payout_items SET attempts = attempts + 1, error = $2, updated_at = now() WHERE id = ANY($1)",
                )
                .bind(&ids)
                .bind(e.to_string())
                .execute(db)
                .await?;
            }
        }
        refresh_batch_status(db, batch_id).await?;
    }
    Ok(retried)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payout_is_next_morning_venue_time() {
        // Show ends 23:00 Seoul (14:00 UTC) on Fri 13 Nov -> 09:00 Seoul Sat 14 Nov = 00:00 UTC.
        let ends: DateTime<Utc> = "2026-11-13T14:00:00Z".parse().unwrap();
        assert_eq!(payout_time(ends, 540), "2026-11-14T00:00:00Z".parse::<DateTime<Utc>>().unwrap());
        // Ends 00:30 Seoul on Sat (after midnight) -> 09:00 Seoul Sun.
        let late: DateTime<Utc> = "2026-11-13T15:30:00Z".parse().unwrap();
        assert_eq!(payout_time(late, 540), "2026-11-15T00:00:00Z".parse::<DateTime<Utc>>().unwrap());
    }

    #[test]
    fn item_status_mapping() {
        assert_eq!(map_item_status("SUCCESS"), "succeeded");
        assert_eq!(map_item_status("UNCLAIMED"), "unclaimed");
        assert_eq!(map_item_status("FAILED"), "failed");
        assert_eq!(map_item_status("ONHOLD"), "pending");
    }
}
