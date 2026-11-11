//! Bookings: availability, AI proposals, requests (holds), approval, cancellation, rescheduling.
//!
//! Overlap rules: within one demo session the database's exclusion constraint forbids two live
//! bookings of the same space at the same time. A session's booking must also not overlap the
//! shared sample bookings; that check runs here under a per-space advisory lock, so two requests
//! for the same slot are serialised and only one can win.

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Utc, Weekday};
use serde::Serialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::clock;
use crate::error::{AppError, AppResult};
use crate::payouts::{self, Settlement};
use crate::routes::venues::{Space, Venue};

/// Deposit = 30% of the quote.
pub const DEPOSIT_BPS: i64 = 3_000;
/// Default ticket price for shows booked through the agent (the promoter can say otherwise).
pub const DEFAULT_TICKET_CENTS: i64 = 3_000;
const LIVE: &str = "('hold','approved','paid','completed')";

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct BookingSummary {
    pub id: Uuid,
    pub title: String,
    pub status: String,
    pub space_id: Uuid,
    pub space_name: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub quote_cents: i64,
    pub deposit_cents: i64,
    pub headcount: Option<i32>,
    pub genre: Option<String>,
    pub created_by: String,
    pub venue_slug: String,
    pub venue_name: String,
    /// The venue's UTC offset in minutes (times are shown in venue time).
    pub venue_tz: i32,
}

pub const SUMMARY_SQL: &str = "SELECT b.id, b.title, b.status, b.space_id, s.name AS space_name, b.starts_at, b.ends_at,
        b.quote_cents, b.deposit_cents, b.headcount, b.genre, b.created_by,
        v.slug AS venue_slug, v.name AS venue_name, v.tz_offset_minutes AS venue_tz
   FROM bookings b JOIN spaces s ON s.id = b.space_id JOIN venues v ON v.id = b.venue_id";

