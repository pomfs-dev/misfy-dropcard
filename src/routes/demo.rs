use axum::extract::State;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::clock;
use crate::error::{AppError, AppResult};
use crate::session::DemoSession;
use crate::AppState;

const MAX_OFFSET_SECONDS: i64 = 90 * 24 * 3600;

#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub session_id: Uuid,
    pub now: DateTime<Utc>,
    pub clock_offset_seconds: i64,
}

async fn info(st: &AppState, sid: Uuid) -> AppResult<SessionInfo> {
    let off = clock::offset_seconds(&st.db, sid).await?;
    Ok(SessionInfo {
        session_id: sid,
        now: Utc::now() + chrono::Duration::seconds(off),
        clock_offset_seconds: off,
    })
}

/// GET /api/session
pub async fn session(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
) -> AppResult<Json<SessionInfo>> {
    Ok(Json(info(&st, sid).await?))
}

#[derive(Debug, Deserialize)]
pub struct Advance {
    pub hours: i64,
}

/// POST /api/demo/clock/advance {"hours": 24} — move this session's demo clock forward.
pub async fn advance_clock(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Json(body): Json<Advance>,
) -> AppResult<Json<SessionInfo>> {
    if !(1..=24 * 30).contains(&body.hours) {
        return Err(AppError::BadRequest("hours must be between 1 and 720".into()));
    }
    let updated = sqlx::query(
        "UPDATE demo_sessions SET clock_offset_seconds = clock_offset_seconds + $2
          WHERE id = $1 AND clock_offset_seconds + $2 <= $3",
    )
    .bind(sid)
    .bind(body.hours * 3600)
    .bind(MAX_OFFSET_SECONDS)
    .execute(&st.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Conflict("the demo clock can move at most 90 days ahead".into()));
    }
    Ok(Json(info(&st, sid).await?))
}

/// POST /api/demo/clock/reset — back to real time.
pub async fn reset_clock(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
) -> AppResult<Json<SessionInfo>> {
    sqlx::query("UPDATE demo_sessions SET clock_offset_seconds = 0 WHERE id = $1")
        .bind(sid)
        .execute(&st.db)
        .await?;
    Ok(Json(info(&st, sid).await?))
}

/// POST /api/demo/reset — forget this visitor's bookings and conversations and reset the clock.
/// Shared sample data is not touched. Money already moved in the PayPal sandbox stays there.
pub async fn reset_session(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
) -> AppResult<Json<SessionInfo>> {
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM agent_runs WHERE demo_session_id = $1").bind(sid).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM bookings WHERE demo_session_id = $1").bind(sid).execute(&mut *tx).await?;
    sqlx::query("UPDATE demo_sessions SET clock_offset_seconds = 0 WHERE id = $1")
        .bind(sid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(info(&st, sid).await?))
}
