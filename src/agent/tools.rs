//! The agent's tools. Every tool runs inside the visitor's demo session, so the model can only
//! see and change that visitor's bookings. Money tools are gated by the server, not by the prompt:
//! `create_deposit_order` is refused unless the venue owner has approved the booking.
//!
//! Two agents share them: the venue concierge (one venue, fixed) and the AI venue scout, which works for an
//! artist across the whole venue directory (`search_venues`) and names the venue in each booking tool.

use chrono::{NaiveDate, TimeZone, Weekday};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::bookings::{self, BookingSummary, NewProposal, SlotQuery};
use crate::error::AppError;
use crate::routes::venues::{spaces_of, Space, Venue, VENUE_COLUMNS};
use crate::{clock, payments, AppState};

/// Which agent is talking: the concierge of one venue, or the scout that searches every venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Venue,
    Scout,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Venue => "venue",
            Scope::Scout => "scout",
        }
    }
}

/// The artist the scout works for.
#[derive(Debug, Clone, FromRow)]
pub struct ScoutArtist {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub genre: String,
    pub country: String,
}

const ACTIVITY_NOTE: &str = "Activity 0-100: how many shows MiSFY's discovery data has on record for the venue, \
as a percentile across the directory. It measures how busy a venue's calendar has been, not live popularity.";

fn venue_param() -> Value {
    json!({ "type": "string", "description": "The venue's slug from search_venues (e.g. \"lumen-annex-tyo\")." })
}

fn check_availability_def(scope: Scope) -> Value {
    let mut def = json!({
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
    });
    if scope == Scope::Scout {
        def["description"] = json!("Find open slots at ONE venue between two dates. Returns at most one slot per date (the best-fitting free room for the headcount) with its quote (day rate) and the 30% deposit. Always call this before proposing dates; never guess availability or prices.");
        def["input_schema"]["properties"]["venue"] = venue_param();
        def["input_schema"]["required"] = json!(["venue", "from_date", "to_date"]);
    }
    def
}

fn propose_slots_def(scope: Scope) -> Value {
    let mut def = json!({
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
    });
    if scope == Scope::Scout {
        def["description"] = json!("Show the artist up to 3 candidate slots at ONE venue, from check_availability. They appear as cards with a \"Request this slot\" button and as dashed bars on that venue's schedule. Replaces this conversation's earlier proposals. Proposals do not reserve anything. The artist you work for is the headliner.");
        let props = def["input_schema"]["properties"].as_object_mut().expect("object");
        props.remove("artist");
        props.insert("venue".into(), venue_param());
        props.insert("ticket_price_usd".into(), json!({ "type": "number", "description": "Ticket price in USD if the artist gave one (default 30)." }));
        def["input_schema"]["required"] = json!(["venue", "title", "slots"]);
    }
    def
}

fn request_booking_def(scope: Scope) -> Value {
    let (description, letter) = match scope {
        Scope::Venue => (
            "Send the booking request for ONE proposed slot to the venue owner; the slot becomes a hold. Only call this after the promoter has clearly chosen that slot.",
            "A short inquiry for the venue owner: the event, expected guests, genre, technical needs and anything the promoter asked to pass on.",
        ),
        Scope::Scout => (
            "Send the booking proposal for ONE proposed slot to the venue owner; the slot becomes a hold. Only call this after the artist has clearly chosen that slot and agreed to send.",
            "The proposal letter you write for the artist (under 120 words): who they are, the show and date, expected crowd, why this venue fits (its genres and how active it is), ticket price and technical needs.",
        ),
    };
    json!({
        "name": "request_booking",
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": {
                "booking_id": { "type": "string", "description": "booking_id of the chosen proposal." },
                "inquiry_text": { "type": "string", "description": letter }
            },
            "required": ["booking_id", "inquiry_text"],
            "additionalProperties": false
        }
    })
}

