//! Venue owner console. The visitor plays the venue owner for the bookings of their own session;
//! shared sample data is visible read-only.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::bookings::{self, Booking};
use crate::error::AppResult;
use crate::payouts::{self, Settlement};
use crate::routes::bookings::optional_json;
use crate::routes::venues::venue_by_slug;
use crate::session::DemoSession;
use crate::{jobs, AppState};

/// GET /api/console/{slug}/inbox — holds waiting for approval, then approved bookings waiting for the deposit
pub async fn inbox(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(slug): Path<String>,
) -> AppResult<Json<Vec<Booking>>> {
    let venue = venue_by_slug(&st.db, &slug).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM bookings
          WHERE venue_id = $1 AND demo_session_id = $2 AND status IN ('hold','approved')
          ORDER BY (status = 'hold') DESC, COALESCE(requested_at, created_at) DESC",
    )
    .bind(venue.id)
    .bind(sid)
    .fetch_all(&st.db)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(bookings::load(&st.db, sid, id).await?);
    }
    Ok(Json(out))
}

/// POST /api/console/bookings/{id}/approve
pub async fn approve(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Booking>> {
    bookings::approve(&st.db, sid, id).await?;
    Ok(Json(bookings::load(&st.db, sid, id).await?))
}

/// POST /api/console/bookings/{id}/decline
pub async fn decline(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Booking>> {
    bookings::cancel(&st.db, sid, id, &["hold"]).await?;
    Ok(Json(bookings::load(&st.db, sid, id).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct PatchBody {
    pub space_id: Option<Uuid>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
}

/// PATCH /api/console/bookings/{id} — drag/resize on the scheduler
pub async fn patch(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> AppResult<Json<Booking>> {
    let b: PatchBody = optional_json(&body)?;
    bookings::reschedule(&st.db, sid, id, b.space_id, b.starts_at, b.ends_at).await?;
    Ok(Json(bookings::load(&st.db, sid, id).await?))
}

#[derive(Debug, Serialize, FromRow)]
pub struct OrderView {
    pub id: Uuid,
    pub booking_id: Uuid,
    pub booking_title: String,
    pub kind: String,
    pub source: String,
    pub paypal_order_id: Option<String>,
    pub request_id: Uuid,
    pub quantity: i32,
    pub amount_cents: i64,
    pub paypal_fee_cents: Option<i64>,
    pub status: String,
    pub payer_country: Option<String>,
    pub confirmed_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub captured_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct WebhookView {
    pub id: i64,
    pub event_type: String,
    pub resource_id: Option<String>,
    pub signature_valid: bool,
    pub received_at: DateTime<Utc>,
    pub outcome: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Payments {
    pub orders: Vec<OrderView>,
    pub webhooks: Vec<WebhookView>,
}

/// GET /api/console/{slug}/payments — the visitor's orders first, then recent sample orders; webhook log
pub async fn payments(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(slug): Path<String>,
) -> AppResult<Json<Payments>> {
    let venue = venue_by_slug(&st.db, &slug).await?;
    let orders = sqlx::query_as::<_, OrderView>(
        "SELECT o.id, o.booking_id, b.title AS booking_title, o.kind, o.source, o.paypal_order_id, o.request_id,
                o.quantity, o.amount_cents, o.paypal_fee_cents, o.status, o.payer_country, o.confirmed_by,
                o.created_at, o.captured_at
           FROM orders o JOIN bookings b ON b.id = o.booking_id
          WHERE b.venue_id = $1 AND (b.demo_session_id = $2 OR b.demo_session_id IS NULL)
          ORDER BY (b.demo_session_id IS NULL), o.created_at DESC
          LIMIT 200",
    )
    .bind(venue.id)
    .bind(sid)
    .fetch_all(&st.db)
    .await?;
    // Events about this visitor's bookings, plus unmatched or rejected events of the last 24 hours.
    let webhooks = sqlx::query_as::<_, WebhookView>(
        "SELECT id, event_type, resource_id, signature_valid, received_at, outcome, error
           FROM webhook_events
          WHERE booking_id IN (SELECT id FROM bookings WHERE demo_session_id = $1)
             OR (booking_id IS NULL AND received_at > now() - interval '24 hours')
          ORDER BY id DESC LIMIT 100",
    )
    .bind(sid)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(Payments { orders, webhooks }))
}

#[derive(Debug, Serialize)]
pub struct Batches {
    pub batches: Vec<Settlement>,
}

/// GET /api/console/{slug}/payouts
pub async fn payouts_list(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(slug): Path<String>,
) -> AppResult<Json<Batches>> {
    let venue = venue_by_slug(&st.db, &slug).await?;
    Ok(Json(Batches { batches: payouts::settlements_for_venue(&st.db, sid, venue.id).await? }))
}

/// POST /api/console/{slug}/settlement/run — run due settlements now (by this session's clock)
pub async fn run_settlement(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(slug): Path<String>,
) -> AppResult<Json<Batches>> {
    let venue = venue_by_slug(&st.db, &slug).await?;
    jobs::run_cycle(&st, Some(sid)).await?;
    Ok(Json(Batches { batches: payouts::settlements_for_venue(&st.db, sid, venue.id).await? }))
}

#[derive(Debug, Serialize, FromRow)]
pub struct AgentRun {
    pub conversation_id: Uuid,
    pub step: i32,
    pub kind: String,
    pub tool_name: Option<String>,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub latency_ms: Option<i32>,
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct AgentRuns {
    pub runs: Vec<AgentRun>,
}

/// GET /api/console/{slug}/agent-runs — what the AI agent did in this visitor's conversations
pub async fn agent_runs(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(_slug): Path<String>,
) -> AppResult<Json<AgentRuns>> {
    let runs = sqlx::query_as::<_, AgentRun>(
        "SELECT conversation_id, step, kind, tool_name, input, output, latency_ms, input_tokens, output_tokens, created_at
           FROM agent_runs WHERE demo_session_id = $1
          ORDER BY created_at DESC, step DESC LIMIT 300",
    )
    .bind(sid)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(AgentRuns { runs }))
}

#[derive(Debug, Serialize, FromRow)]
pub struct ShowRow {
    pub booking_id: Uuid,
    pub date: DateTime<Utc>,
    pub title: String,
    pub space: String,
    pub status: String,
    pub sample: bool,
    pub tickets_sold: i64,
    pub gross_cents: i64,
    pub paypal_fees_cents: i64,
    pub refunds_cents: i64,
    pub platform_fee_cents: i64,
    pub venue_cents: i64,
    pub artist_cents: i64,
    pub promoter_cents: i64,
    pub payout_status: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct CountryRow {
    pub country: String,
    pub tickets: i64,
    pub gross_cents: i64,
}

#[derive(Debug, Serialize)]
pub struct Totals {
    pub gross_cents: i64,
    pub paypal_fees_cents: i64,
    pub platform_fee_cents: i64,
    pub paid_out_cents: i64,
    pub international_share: f64,
}

#[derive(Debug, Serialize)]
pub struct Dashboard {
    pub shows: Vec<ShowRow>,
    pub by_country: Vec<CountryRow>,
    pub totals: Totals,
}

/// GET /api/console/{slug}/dashboard — every number comes from the ledger.
pub async fn dashboard(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(slug): Path<String>,
) -> AppResult<Json<Dashboard>> {
    let venue = venue_by_slug(&st.db, &slug).await?;
    let shows = sqlx::query_as::<_, ShowRow>(
        "WITH shows AS (
            SELECT b.id, b.title, s.name AS space, b.status, b.starts_at, (b.demo_session_id IS NULL) AS sample
              FROM bookings b JOIN spaces s ON s.id = b.space_id
             WHERE b.venue_id = $1 AND (b.demo_session_id IS NULL OR b.demo_session_id = $2)
               AND b.status IN ('paid','completed')
         ), money AS (
            SELECT l.booking_id,
                   (-COALESCE(sum(l.amount_cents) FILTER (WHERE l.account = 'payer' AND l.kind = 'payment'), 0))::bigint AS gross,
                   COALESCE(sum(l.amount_cents) FILTER (WHERE l.account = 'payer' AND l.kind = 'refund'), 0)::bigint AS refunds,
                   COALESCE(sum(l.amount_cents) FILTER (WHERE l.account = 'paypal_fees'), 0)::bigint AS paypal_fees,
                   COALESCE(sum(l.amount_cents) FILTER (WHERE l.account = 'platform_revenue'), 0)::bigint AS platform_fee,
                   COALESCE(sum(l.amount_cents) FILTER (WHERE l.account = 'venue'), 0)::bigint AS venue,
                   COALESCE(sum(l.amount_cents) FILTER (WHERE l.account = 'artist'), 0)::bigint AS artist,
                   COALESCE(sum(l.amount_cents) FILTER (WHERE l.account = 'promoter'), 0)::bigint AS promoter
              FROM ledger_entries l WHERE l.booking_id IN (SELECT id FROM shows) GROUP BY l.booking_id
         ), tix AS (
            SELECT booking_id, sum(quantity)::bigint AS tickets FROM orders
             WHERE kind = 'ticket' AND status = 'captured' AND booking_id IN (SELECT id FROM shows)
             GROUP BY booking_id
         )
         SELECT sh.id AS booking_id, sh.starts_at AS date, sh.title, sh.space, sh.status, sh.sample,
                COALESCE(t.tickets, 0)::bigint AS tickets_sold,
                COALESCE(m.gross, 0)::bigint AS gross_cents, COALESCE(m.paypal_fees, 0)::bigint AS paypal_fees_cents,
                COALESCE(m.refunds, 0)::bigint AS refunds_cents, COALESCE(m.platform_fee, 0)::bigint AS platform_fee_cents,
                COALESCE(m.venue, 0)::bigint AS venue_cents, COALESCE(m.artist, 0)::bigint AS artist_cents,
                COALESCE(m.promoter, 0)::bigint AS promoter_cents, pb.status AS payout_status
           FROM shows sh
           LEFT JOIN money m ON m.booking_id = sh.id
           LEFT JOIN tix t ON t.booking_id = sh.id
           LEFT JOIN payout_batches pb ON pb.booking_id = sh.id
          ORDER BY sh.starts_at",
    )
    .bind(venue.id)
    .bind(sid)
    .fetch_all(&st.db)
    .await?;

    let by_country = sqlx::query_as::<_, CountryRow>(
        "SELECT COALESCE(o.payer_country, '??') AS country, sum(o.quantity)::bigint AS tickets,
                sum(o.amount_cents)::bigint AS gross_cents
           FROM orders o JOIN bookings b ON b.id = o.booking_id
          WHERE b.venue_id = $1 AND (b.demo_session_id IS NULL OR b.demo_session_id = $2)
            AND o.kind = 'ticket' AND o.status = 'captured'
          GROUP BY 1 ORDER BY 2 DESC",
    )
    .bind(venue.id)
    .bind(sid)
    .fetch_all(&st.db)
    .await?;

    let ticket_gross: i64 = by_country.iter().map(|c| c.gross_cents).sum();
    let international: i64 = by_country.iter().filter(|c| c.country != venue.country).map(|c| c.gross_cents).sum();
    let totals = Totals {
        gross_cents: shows.iter().map(|s| s.gross_cents).sum(),
        paypal_fees_cents: shows.iter().map(|s| s.paypal_fees_cents).sum(),
        platform_fee_cents: shows.iter().map(|s| s.platform_fee_cents).sum(),
        paid_out_cents: shows.iter().map(|s| s.venue_cents + s.artist_cents + s.promoter_cents).sum(),
        international_share: if ticket_gross > 0 { international as f64 / ticket_gross as f64 } else { 0.0 },
    };
    Ok(Json(Dashboard { shows, by_country, totals }))
}
