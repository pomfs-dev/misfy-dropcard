use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::{Extension, Json};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::bookings::{self, Booking};
use crate::error::{AppError, AppResult};
use crate::payments::{self, DepositOrderResponse};
use crate::session::DemoSession;
use crate::AppState;

/// Parses an optional JSON body: empty body = defaults.
pub fn optional_json<T: DeserializeOwned + Default>(body: &Bytes) -> AppResult<T> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))
}

/// GET /api/bookings/{id}
pub async fn get_booking(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Booking>> {
    Ok(Json(bookings::load(&st.db, sid, id).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct RequestBody {
    pub inquiry_text: Option<String>,
}

/// POST /api/bookings/{id}/request — proposal -> hold
pub async fn request(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> AppResult<Json<Booking>> {
    let body: RequestBody = optional_json(&body)?;
    let inquiry = body
        .inquiry_text
        .map(|t| t.trim().chars().take(4000).collect::<String>())
        .filter(|t| !t.is_empty());
    bookings::request(&st.db, sid, id, inquiry.as_deref()).await?;
    Ok(Json(bookings::load(&st.db, sid, id).await?))
}

/// POST /api/bookings/{id}/cancel — the promoter withdraws (before paying)
pub async fn cancel(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Booking>> {
    bookings::cancel(&st.db, sid, id, &["ai_proposed", "hold", "approved"]).await?;
    Ok(Json(bookings::load(&st.db, sid, id).await?))
}

/// POST /api/bookings/{id}/deposit-order (header Idempotency-Key: <uuid>)
pub async fn deposit_order(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> AppResult<Json<DepositOrderResponse>> {
    let key = idempotency_key(&headers)?;
    Ok(Json(payments::create_deposit_order(&st, sid, id, key).await?))
}

fn idempotency_key(headers: &HeaderMap) -> AppResult<Uuid> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v.trim()).ok())
        .ok_or_else(|| AppError::BadRequest("the Idempotency-Key header (a UUID) is required".into()))
}

#[derive(Debug, Default, Deserialize)]
pub struct TicketBody {
    pub quantity: Option<i32>,
}

/// POST /api/shows/{id}/ticket-order {"quantity": 2} (header Idempotency-Key: <uuid>)
pub async fn ticket_order(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<DepositOrderResponse>> {
    let key = idempotency_key(&headers)?;
    let body: TicketBody = optional_json(&body)?;
    Ok(Json(payments::create_ticket_order(&st, sid, id, body.quantity.unwrap_or(1), key).await?))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct OrderStatus {
    pub paypal_order_id: String,
    pub booking_id: Uuid,
    pub kind: String,
    pub status: String,
    pub confirmed_by: Option<String>,
    pub quantity: i32,
    pub amount_cents: i64,
    pub captured_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// GET /api/orders/{paypal_order_id} — one of this visitor's PayPal orders (poll after a ticket capture).
pub async fn order_status(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(paypal_order_id): Path<String>,
) -> AppResult<Json<OrderStatus>> {
    let order = sqlx::query_as::<_, OrderStatus>(
        "SELECT o.paypal_order_id, o.booking_id, o.kind, o.status, o.confirmed_by, o.quantity, o.amount_cents, o.captured_at
           FROM orders o JOIN bookings b ON b.id = o.booking_id
          WHERE o.paypal_order_id = $1 AND b.demo_session_id = $2",
    )
    .bind(&paypal_order_id)
    .bind(sid)
    .fetch_optional(&st.db)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(order))
}

#[derive(Debug, Serialize)]
pub struct CaptureResponse {
    pub status: &'static str,
    pub message: Option<String>,
    pub booking: Booking,
}

/// POST /api/paypal/orders/{paypal_order_id}/capture — after the buyer approved in the PayPal window
pub async fn capture(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(paypal_order_id): Path<String>,
) -> AppResult<Json<CaptureResponse>> {
    let out = payments::capture(&st, sid, &paypal_order_id).await?;
    let booking = bookings::load(&st.db, sid, out.booking_id).await?;
    Ok(Json(CaptureResponse { status: out.status, message: out.message, booking }))
}