fn search_venues_def() -> Value {
    json!({
        "name": "search_venues",
        "description": "Search MiSFY's venue directory. Venues are ranked by activity (0-100): how many shows MiSFY's discovery data has on record for the venue, as a percentile across the directory. Activity shows how busy a venue's calendar has been; it is not live or real-time popularity. Genres are broad categories: Electronic / DJ, Rock / Indie, Vocal / Soul, Pop / Hip-hop, Jazz, Techno / House. Search with the category closest to the artist's genre (for example synthwave or dream pop -> Electronic / DJ, garage rock or indie folk -> Rock / Indie). Returns each venue's slug (use it with check_availability and propose_slots), city, genres, room capacity, shows on record, activity and the lowest quote. Never invent venues or numbers.",
        "input_schema": {
            "type": "object",
            "properties": {
                "place": { "type": "string", "description": "City, country or region, e.g. \"Tokyo\", \"Germany\", \"Europe\", \"North America\". Several separated by commas. Omit for anywhere." },
                "genre": { "type": "string", "description": "A broad genre category or keyword, e.g. \"Electronic\", \"Rock\", \"Jazz\"." },
                "min_capacity": { "type": "integer", "description": "The room must hold at least this many guests." },
                "max_capacity": { "type": "integer", "description": "The room must hold at most this many guests." },
                "sort": { "type": "string", "enum": ["activity", "size"], "description": "Default activity." },
                "limit": { "type": "integer", "description": "1-10, default 5." }
            },
            "additionalProperties": false
        }
    })
}

pub fn definitions(scope: Scope) -> Value {
    let get_booking_status = json!({
        "name": "get_booking_status",
        "description": format!("Current state of one of this {}'s bookings (proposal, hold, approved, paid, completed) with deposit and payout details.", if scope == Scope::Scout { "artist" } else { "promoter" }),
        "input_schema": {
            "type": "object",
            "properties": { "booking_id": { "type": "string" } },
            "required": ["booking_id"],
            "additionalProperties": false
        }
    });
    let create_deposit_order = json!({
        "name": "create_deposit_order",
        "description": format!("Create the PayPal order for the 30% deposit of an APPROVED booking so the {} can pay in the PayPal window. The server refuses this (blocked) unless the venue owner has approved the booking. Only call it when they want to pay.", if scope == Scope::Scout { "artist" } else { "promoter" }),
        "input_schema": {
            "type": "object",
            "properties": { "booking_id": { "type": "string" } },
            "required": ["booking_id"],
            "additionalProperties": false
        }
    });
    match scope {
        Scope::Venue => json!([
            check_availability_def(scope),
            {
                "name": "find_artists",
                "description": "List artists on MiSFY who could headline, optionally filtered by a genre keyword. Use when the promoter has no artist yet or asks for suggestions.",
                "input_schema": {
                    "type": "object",
                    "properties": { "genre": { "type": "string", "description": "Genre keyword, e.g. \"synthwave\"." } },
                    "additionalProperties": false
                }
            },
            propose_slots_def(scope),
            request_booking_def(scope),
            get_booking_status,
            create_deposit_order
        ]),
        Scope::Scout => json!([
            search_venues_def(),
            check_availability_def(scope),
            propose_slots_def(scope),
            request_booking_def(scope),
            get_booking_status,
            create_deposit_order
        ]),
    }
}

/// One venue in a search result (also sent to the app, which shows it as a card).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct VenueHit {
    pub slug: String,
    pub name: String,
    pub city: String,
    pub country: String,
    pub region: Option<String>,
    pub kind: Option<String>,
    pub genres: Vec<String>,
    pub capacity: i32,
    pub shows_on_record: Option<i32>,
    pub activity: i32,
    pub quote_from_cents: i64,
    pub featured: bool,
    pub data_source: String,
    #[serde(skip)]
    pub total: i64,
}

pub struct ToolContext<'a> {
    pub st: &'a AppState,
    pub session: Uuid,
    pub conversation_id: Uuid,
    /// The concierge's venue; `None` for the scout (each booking tool names its venue).
    pub venue: Option<&'a Venue>,
    pub spaces: &'a [Space],
    /// The scout's artist (the headliner of everything it proposes).
    pub artist: Option<&'a ScoutArtist>,
    /// Proposals created this turn (shown as cards and dashed bars).
    pub proposals: Vec<BookingSummary>,
    /// The latest search_venues result this turn (shown as venue cards).
    pub venues_found: Vec<VenueHit>,
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

