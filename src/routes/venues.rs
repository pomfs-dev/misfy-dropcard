use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::clock;
use crate::error::{AppError, AppResult};
use crate::session::DemoSession;
use crate::AppState;

#[derive(Debug, Serialize, FromRow)]
pub struct Venue {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub city: String,
    pub country: String,
    pub tagline: String,
    pub description: String,
    pub tags: Vec<String>,
    pub capacity_band: String,
    pub tz_offset_minutes: i32,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Space {
    pub id: Uuid,
    pub name: String,
    pub capacity_min: i32,
    pub capacity_max: i32,
    pub day_rate_cents: i64,
    pub sort_order: i32,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UpcomingShow {
    pub id: Uuid,
    pub title: String,
    pub space: String,
    pub starts_at: DateTime<Utc>,
    pub ticket_price_cents: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct Card {
    pub venue: Venue,
    pub spaces: Vec<Space>,
    pub upcoming: Vec<UpcomingShow>,
}

const VENUE_COLUMNS: &str =
    "id, slug, name, city, country, tagline, description, tags, capacity_band, tz_offset_minutes";

pub async fn venue_by_slug(db: &sqlx::PgPool, slug: &str) -> AppResult<Venue> {
    sqlx::query_as::<_, Venue>(&format!("SELECT {VENUE_COLUMNS} FROM venues WHERE slug = $1"))
        .bind(slug)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn spaces_of(db: &sqlx::PgPool, venue_id: Uuid) -> AppResult<Vec<Space>> {
    Ok(sqlx::query_as::<_, Space>(
        "SELECT id, name, capacity_min, capacity_max, day_rate_cents, sort_order
           FROM spaces WHERE venue_id = $1 ORDER BY sort_order, name",
    )
    .bind(venue_id)
    .fetch_all(db)
    .await?)
}

/// GET /api/venues
pub async fn list(State(st): State<AppState>) -> AppResult<Json<Vec<Venue>>> {
    let rows = sqlx::query_as::<_, Venue>(&format!("SELECT {VENUE_COLUMNS} FROM venues ORDER BY name"))
        .fetch_all(&st.db)
        .await?;
    Ok(Json(rows))
}

/// GET /api/venues/{slug} — the public venue card.
pub async fn card(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(slug): Path<String>,
) -> AppResult<Json<Card>> {
    let venue = venue_by_slug(&st.db, &slug).await?;
    let spaces = spaces_of(&st.db, venue.id).await?;
    let now = clock::session_now(&st.db, sid).await?;
    let upcoming = sqlx::query_as::<_, UpcomingShow>(
        "SELECT b.id, b.title, s.name AS space, b.starts_at, b.ticket_price_cents
           FROM bookings b JOIN spaces s ON s.id = b.space_id
          WHERE b.venue_id = $1 AND b.status = 'paid' AND b.starts_at > $2
            AND (b.demo_session_id IS NULL OR b.demo_session_id = $3)
            AND b.ticket_price_cents IS NOT NULL
          ORDER BY b.starts_at LIMIT 5",
    )
    .bind(venue.id)
    .bind(now)
    .bind(sid)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(Card { venue, spaces, upcoming }))
}

#[derive(Debug, Deserialize)]
pub struct WindowQuery {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct ScheduleEvent {
    pub id: Uuid,
    pub space_id: Uuid,
    pub title: String,
    pub status: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub mine: bool,
    pub sample: bool,
}

#[derive(Debug, Serialize)]
pub struct Schedule {
    pub now: DateTime<Utc>,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub tz_offset_minutes: i32,
    pub spaces: Vec<Space>,
    pub events: Vec<ScheduleEvent>,
}

/// GET /api/venues/{slug}/schedule?from=&to= — spaces (rows) and bookings (bars) for the scheduler.
/// A visitor sees the shared sample bookings plus their own; other people's holds show no title.
pub async fn schedule(
    State(st): State<AppState>,
    Extension(DemoSession(sid)): Extension<DemoSession>,
    Path(slug): Path<String>,
    Query(q): Query<WindowQuery>,
) -> AppResult<Json<Schedule>> {
    let venue = venue_by_slug(&st.db, &slug).await?;
    let now = clock::session_now(&st.db, sid).await?;
    let from = q.from.unwrap_or(now - Duration::days(14));
    let to = q.to.unwrap_or(now + Duration::days(75));
    if to <= from || to - from > Duration::days(190) {
        return Err(AppError::BadRequest("window must be positive and at most 190 days".into()));
    }
    let spaces = spaces_of(&st.db, venue.id).await?;
    let events = sqlx::query_as::<_, ScheduleEvent>(
        "SELECT b.id, b.space_id,
                CASE WHEN b.demo_session_id = $2 OR b.status IN ('paid','completed')
                     THEN b.title ELSE 'On hold' END AS title,
                b.status, b.starts_at, b.ends_at,
                COALESCE(b.demo_session_id = $2, false) AS mine,
                b.demo_session_id IS NULL AS sample
           FROM bookings b
          WHERE b.venue_id = $1
            AND b.status <> 'cancelled'
            AND (b.demo_session_id IS NULL OR b.demo_session_id = $2)
            AND tstzrange(b.starts_at, b.ends_at) && tstzrange($3, $4)
          ORDER BY b.starts_at",
    )
    .bind(venue.id)
    .bind(sid)
    .bind(from)
    .bind(to)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(Schedule { now, from, to, tz_offset_minutes: venue.tz_offset_minutes, spaces, events }))
}
