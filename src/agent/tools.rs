//! The concierge's tools. Every tool runs inside the visitor's demo session, so the model can only
//! see and change that visitor's bookings. Money tools are gated by the server, not by the prompt:
//! `create_deposit_order` is refused unless the venue owner has approved the booking.

use chrono::{NaiveDate, TimeZone, Weekday};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::bookings::{self, BookingSummary, NewProposal, SlotQuery};
use crate::error::AppError;
use crate::routes::venues::{Space, Venue};
use crate::{clock, payments, AppState};

pub fn definitions() -> Value {
    json!([
        {
            "name": "check_availability",
            "description": "Find open slots at this venue between two dates. Returns at most one slot per date (the best-fitting free space for the headcount) with its quote (day rate) and the 30% deposit. Always call this before proposing dates; never guess availability or prices.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "from_date": { "type": "string", "description": "First date to search, YYYY-MM-DD, venue time." },
                    "to_date": { "type": "string", "description": "Last date to search, YYYY-MM-DD, venue time (at most 62 days after from_date)." },
                    "weekdays": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] },
                        "description": "Only these weekdays. Omit for any day."
                    },
                    "headcount": { "type": "integer", "description": "Expected number of guests." },
                    "start_time": { "type": "string", "description": "Start time HH:MM, venue time. Default 19:00." },
                    "hours": { "type": "integer", "description": "Length of the booking in hours, 1-12. Default 4." },
                    "space": { "type": "string", "description": "Only search this space, e.g. \"Rooftop\"." }
                },
                "required": ["from_date", "to_date"],
                "additionalProperties": false
            }
        },
        {
            "name": "find_artists",
            "description": "List artists on MiSFY who could headline, optionally filtered by a genre keyword. Use when the promoter has no artist yet or asks for suggestions.",
            "input_schema": {
                "type": "object",
                "properties": { "genre": { "type": "string", "description": "Genre keyword, e.g. \"synthwave\"." } },
                "additionalProperties": false
            }
        },
        {
            "name": "propose_slots",
            "description": "Show the promoter up to 3 candidate slots from check_availability. They appear as dashed bars on the venue schedule and as cards with a \"Request this slot\" button. Replaces this conversation's earlier proposals. Proposals do not reserve anything.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Event title, e.g. \"Synthwave Night\"." },
                    "slots": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 3,
                        "items": {
                            "type": "object",
                            "properties": {
                                "space": { "type": "string", "description": "Space name exactly as returned by check_availability." },
                                "date": { "type": "string", "description": "YYYY-MM-DD, venue time." },
                                "start_time": { "type": "string", "description": "HH:MM, venue time. Default 19:00." },
                                "hours": { "type": "integer", "description": "1-12. Default 4." }
                            },
                            "required": ["space", "date"],
                            "additionalProperties": false
                        }
                    },
                    "genre": { "type": "string" },
                    "headcount": { "type": "integer" },
                    "artist": { "type": "string", "description": "Headliner name from find_artists, if one was chosen." },
                    "ticket_price_usd": { "type": "number", "description": "Ticket price in USD if the promoter gave one (default 30)." }
                },
                "required": ["title", "slots"],
                "additionalProperties": false
            }
        },
        {
            "name": "request_booking",
            "description": "Send the booking request for ONE proposed slot to the venue owner; the slot becomes a hold. Only call this after the promoter has clearly chosen that slot.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "booking_id": { "type": "string", "description": "booking_id of the chosen proposal." },
                    "inquiry_text": { "type": "string", "description": "A short inquiry for the venue owner: the event, expected guests, genre, technical needs and anything the promoter asked to pass on." }
                },
                "required": ["booking_id", "inquiry_text"],
                "additionalProperties": false
            }
        },
        {
            "name": "get_booking_status",
            "description": "Current state of one of this promoter's bookings (proposal, hold, approved, paid, completed) with deposit and payout details.",
            "input_schema": {
                "type": "object",
                "properties": { "booking_id": { "type": "string" } },
                "required": ["booking_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "create_deposit_order",
            "description": "Create the PayPal order for the 30% deposit of an APPROVED booking so the promoter can pay in the PayPal window. The server refuses this (blocked) unless the venue owner has approved the booking. Only call it when the promoter wants to pay.",
            "input_schema": {
                "type": "object",
                "properties": { "booking_id": { "type": "string" } },
                "required": ["booking_id"],
                "additionalProperties": false
            }
        }
    ])
}

pub struct ToolContext<'a> {
    pub st: &'a AppState,
    pub session: Uuid,
    pub conversation_id: Uuid,
    pub venue: &'a Venue,
    pub spaces: &'a [Space],
    /// Proposals created this turn (shown as cards and dashed bars).
    pub proposals: Vec<BookingSummary>,
}

pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    pub status: &'static str, // "ok" | "blocked" | "error"
    pub summary: String,
}

fn ok(content: Value, summary: impl Into<String>) -> ToolOutput {
    ToolOutput { content: content.to_string(), is_error: false, status: "ok", summary: summary.into() }
}

fn err(message: impl Into<String>) -> ToolOutput {
    let message = message.into();
    ToolOutput { content: json!({ "error": message }).to_string(), is_error: true, status: "error", summary: message }
}

/// Errors the model may see. Internal errors are logged and replaced with a generic message.
fn user_facing(e: AppError) -> String {
    match e {
        AppError::NotFound => "not found (it may belong to another visitor or no longer exist)".into(),
        AppError::BadRequest(m) | AppError::Conflict(m) | AppError::Unavailable(m) | AppError::Upstream(m) => m,
        other => {
            tracing::error!(error = %other, "tool failed");
            "internal error — please try again".into()
        }
    }
}

fn parse<T: DeserializeOwned>(input: &Value) -> Result<T, ToolOutput> {
    serde_json::from_value(input.clone()).map_err(|e| err(format!("invalid input: {e}")))
}

fn parse_date(s: &str) -> Result<NaiveDate, ToolOutput> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map_err(|_| err(format!("invalid date {s:?}, expected YYYY-MM-DD")))
}

fn parse_hour(s: Option<&str>) -> Result<u32, ToolOutput> {
    let Some(s) = s else { return Ok(19) };
    let hour = s.trim().split(':').next().and_then(|h| h.parse::<u32>().ok());
    match hour {
        Some(h) if h < 24 => Ok(h),
        _ => Err(err(format!("invalid start_time {s:?}, expected HH:MM"))),
    }
}

fn usd(cents: i64) -> f64 {
    cents as f64 / 100.0
}

fn find_space<'a>(spaces: &'a [Space], name: &str) -> Option<&'a Space> {
    let n = name.trim().to_lowercase();
    spaces
        .iter()
        .find(|s| s.name.to_lowercase() == n)
        .or_else(|| spaces.iter().find(|s| s.name.to_lowercase().contains(&n) || n.contains(&s.name.to_lowercase())))
}

fn parse_uuid(s: &str) -> Result<Uuid, ToolOutput> {
    Uuid::parse_str(s.trim()).map_err(|_| err("invalid booking_id"))
}

pub async fn run(ctx: &mut ToolContext<'_>, name: &str, input: &Value) -> ToolOutput {
    let result = match name {
        "check_availability" => check_availability(ctx, input).await,
        "find_artists" => find_artists(ctx, input).await,
        "propose_slots" => propose_slots(ctx, input).await,
        "request_booking" => request_booking(ctx, input).await,
        "get_booking_status" => get_booking_status(ctx, input).await,
        "create_deposit_order" => create_deposit_order(ctx, input).await,
        other => Err(err(format!("unknown tool {other}"))),
    };
    result.unwrap_or_else(|e| e)
}

type ToolResult = Result<ToolOutput, ToolOutput>;

async fn check_availability(ctx: &mut ToolContext<'_>, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    struct In {
        from_date: String,
        to_date: String,
        #[serde(default)]
        weekdays: Vec<String>,
        headcount: Option<i32>,
        start_time: Option<String>,
        hours: Option<u32>,
        space: Option<String>,
    }
    let i: In = parse(input)?;
    let weekdays = i
        .weekdays
        .iter()
        .map(|w| w.parse::<Weekday>().map_err(|_| err(format!("invalid weekday {w:?}"))))
        .collect::<Result<Vec<_>, _>>()?;
    let q = SlotQuery {
        from: parse_date(&i.from_date)?,
        to: parse_date(&i.to_date)?,
        headcount: i.headcount.filter(|h| *h > 0),
        weekdays,
        start_hour: parse_hour(i.start_time.as_deref())?,
        hours: i.hours.unwrap_or(4),
        space_name: i.space,
        limit: 8,
    };
    let slots = bookings::free_slots(&ctx.st.db, ctx.session, ctx.venue, ctx.spaces, &q)
        .await
        .map_err(|e| err(user_facing(e)))?;
    let n = slots.len();
    let items: Vec<Value> = slots
        .iter()
        .map(|s| {
            json!({
                "space": s.space_name, "capacity": s.capacity, "date": s.date.to_string(),
                "start": s.local_time, "hours": (s.ends_at - s.starts_at).num_hours(),
                "quote_usd": usd(s.quote_cents), "deposit_usd": usd(s.deposit_cents)
            })
        })
        .collect();
    let mut out = json!({ "slots": items });
    if n == 0 {
        out["note"] = json!("No open slot matches. Try other dates or weekdays, a different start time, or a smaller group.");
    }
    if let Some(h) = q.headcount {
        if !ctx.spaces.iter().any(|s| s.capacity_max >= h) {
            out["note"] = json!(format!("No space here holds {h} guests."));
        }
    }
    Ok(ok(out, format!("{n} open slot{}", if n == 1 { "" } else { "s" })))
}

