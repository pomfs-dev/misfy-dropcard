//! Demo clock. Settlement runs at 09:00 venue time on the day after a show; nobody can wait for
//! that in a 3-minute video, so each demo session can move its own clock forward. The offset only
//! changes what "now" means for that session's bookings. PayPal calls always happen for real.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn offset_seconds(db: &PgPool, session: Uuid) -> sqlx::Result<i64> {
    let off: Option<i64> =
        sqlx::query_scalar("SELECT clock_offset_seconds FROM demo_sessions WHERE id = $1")
            .bind(session)
            .fetch_optional(db)
            .await?;
    Ok(off.unwrap_or(0))
}

/// "Now" as seen by a demo session.
pub async fn session_now(db: &PgPool, session: Uuid) -> sqlx::Result<DateTime<Utc>> {
    Ok(Utc::now() + Duration::seconds(offset_seconds(db, session).await?))
}