/// The venue a booking tool works on: the concierge's own venue, or the one the scout named.
async fn venue_for(ctx: &ToolContext<'_>, key: Option<&str>) -> Result<(Venue, Vec<Space>), ToolOutput> {
    if let Some(v) = ctx.venue {
        return Ok((v.clone(), ctx.spaces.to_vec()));
    }
    let key = key
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .ok_or_else(|| err("venue is required: pass the venue's slug from search_venues"))?;
    let venue = sqlx::query_as::<_, Venue>(&format!(
        "SELECT {VENUE_COLUMNS} FROM venues WHERE slug = lower($1) OR lower(name) = lower($1) ORDER BY featured DESC LIMIT 1"
    ))
    .bind(key)
    .fetch_optional(&ctx.st.db)
    .await
    .map_err(|e| err(user_facing(e.into())))?
    .ok_or_else(|| err(format!("no venue {key:?} in the directory — use a slug from search_venues")))?;
    let spaces = spaces_of(&ctx.st.db, venue.id).await.map_err(|e| err(user_facing(e)))?;
    Ok((venue, spaces))
}

pub async fn run(ctx: &mut ToolContext<'_>, name: &str, input: &Value) -> ToolOutput {
    let result = match name {
        "search_venues" => search_venues(ctx, input).await,
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

/// ISO code for a country the model may name in words.
fn country_code(s: &str) -> Option<&'static str> {
    let k = s.trim().to_lowercase();
    let k = k.strip_prefix("the ").unwrap_or(&k);
    Some(match k {
        "korea" | "south korea" | "republic of korea" | "kr" => "KR",
        "japan" | "jp" => "JP",
        "taiwan" | "tw" => "TW",
        "hong kong" | "hk" => "HK",
        "thailand" | "th" => "TH",
        "singapore" | "sg" => "SG",
        "indonesia" | "id" => "ID",
        "philippines" | "ph" => "PH",
        "malaysia" | "my" => "MY",
        "vietnam" | "viet nam" | "vn" => "VN",
        "australia" | "au" => "AU",
        "usa" | "us" | "united states" | "united states of america" | "america" => "US",
        "uk" | "gb" | "united kingdom" | "britain" | "great britain" | "england" => "GB",
        "germany" | "de" => "DE",
        "netherlands" | "the netherlands" | "holland" | "nl" => "NL",
        "spain" | "es" => "ES",
        "italy" | "it" => "IT",
        "france" | "fr" => "FR",
        "mexico" | "mx" => "MX",
        "brazil" | "br" => "BR",
        "canada" | "ca" => "CA",
        _ => return None,
    })
}

/// Splits "Tokyo, Japan" into ILIKE patterns for city/region and ISO codes for the country.
fn place_filters(place: Option<&str>) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let Some(place) = place.map(str::trim).filter(|p| !p.is_empty()) else { return (None, None) };
    let mut patterns = Vec::new();
    let mut codes = Vec::new();
    for part in place.split([',', '/', ';']).map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(code) = country_code(part) {
            codes.push(code.to_string());
        }
        if part.chars().count() > 2 {
            patterns.push(format!("%{part}%"));
        }
    }
    ((!patterns.is_empty()).then_some(patterns), (!codes.is_empty()).then_some(codes))
}