#[derive(Debug, Serialize)]
pub struct ArtistRef {
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct DepositOrder {
    pub paypal_order_id: Option<String>,
    pub status: String,
    pub amount_cents: i64,
    pub captured_at: Option<DateTime<Utc>>,
    pub confirmed_by: Option<String>,
    #[serde(skip)]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct TimelineEntry {
    pub at: DateTime<Utc>,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct Booking {
    #[serde(flatten)]
    pub summary: BookingSummary,
    pub artist: Option<ArtistRef>,
    pub ticket_price_cents: Option<i64>,
    /// Tickets with a confirmed PayPal capture, and their gross.
    pub tickets_sold: i64,
    pub ticket_sales_cents: i64,
    pub inquiry_text: Option<String>,
    pub approved_at: Option<DateTime<Utc>>,
    pub conversation_id: Option<Uuid>,
    pub deposit_order: Option<DepositOrder>,
    pub settlement: Option<Settlement>,
    pub timeline: Vec<TimelineEntry>,
}

#[derive(Debug, FromRow)]
struct DetailRow {
    artist_slug: Option<String>,
    artist_name: Option<String>,
    ticket_price_cents: Option<i64>,
    inquiry_text: Option<String>,
    approved_at: Option<DateTime<Utc>>,
    conversation_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    requested_at: Option<DateTime<Utc>>,
    paid_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    cancelled_at: Option<DateTime<Utc>>,
}

/// Maps constraint violations (overlap, duplicate) to 409 instead of 500.
pub fn conflict_on_constraint(e: sqlx::Error, message: &str) -> AppError {
    if let sqlx::Error::Database(db) = &e {
        if matches!(db.code().as_deref(), Some("23P01") | Some("23505")) {
            return AppError::Conflict(message.to_string());
        }
    }
    AppError::Db(e)
}

pub async fn summary(db: &PgPool, id: Uuid) -> AppResult<BookingSummary> {
    sqlx::query_as::<_, BookingSummary>(&format!("{SUMMARY_SQL} WHERE b.id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)
}

/// A booking as the visitor may see it: their own, or shared sample data.
pub async fn load(db: &PgPool, session: Uuid, id: Uuid) -> AppResult<Booking> {
    let summary = sqlx::query_as::<_, BookingSummary>(&format!(
        "{SUMMARY_SQL} WHERE b.id = $1 AND (b.demo_session_id IS NULL OR b.demo_session_id = $2)"
    ))
    .bind(id)
    .bind(session)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;

    let d = sqlx::query_as::<_, DetailRow>(
        "SELECT a.slug AS artist_slug, a.name AS artist_name, b.ticket_price_cents,
                b.inquiry_text, b.approved_at, b.conversation_id, b.created_at, b.requested_at, b.paid_at,
                b.completed_at, b.cancelled_at
           FROM bookings b LEFT JOIN artists a ON a.id = b.artist_id
          WHERE b.id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await?;

    let deposit_order = sqlx::query_as::<_, DepositOrder>(
        "SELECT paypal_order_id, status, amount_cents, captured_at, confirmed_by, created_at
           FROM orders WHERE booking_id = $1 AND kind = 'deposit'
          ORDER BY (status = 'captured') DESC, created_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;

    let settlement = payouts::settlement_for_booking(db, id).await?;
    let (tickets_sold, ticket_sales_cents): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(sum(quantity), 0)::bigint, COALESCE(sum(amount_cents), 0)::bigint
           FROM orders WHERE booking_id = $1 AND kind = 'ticket' AND status = 'captured'",
    )
    .bind(id)
    .fetch_one(db)
    .await?;

    let mut timeline = vec![TimelineEntry {
        at: d.created_at,
        label: match summary.created_by.as_str() {
            "agent" => "AI proposed this slot".into(),
            "promoter" => "Slot proposed".into(),
            _ => "Booking created".into(),
        },
    }];
    if let Some(at) = d.requested_at {
        let label = if summary.created_by == "agent" {
            "Proposal sent to the venue (written by the AI)"
        } else {
            "Requested by the promoter (inquiry sent)"
        };
        timeline.push(TimelineEntry { at, label: label.into() });
    }
    if let Some(at) = d.approved_at {
        timeline.push(TimelineEntry { at, label: "Approved by the venue".into() });
    }
    if let Some(o) = &deposit_order {
        timeline.push(TimelineEntry { at: o.created_at, label: "PayPal order created for the deposit".into() });
        if let Some(at) = o.captured_at {
            let how = match o.confirmed_by.as_deref() {
                Some("reconciliation") => "confirmed by reconciliation with the PayPal API",
                _ => "confirmed by a verified PayPal webhook",
            };
            timeline.push(TimelineEntry { at, label: format!("Deposit paid — {how}") });
        }
    } else if let Some(at) = d.paid_at {
        timeline.push(TimelineEntry { at, label: "Deposit paid".into() });
    }
    if d.completed_at.is_some() || summary.status == "completed" {
        timeline.push(TimelineEntry { at: summary.ends_at, label: "Show ended".into() });
    }
    if let Some(s) = &settlement {
        timeline.push(TimelineEntry { at: s.scheduled_for, label: "Settlement due (09:00 venue time, day after the show)".into() });
        if let Some(at) = s.submitted_at {
            timeline.push(TimelineEntry { at, label: "Payout batch sent to PayPal".into() });
        }
        if let Some(at) = s.completed_at {
            timeline.push(TimelineEntry { at, label: "Paid out to venue, artist and promoter".into() });
        }
    }
    if let Some(at) = d.cancelled_at {
        timeline.push(TimelineEntry { at, label: "Cancelled".into() });
    }
    // Entries are already in lifecycle order. Do not sort by time: with a moved demo clock, show and
    // settlement times are in the session's future while PayPal events carry real timestamps.

    Ok(Booking {
        summary,
        artist: match (d.artist_slug, d.artist_name) {
            (Some(slug), Some(name)) => Some(ArtistRef { slug, name }),
            _ => None,
        },
        ticket_price_cents: d.ticket_price_cents,
        tickets_sold,
        ticket_sales_cents,
        inquiry_text: d.inquiry_text,
        approved_at: d.approved_at,
        conversation_id: d.conversation_id,
        deposit_order,
        settlement,
        timeline,
    })
}

// ------------------------------------------------------------------ availability

pub fn venue_tz(venue: &Venue) -> FixedOffset {
    tz_from_minutes(venue.tz_offset_minutes)
}

pub fn tz_from_minutes(minutes: i32) -> FixedOffset {
    FixedOffset::east_opt(minutes * 60).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap())
}

pub fn local_label(at: DateTime<Utc>, tz: FixedOffset) -> String {
    at.with_timezone(&tz).format("%a %-d %b %Y, %H:%M").to_string()
}

#[derive(Debug, Clone)]
pub struct SlotQuery {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub headcount: Option<i32>,
    pub weekdays: Vec<Weekday>,
    pub start_hour: u32,
    pub hours: u32,
    pub space_name: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Slot {
    pub space_id: Uuid,
    pub space_name: String,
    pub capacity: String,
    pub date: NaiveDate,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub local_time: String,
    pub quote_cents: i64,
    pub deposit_cents: i64,
}

pub fn deposit_for(quote_cents: i64) -> i64 {
    quote_cents * DEPOSIT_BPS / 10_000
}

/// Free slots, one per date (the best-fitting free space), at most 62 days ahead.
pub async fn free_slots(
    db: &PgPool,
    session: Uuid,
    venue: &Venue,
    spaces: &[Space],
    q: &SlotQuery,
) -> AppResult<Vec<Slot>> {
    let tz = venue_tz(venue);
    let now = clock::session_now(db, session).await?;
    let today = now.with_timezone(&tz).date_naive();
    let from = q.from.max(today);
    let to = q.to.min(from + Duration::days(62));
    if to < from {
        return Ok(vec![]);
    }
    let hours = q.hours.clamp(1, 12);
    let start_hour = q.start_hour.min(23);

    let window_start = tz
        .from_local_datetime(&from.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .with_timezone(&Utc);
    let window_end = tz
        .from_local_datetime(&(to + Duration::days(2)).and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let busy: Vec<(Uuid, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(&format!(
        "SELECT space_id, starts_at, ends_at FROM bookings
          WHERE venue_id = $1 AND status IN {LIVE}
            AND (demo_session_id IS NULL OR demo_session_id = $2)
            AND tstzrange(starts_at, ends_at) && tstzrange($3, $4)"
    ))
    .bind(venue.id)
    .bind(session)
    .bind(window_start)
    .bind(window_end)
    .fetch_all(db)
    .await?;

    // Best fit first: spaces whose range contains the headcount, smallest first; then bigger rooms.
    let mut candidates: Vec<&Space> = spaces
        .iter()
        .filter(|s| q.headcount.map_or(true, |h| s.capacity_max >= h))
        .filter(|s| {
            q.space_name
                .as_ref()
                .map_or(true, |n| s.name.to_lowercase().contains(&n.to_lowercase()))
        })
        .collect();
    candidates.sort_by_key(|s| {
        let fits = q.headcount.map_or(true, |h| s.capacity_min <= h);
        (!fits, s.capacity_max)
    });

    let mut out = Vec::new();
    let mut date = from;
    while date <= to && out.len() < q.limit {
        if q.weekdays.is_empty() || q.weekdays.contains(&date.weekday()) {
            let starts = tz
                .from_local_datetime(&date.and_hms_opt(start_hour, 0, 0).unwrap())
                .single()
                .unwrap()
                .with_timezone(&Utc);
            let ends = starts + Duration::hours(hours as i64);
            if starts > now {
                if let Some(space) = candidates.iter().find(|s| {
                    !busy.iter().any(|(sid, bs, be)| *sid == s.id && *bs < ends && starts < *be)
                }) {
                    out.push(Slot {
                        space_id: space.id,
                        space_name: space.name.clone(),
                        capacity: format!("{}–{}", space.capacity_min, space.capacity_max),
                        date,
                        starts_at: starts,
                        ends_at: ends,
                        local_time: local_label(starts, tz),
                        quote_cents: space.day_rate_cents,
                        deposit_cents: deposit_for(space.day_rate_cents),
                    });
                }
            }
        }
        date += Duration::days(1);
    }
    Ok(out)
}

// ------------------------------------------------------------------ state changes

async fn lock_space(tx: &mut Transaction<'_, Postgres>, space_id: Uuid) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 7))")
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn slot_taken(
    tx: &mut Transaction<'_, Postgres>,
    session: Uuid,
    space_id: Uuid,
    starts: DateTime<Utc>,
    ends: DateTime<Utc>,
    exclude: Option<Uuid>,
) -> AppResult<bool> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM bookings
                         WHERE space_id = $1 AND status IN {LIVE}
                           AND (demo_session_id IS NULL OR demo_session_id = $2)
                           AND ($5::uuid IS NULL OR id <> $5)
                           AND tstzrange(starts_at, ends_at) && tstzrange($3, $4))"
    ))
    .bind(space_id)
    .bind(session)
    .bind(starts)
    .bind(ends)
    .bind(exclude)
    .fetch_one(&mut **tx)
    .await?)
}

#[derive(Debug, FromRow)]
struct OwnRow {
    status: String,
    space_id: Uuid,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    venue_id: Uuid,
    conversation_id: Option<Uuid>,
    headcount: Option<i32>,
}

fn too_small(space_name: &str, capacity_max: i32, headcount: Option<i32>) -> Option<AppError> {
    match headcount {
        Some(h) if h > capacity_max => Some(AppError::Conflict(format!(
            "{space_name} holds at most {capacity_max} guests; this booking is for {h}"
        ))),
        _ => None,
    }
}

async fn lock_own(tx: &mut Transaction<'_, Postgres>, session: Uuid, id: Uuid) -> AppResult<OwnRow> {
    sqlx::query_as::<_, OwnRow>(
        "SELECT status, space_id, starts_at, ends_at, venue_id, conversation_id, headcount
           FROM bookings WHERE id = $1 AND demo_session_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(session)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(AppError::NotFound)
}

pub struct NewProposal<'a> {
    /// Groups one round of proposals (an agent conversation, or one search of the booking form).
    pub conversation_id: Uuid,
    /// "promoter" for the booking form, "agent" for the optional AI concierge.
    pub created_by: &'static str,
    pub venue: &'a Venue,
    pub space: &'a Space,
    pub starts_at: DateTime<Utc>,
    pub hours: u32,
    pub title: String,
    pub headcount: Option<i32>,
    pub genre: Option<String>,
    pub artist_id: Option<Uuid>,
    pub ticket_price_cents: Option<i64>,
}

/// A proposed slot (dashed bar). It does not reserve the slot, but we only propose free slots.
pub async fn propose(db: &PgPool, session: Uuid, p: NewProposal<'_>) -> AppResult<BookingSummary> {
    let now = clock::session_now(db, session).await?;
    if p.starts_at <= now {
        return Err(AppError::BadRequest("that time is in the past".into()));
    }
    if let Some(e) = too_small(&p.space.name, p.space.capacity_max, p.headcount) {
        return Err(e);
    }
    let ends_at = p.starts_at + Duration::hours(p.hours.clamp(1, 12) as i64);
    let mut tx = db.begin().await?;
    lock_space(&mut tx, p.space.id).await?;
    if slot_taken(&mut tx, session, p.space.id, p.starts_at, ends_at, None).await? {
        return Err(AppError::Conflict(format!("{} is already booked at that time", p.space.name)));
    }
    let promoter: Uuid =
        sqlx::query_scalar("SELECT id FROM promoters WHERE slug = 'nightfall-collective'").fetch_one(&mut *tx).await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO bookings (demo_session_id, conversation_id, venue_id, space_id, promoter_id, artist_id, title,
                               genre, headcount, starts_at, ends_at, status, quote_cents, deposit_cents,
                               ticket_price_cents, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'ai_proposed', $12, $13, $14, $15)
         RETURNING id",
    )
    .bind(session)
    .bind(p.conversation_id)
    .bind(p.venue.id)
    .bind(p.space.id)
    .bind(promoter)
    .bind(p.artist_id)
    .bind(&p.title)
    .bind(&p.genre)
    .bind(p.headcount)
    .bind(p.starts_at)
    .bind(ends_at)
    .bind(p.space.day_rate_cents)
    .bind(deposit_for(p.space.day_rate_cents))
    .bind(p.ticket_price_cents.or(Some(DEFAULT_TICKET_CENTS)))
    .bind(p.created_by)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    summary(db, id).await
}

/// Withdraws this conversation's open proposals (before the agent proposes new ones).
pub async fn clear_proposals(db: &PgPool, session: Uuid, conversation_id: Uuid) -> AppResult<u64> {
    Ok(sqlx::query(
        "UPDATE bookings SET status = 'cancelled', cancelled_at = now(), updated_at = now()
          WHERE demo_session_id = $1 AND conversation_id = $2 AND status = 'ai_proposed'",
    )
    .bind(session)
    .bind(conversation_id)
    .execute(db)
    .await?
    .rows_affected())
}

/// Withdraws this session's open proposals at a venue (a new search of the booking form replaces them).
pub async fn clear_open_proposals(db: &PgPool, session: Uuid, venue_id: Uuid) -> AppResult<u64> {
    Ok(sqlx::query(
        "UPDATE bookings SET status = 'cancelled', cancelled_at = now(), updated_at = now()
          WHERE demo_session_id = $1 AND venue_id = $2 AND status = 'ai_proposed'",
    )
    .bind(session)
    .bind(venue_id)
    .execute(db)
    .await?
    .rows_affected())
}

/// Proposal -> hold: the promoter requests the slot and the inquiry goes to the venue.
pub async fn request(db: &PgPool, session: Uuid, id: Uuid, inquiry: Option<&str>) -> AppResult<()> {
    let mut tx = db.begin().await?;
    let b = lock_own(&mut tx, session, id).await?;
    if b.status != "ai_proposed" {
        return Err(AppError::Conflict(format!("this booking is already {}", b.status)));
    }
    lock_space(&mut tx, b.space_id).await?;
    if slot_taken(&mut tx, session, b.space_id, b.starts_at, b.ends_at, Some(id)).await? {
        return Err(AppError::Conflict("this slot was just taken — pick another date".into()));
    }
    sqlx::query(
        "UPDATE bookings SET status = 'hold', requested_at = now(), updated_at = now(),
                inquiry_text = COALESCE($2, inquiry_text)
          WHERE id = $1",
    )
    .bind(id)
    .bind(inquiry)
    .execute(&mut *tx)
    .await
    .map_err(|e| conflict_on_constraint(e, "this slot overlaps another booking"))?;
    if let Some(conv) = b.conversation_id {
        sqlx::query(
            "UPDATE bookings SET status = 'cancelled', cancelled_at = now(), updated_at = now()
              WHERE demo_session_id = $1 AND conversation_id = $2 AND status = 'ai_proposed' AND id <> $3",
        )
        .bind(session)
        .bind(conv)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Venue owner approves a hold. From now on the deposit can be paid.
pub async fn approve(db: &PgPool, session: Uuid, id: Uuid) -> AppResult<()> {
    let mut tx = db.begin().await?;
    let b = lock_own(&mut tx, session, id).await?;
    if b.status != "hold" {
        return Err(AppError::Conflict(format!("only holds can be approved (this booking is {})", b.status)));
    }
    sqlx::query("UPDATE bookings SET status = 'approved', approved_at = now(), updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Venue owner declines a hold, or the promoter cancels before paying.
pub async fn cancel(db: &PgPool, session: Uuid, id: Uuid, allowed: &[&str]) -> AppResult<()> {
    let mut tx = db.begin().await?;
    let b = lock_own(&mut tx, session, id).await?;
    if !allowed.contains(&b.status.as_str()) {
        return Err(AppError::Conflict(format!("a booking that is {} cannot be cancelled here", b.status)));
    }
    let capture_in_flight: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM orders WHERE booking_id = $1 AND status = 'approved')",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if capture_in_flight {
        return Err(AppError::Conflict("a PayPal payment for this booking is being confirmed — try again in a minute".into()));
    }
    sqlx::query("UPDATE orders SET status = 'expired' WHERE booking_id = $1 AND status = 'created'")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE bookings SET status = 'cancelled', cancelled_at = now(), updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Drag/resize on the scheduler: only proposals and holds of this session can move.
pub async fn reschedule(
    db: &PgPool,
    session: Uuid,
    id: Uuid,
    space_id: Option<Uuid>,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
) -> AppResult<()> {
    let now = clock::session_now(db, session).await?;
    let mut tx = db.begin().await?;
    let b = lock_own(&mut tx, session, id).await?;
    if !matches!(b.status.as_str(), "ai_proposed" | "hold") {
        return Err(AppError::Conflict("only proposals and holds can be moved".into()));
    }
    let space_id = space_id.unwrap_or(b.space_id);
    let starts = starts_at.unwrap_or(b.starts_at);
    let ends = ends_at.unwrap_or(b.ends_at);
    if ends <= starts || ends - starts > Duration::hours(12) {
        return Err(AppError::BadRequest("a booking must last between a few minutes and 12 hours".into()));
    }
    if starts <= now {
        return Err(AppError::BadRequest("that time is in the past".into()));
    }
    let space: Option<(i64, i32, String)> =
        sqlx::query_as("SELECT day_rate_cents, capacity_max, name FROM spaces WHERE id = $1 AND venue_id = $2")
            .bind(space_id)
            .bind(b.venue_id)
            .fetch_optional(&mut *tx)
            .await?;
    let (rate, capacity_max, space_name) =
        space.ok_or_else(|| AppError::BadRequest("unknown space for this venue".into()))?;
    if let Some(e) = too_small(&space_name, capacity_max, b.headcount) {
        return Err(e);
    }
    lock_space(&mut tx, space_id).await?;
    if slot_taken(&mut tx, session, space_id, starts, ends, Some(id)).await? {
        return Err(AppError::Conflict("that time overlaps another booking".into()));
    }
    sqlx::query(
        "UPDATE bookings SET space_id = $2, starts_at = $3, ends_at = $4, quote_cents = $5, deposit_cents = $6,
                updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(space_id)
    .bind(starts)
    .bind(ends)
    .bind(rate)
    .bind(deposit_for(rate))
    .execute(&mut *tx)
    .await
    .map_err(|e| conflict_on_constraint(e, "that time overlaps another booking"))?;
    tx.commit().await?;
    Ok(())
}