async fn find_artists(ctx: &mut ToolContext<'_>, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    struct In {
        genre: Option<String>,
    }
    let i: In = parse(input)?;
    let pattern = i.genre.as_deref().map(|g| format!("%{}%", g.trim())).unwrap_or_else(|| "%".into());
    let mut rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT name, genre, country, bio, fee_cents FROM artists WHERE genre ILIKE $1 OR bio ILIKE $1 ORDER BY name",
    )
    .bind(&pattern)
    .fetch_all(&ctx.st.db)
    .await
    .map_err(|e| err(user_facing(e.into())))?;
    let matched = !rows.is_empty();
    if !matched {
        rows = sqlx::query_as("SELECT name, genre, country, bio, fee_cents FROM artists ORDER BY name")
            .fetch_all(&ctx.st.db)
            .await
            .map_err(|e| err(user_facing(e.into())))?;
    }
    let list: Vec<Value> = rows
        .iter()
        .map(|(name, genre, country, bio, fee)| json!({ "name": name, "genre": genre, "country": country, "bio": bio, "typical_fee_usd": usd(*fee) }))
        .collect();
    let n = list.len();
    let mut out = json!({ "artists": list });
    if !matched {
        out["note"] = json!("No artist matched that genre; here is everyone on the roster.");
    }
    Ok(ok(out, format!("{n} artist{}", if n == 1 { "" } else { "s" })))
}

async fn propose_slots(ctx: &mut ToolContext<'_>, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    struct SlotIn {
        space: String,
        date: String,
        start_time: Option<String>,
        hours: Option<u32>,
    }
    #[derive(Deserialize)]
    struct In {
        title: String,
        slots: Vec<SlotIn>,
        genre: Option<String>,
        headcount: Option<i32>,
        artist: Option<String>,
        ticket_price_usd: Option<f64>,
    }
    let i: In = parse(input)?;
    let title: String = i.title.trim().chars().take(120).collect();
    if title.is_empty() {
        return Err(err("title is required"));
    }
    if i.slots.is_empty() || i.slots.len() > 3 {
        return Err(err("propose between 1 and 3 slots"));
    }
    let db = &ctx.st.db;
    let artist: Option<(Uuid, String)> = match i.artist.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => sqlx::query_as("SELECT id, name FROM artists WHERE lower(name) = lower($1) OR slug = lower($1)")
            .bind(a)
            .fetch_optional(db)
            .await
            .map_err(|e| err(user_facing(e.into())))?,
        None => None,
    };
    let ticket_cents = i.ticket_price_usd.map(|p| ((p * 100.0).round() as i64).clamp(100, 50_000));

    bookings::clear_proposals(db, ctx.session, ctx.conversation_id).await.map_err(|e| err(user_facing(e)))?;
    let tz = bookings::venue_tz(ctx.venue);
    let mut proposed = Vec::new();
    let mut errors = Vec::new();
    for s in &i.slots {
        let Some(space) = find_space(ctx.spaces, &s.space) else {
            errors.push(format!("unknown space {:?}", s.space));
            continue;
        };
        let date = match parse_date(&s.date) {
            Ok(d) => d,
            Err(e) => {
                errors.push(e.summary);
                continue;
            }
        };
        let hour = match parse_hour(s.start_time.as_deref()) {
            Ok(h) => h,
            Err(e) => {
                errors.push(e.summary);
                continue;
            }
        };
        let Some(starts_local) = date.and_hms_opt(hour, 0, 0) else { continue };
        let Some(starts_at) = tz.from_local_datetime(&starts_local).single() else { continue };
        let created = bookings::propose(
            db,
            ctx.session,
            NewProposal {
                conversation_id: ctx.conversation_id,
                venue: ctx.venue,
                space,
                starts_at: starts_at.with_timezone(&chrono::Utc),
                hours: s.hours.unwrap_or(4),
                title: title.clone(),
                headcount: i.headcount,
                genre: i.genre.clone(),
                artist_id: artist.as_ref().map(|a| a.0),
                ticket_price_cents: ticket_cents,
            },
        )
        .await;
        match created {
            Ok(b) => proposed.push(b),
            Err(e) => errors.push(format!("{} on {}: {}", space.name, date, user_facing(e))),
        }
    }
    let items: Vec<Value> = proposed
        .iter()
        .map(|b| {
            json!({
                "booking_id": b.id, "space": b.space_name,
                "start": bookings::local_label(b.starts_at, tz),
                "hours": (b.ends_at - b.starts_at).num_hours(),
                "quote_usd": usd(b.quote_cents), "deposit_usd": usd(b.deposit_cents)
            })
        })
        .collect();
    let n = proposed.len();
    ctx.proposals = proposed;
    let mut out = json!({ "proposed": items, "errors": errors });
    if let Some((_, name)) = &artist {
        out["artist"] = json!(name);
    } else if i.artist.is_some() {
        out["note"] = json!("That artist is not on the MiSFY roster, so the proposals have no headliner yet.");
    }
    if n == 0 {
        return Err(ToolOutput { content: out.to_string(), is_error: true, status: "error", summary: "no slot could be proposed".into() });
    }
    Ok(ok(out, format!("{n} slot{} proposed", if n == 1 { "" } else { "s" })))
}