async fn search_venues(ctx: &mut ToolContext<'_>, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    struct In {
        place: Option<String>,
        genre: Option<String>,
        min_capacity: Option<i32>,
        max_capacity: Option<i32>,
        sort: Option<String>,
        limit: Option<i64>,
    }
    let i: In = parse(input)?;
    let (patterns, codes) = place_filters(i.place.as_deref());
    // "Electronic / DJ" -> "electronic": match on the words, not the slashes.
    let genre = i
        .genre
        .as_deref()
        .map(|g| g.split('/').next().unwrap_or(g).trim().to_string())
        .filter(|g| !g.is_empty());
    let sort = match i.sort.as_deref() {
        Some("size") => "size",
        _ => "activity",
    };
    let limit = i.limit.unwrap_or(5).clamp(1, 10);
    let hits = sqlx::query_as::<_, VenueHit>(
        "WITH room AS (SELECT venue_id, max(capacity_max) AS capacity, min(day_rate_cents) AS quote_from_cents
                         FROM spaces GROUP BY venue_id)
         SELECT v.slug, v.name, v.city, v.country, v.region, v.kind, v.genres, room.capacity, v.shows_on_record,
                v.activity, room.quote_from_cents, v.featured, v.data_source, count(*) OVER () AS total
           FROM venues v JOIN room ON room.venue_id = v.id
          WHERE v.activity IS NOT NULL
            AND (($1::text[] IS NULL AND $2::text[] IS NULL)
                 OR v.city ILIKE ANY($1) OR v.region ILIKE ANY($1) OR v.country = ANY($2))
            AND ($3::text IS NULL OR EXISTS (SELECT 1 FROM unnest(v.genres) g
                                              WHERE g ILIKE '%' || $3 || '%' OR $3 ILIKE '%' || g || '%'))
            AND ($4::int IS NULL OR room.capacity >= $4)
            AND ($5::int IS NULL OR room.capacity <= $5)
          ORDER BY CASE $6 WHEN 'size' THEN room.capacity ELSE v.activity END DESC,
                   v.activity DESC, v.name
          LIMIT $7",
    )
    .bind(&patterns)
    .bind(&codes)
    .bind(&genre)
    .bind(i.min_capacity.filter(|c| *c > 0))
    .bind(i.max_capacity.filter(|c| *c > 0))
    .bind(sort)
    .bind(limit)
    .fetch_all(&ctx.st.db)
    .await
    .map_err(|e| err(user_facing(e.into())))?;

    let total = hits.first().map(|h| h.total).unwrap_or(0);
    let items: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({
                "venue": h.slug, "name": h.name, "city": h.city, "country": h.country, "kind": h.kind,
                "genres": h.genres, "room_capacity": h.capacity, "shows_on_record": h.shows_on_record,
                "activity": h.activity, "quote_from_usd": usd(h.quote_from_cents)
            })
        })
        .collect();
    let mut out = json!({ "venues": items, "matched": total, "ranked_by": sort, "activity": ACTIVITY_NOTE });
    if hits.iter().any(|h| h.data_source == "discovery") {
        out["data_note"] = json!("City, genres and shows on record come from MiSFY's discovery data (anonymized, rounded). Venue names, rooms and prices are fictional in this demo.");
    } else if !hits.is_empty() {
        out["data_note"] = json!("Sample directory: venue names and numbers are fictional in this demo.");
    }
    if hits.is_empty() {
        let cities: Vec<String> =
            sqlx::query_scalar("SELECT DISTINCT city FROM venues WHERE activity IS NOT NULL ORDER BY city")
                .fetch_all(&ctx.st.db)
                .await
                .unwrap_or_default();
        let genres: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT g FROM venues, unnest(genres) g WHERE activity IS NOT NULL ORDER BY g",
        )
        .fetch_all(&ctx.st.db)
        .await
        .unwrap_or_default();
        out["note"] = json!(format!(
            "No venue matched. Try a broader place, another genre category or another size. Cities in the directory: {}. Genre categories: {}.",
            cities.join(", "),
            genres.join(", ")
        ));
    }
    let n = hits.len();
    let summary = match n {
        0 => "no venue matched".to_string(),
        _ => format!("{n} of {total} venue{} ranked by {sort}", if total == 1 { "" } else { "s" }),
    };
    ctx.venues_found = hits;
    Ok(ok(out, summary))
}

async fn check_availability(ctx: &mut ToolContext<'_>, input: &Value) -> ToolResult {
    #[derive(Deserialize)]
    struct In {
        venue: Option<String>,
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
    let (venue, spaces) = venue_for(ctx, i.venue.as_deref()).await?;
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
    let slots = bookings::free_slots(&ctx.st.db, ctx.session, &venue, &spaces, &q)
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
    if ctx.venue.is_none() {
        out["venue"] = json!(venue.slug);
        out["venue_name"] = json!(venue.name);
        out["city"] = json!(venue.city);
    }
    if n == 0 {
        out["note"] = json!("No open slot matches. Try other dates or weekdays, a different start time, or a smaller group.");
    }
    if let Some(h) = q.headcount {
        if !spaces.iter().any(|s| s.capacity_max >= h) {
            out["note"] = json!(format!("No space at {} holds {h} guests.", venue.name));
        }
    }
    let at = if ctx.venue.is_none() { format!(" at {}", venue.name) } else { String::new() };
    Ok(ok(out, format!("{n} open slot{}{at}", if n == 1 { "" } else { "s" })))
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
        venue: Option<String>,
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
    let (venue, spaces) = venue_for(ctx, i.venue.as_deref()).await?;
    let db = &ctx.st.db;
    let artist: Option<(Uuid, String)> = match i.artist.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => sqlx::query_as("SELECT id, name FROM artists WHERE lower(name) = lower($1) OR slug = lower($1)")
            .bind(a)
            .fetch_optional(db)
            .await
            .map_err(|e| err(user_facing(e.into())))?,
        None => ctx.artist.map(|a| (a.id, a.name.clone())),
    };
    let genre = i.genre.clone().or_else(|| ctx.artist.map(|a| a.genre.clone()));
    let ticket_cents = i.ticket_price_usd.map(|p| ((p * 100.0).round() as i64).clamp(100, 50_000));

    bookings::clear_proposals(db, ctx.session, ctx.conversation_id).await.map_err(|e| err(user_facing(e)))?;
    let tz = bookings::venue_tz(&venue);
    let mut proposed = Vec::new();
    let mut errors = Vec::new();
    for s in &i.slots {
        let Some(space) = find_space(&spaces, &s.space) else {
            errors.push(format!("unknown space {:?} at {}", s.space, venue.name));
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
                created_by: "agent",
                venue: &venue,
                space,
                starts_at: starts_at.with_timezone(&chrono::Utc),
                hours: s.hours.unwrap_or(4),
                title: title.clone(),
                headcount: i.headcount,
                genre: genre.clone(),
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
                "booking_id": b.id, "venue": b.venue_name, "space": b.space_name,
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
    let at = if ctx.venue.is_none() { format!(" at {}", venue.name) } else { String::new() };
    Ok(ok(out, format!("{n} slot{} proposed{at}", if n == 1 { "" } else { "s" })))
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
    let who = if ctx.venue.is_none() { "artist" } else { "promoter" };
    Ok(ok(
        json!({
            "booking_id": id, "status": "hold",
            "next": format!("The venue owner reviews the request in the owner console. After approval the {who} can pay the 30% deposit with PayPal.")
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
    let tz = bookings::tz_from_minutes(b.summary.venue_tz);
    let out = json!({
        "booking_id": id,
        "title": b.summary.title,
        "status": b.summary.status,
        "venue": b.summary.venue_name,
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
    let next = if ctx.venue.is_none() {
        "The PayPal button on the venue's card (Your booking) is ready. The artist approves the payment in the PayPal window; the booking is confirmed only when PayPal's verified webhook arrives."
    } else {
        "The PayPal button in the booking panel is ready. The promoter approves the payment in the PayPal window; the booking is confirmed only when PayPal's verified webhook arrives."
    };
    Ok(ok(
        json!({
            "paypal_order_id": order.paypal_order_id,
            "amount_usd": usd(order.amount_cents),
            "currency": order.currency,
            "created_at": now,
            "next": next
        }),
        format!("PayPal order created (${:.2})", usd(order.amount_cents)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_filters_split_cities_and_countries() {
        let (p, c) = place_filters(Some("Tokyo, Japan"));
        assert_eq!(p.unwrap(), vec!["%Tokyo%".to_string(), "%Japan%".to_string()]);
        assert_eq!(c.unwrap(), vec!["JP".to_string()]);
        // A two-letter code must not become a substring pattern ("US" would match "Busan").
        let (p, c) = place_filters(Some("US"));
        assert!(p.is_none());
        assert_eq!(c.unwrap(), vec!["US".to_string()]);
        assert_eq!(place_filters(Some("  ")), (None, None));
        let (p, c) = place_filters(Some("Southeast Asia"));
        assert_eq!(p.unwrap(), vec!["%Southeast Asia%".to_string()]);
        assert!(c.is_none());
    }

    #[test]
    fn scout_tools_name_the_venue() {
        let defs = definitions(Scope::Scout);
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(names[0], "search_venues");
        assert!(!names.contains(&"find_artists"));
        let check = defs.as_array().unwrap().iter().find(|d| d["name"] == "check_availability").unwrap();
        assert!(check["input_schema"]["required"].as_array().unwrap().contains(&json!("venue")));
        let propose = defs.as_array().unwrap().iter().find(|d| d["name"] == "propose_slots").unwrap();
        assert!(propose["input_schema"]["properties"].get("artist").is_none());
        // The concierge's tools are unchanged.
        let venue_defs = definitions(Scope::Venue);
        let check = venue_defs.as_array().unwrap().iter().find(|d| d["name"] == "check_availability").unwrap();
        assert!(check["input_schema"]["properties"].get("venue").is_none());
    }
}