async fn request_booking(ctx: &mut ToolContext<'_>, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    struct In {
        booking_id: String,
        inquiry_text: String,
    }
    let i: In = parse(input)?;
    let id = parse_uuid(&i.booking_id)?;
    let inquiry: String = i.inquiry_text.trim().chars().take(4000).collect();
    bookings::request(&ctx.st.db, ctx.session, id, Some(inquiry.as_str()).filter(|t| !t.is_empty()))
        .await
        .map_err(|e| err(user_facing(e)))?;
    Ok(ok(
        json!({
            "booking_id": id, "status": "hold",
            "next": "The venue owner reviews the request in the owner console. After approval the promoter can pay the 30% deposit with PayPal."
        }),
        "request sent to the venue (hold)",
    ))
}

async fn get_booking_status(ctx: &mut ToolContext<'_>, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    struct In {
        booking_id: String,
    }
    let i: In = parse(input)?;
    let id = parse_uuid(&i.booking_id)?;
    let b = bookings::load(&ctx.st.db, ctx.session, id).await.map_err(|e| err(user_facing(e)))?;
    let tz = bookings::venue_tz(ctx.venue);
    let out = json!({
        "booking_id": id,
        "title": b.summary.title,
        "status": b.summary.status,
        "space": b.summary.space_name,
        "start": bookings::local_label(b.summary.starts_at, tz),
        "quote_usd": usd(b.summary.quote_cents),
        "deposit_usd": usd(b.summary.deposit_cents),
        "deposit_order": b.deposit_order.as_ref().map(|o| json!({ "status": o.status, "confirmed_by": o.confirmed_by })),
        "settlement": b.settlement.as_ref().map(|s| json!({
            "status": s.status,
            "items": s.items.iter().map(|it| json!({ "role": it.role, "amount_usd": usd(it.amount_cents), "status": it.status })).collect::<Vec<_>>()
        })),
        "timeline": b.timeline.iter().map(|t| t.label.clone()).collect::<Vec<_>>()
    });
    Ok(ok(out, format!("status: {}", b.summary.status)))
}

async fn create_deposit_order(ctx: &mut ToolContext<'_>, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    struct In {
        booking_id: String,
    }
    let i: In = parse(input)?;
    let id = parse_uuid(&i.booking_id)?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM bookings WHERE id = $1 AND demo_session_id = $2")
            .bind(id)
            .bind(ctx.session)
            .fetch_optional(&ctx.st.db)
            .await
            .map_err(|e| err(user_facing(e.into())))?;
    let Some(status) = status else { return Err(err("booking not found")) };
    if status != "approved" {
        let reason = match status.as_str() {
            "paid" | "completed" => "the deposit is already paid".to_string(),
            other => format!("the venue owner has not approved this booking yet (status: {other})"),
        };
        return Err(ToolOutput {
            content: json!({
                "blocked": true,
                "reason": format!("BLOCKED by server policy: payment tools unlock only after the venue approves the booking — {reason}.")
            })
            .to_string(),
            is_error: true,
            status: "blocked",
            summary: format!("blocked: {reason}"),
        });
    }
    let order = payments::create_deposit_order(ctx.st, ctx.session, id, Uuid::new_v4())
        .await
        .map_err(|e| err(user_facing(e)))?;
    let now = clock::session_now(&ctx.st.db, ctx.session).await.ok();
    Ok(ok(
        json!({
            "paypal_order_id": order.paypal_order_id,
            "amount_usd": usd(order.amount_cents),
            "currency": order.currency,
            "created_at": now,
            "next": "The PayPal button in the booking panel is ready. The promoter approves the payment in the PayPal window; the booking is confirmed only when PayPal's verified webhook arrives."
        }),
        format!("PayPal order created (${:.2})", usd(order.amount_cents)),
    ))
}
