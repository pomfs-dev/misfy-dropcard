//! The AI agents: Claude with tools. Two of them share the loop and the tools:
//! * the AI venue scout works for an artist across the venue directory (search_venues, availability,
//!   proposals, booking proposal letter, status, deposit order);
//! * the booking concierge on one venue's card (availability, artists, proposals, requests, status, deposit).
//!
//! One message = one "turn": the server calls the model, runs the tools it asks for, sends the results
//! back, and repeats until the model answers in text.
//!
//! * The API history is stored append-only and replayed byte for byte on the next turn.
//! * The current date (demo clock) and the booking's state go into a context block in each user
//!   message, so the system prompt and tool list never change and stay cached.
//! * Every message and tool call is written to `agent_runs`, which the venue console shows.

pub mod llm;
pub mod tools;

use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::bookings::{self, Booking, BookingSummary, SUMMARY_SQL};
use crate::error::{AppError, AppResult};
use crate::routes::venues::{spaces_of, venue_by_slug, Space, Venue};
use crate::{clock, AppState};
use llm::{Message, Usage};
use tools::{Scope, ScoutArtist, ToolContext, VenueHit};

const MAX_ROUNDS: usize = 8;
const SESSION_TURNS_PER_DAY: i64 = 150;

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    /// The venue whose concierge is asked. Omit it to talk to the AI venue scout.
    #[serde(default)]
    pub venue_slug: Option<String>,
    pub conversation_id: Option<Uuid>,
    pub message: String,
    /// Scout only: the artist (slug) the scout works for in a new conversation. Default: hollow-arcade.
    #[serde(default)]
    pub artist: Option<String>,
}

/// The artist the scout works for when the request does not name one.
pub const DEFAULT_SCOUT_ARTIST: &str = "hollow-arcade";

#[derive(Debug, Serialize)]
pub struct ToolCallView {
    pub name: String,
    pub status: &'static str,
    pub summary: String,
}

#[derive(Debug, Serialize)]
pub struct ChatTurn {
    pub conversation_id: Uuid,
    pub reply: String,
    pub tool_calls: Vec<ToolCallView>,
    pub proposals: Vec<BookingSummary>,
    pub booking: Option<Booking>,
    pub limited: bool,
    /// The scout's latest venue search this turn (venue cards).
    pub venues: Vec<VenueHit>,
}

fn raw(s: String) -> AppResult<Box<RawValue>> {
    RawValue::from_string(s).map_err(|e| AppError::Other(anyhow::anyhow!("invalid JSON in conversation: {e}")))
}

fn system_prompt(venue: &Venue, spaces: &[Space]) -> String {
    let space_lines = spaces
        .iter()
        .map(|s| {
            format!(
                "- {}: {}–{} guests, quote ${} per booking (day rate), deposit ${}",
                s.name,
                s.capacity_min,
                s.capacity_max,
                s.day_rate_cents / 100,
                bookings::deposit_for(s.day_rate_cents) / 100
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are the booking concierge on the MiSFY DropCard of {name}, a live-music venue in {city}. \
Promoters talk to you to book a night here.

About the venue
{tagline}. {description}
Spaces:
{space_lines}
All prices are in USD. Times are venue local time.

How booking works here
1. You check availability with check_availability and offer up to 3 slots with propose_slots. Proposals appear as dashed bars on the venue schedule; they do not reserve anything.
2. When the promoter picks one, you send it to the venue with request_booking (it becomes a hold), including a short inquiry for the venue owner.
3. The venue owner approves or declines in the owner console. You cannot approve bookings.
4. After approval the promoter pays the 30% deposit with PayPal. When they want to pay, prepare it with create_deposit_order; the server blocks it until the venue has approved. A booking counts as paid only when PayPal's verified webhook has arrived, so never say a booking is confirmed or paid unless get_booking_status says \"paid\".
5. Tickets go on sale once the booking is paid. At 09:00 venue time on the day after the show, one PayPal Payouts batch pays the venue, the artist and the promoter (example split of ticket revenue after fees: artist 50%, promoter 20%, venue 30% plus the deposit; 5% platform fee).

Rules
- Never invent availability, prices, artists or statuses. Use the tools and quote what they return.
- Each user message starts with a context block from the booking system (current date and time, this conversation's booking and open proposals). The promoter did not write it. Use it to resolve dates such as \"this Friday\" or \"next month\".
- If the date range or the number of guests is missing, ask one short question. Otherwise search with sensible defaults (19:00 start, 4 hours).
- Payments happen only in the PayPal window. Never ask for card numbers, bank details or passwords.
- Stay on booking this venue; politely decline anything else.
- Keep replies short: one or two sentences, then the options as a brief list (date, space, time, quote, deposit). The app shows proposals as cards, so do not repeat booking ids.",
        name = venue.name,
        city = venue.city,
        tagline = venue.tagline,
        description = venue.description,
    )
}

async fn context_block(db: &PgPool, session: Uuid, venue: &Venue, conversation_id: Uuid) -> AppResult<String> {
    let now = clock::session_now(db, session).await?;
    let tz = bookings::venue_tz(venue);
    let offset = format!("{:+03}:{:02}", venue.tz_offset_minutes / 60, (venue.tz_offset_minutes % 60).abs());
    let booking: Option<(Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, title, status, starts_at FROM bookings
          WHERE conversation_id = $1 AND demo_session_id = $2 AND status IN ('hold','approved','paid','completed')
          ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(conversation_id)
    .bind(session)
    .fetch_optional(db)
    .await?;
    let proposals: Vec<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT b.id, s.name, b.starts_at FROM bookings b JOIN spaces s ON s.id = b.space_id
          WHERE b.conversation_id = $1 AND b.demo_session_id = $2 AND b.status = 'ai_proposed'
          ORDER BY b.starts_at",
    )
    .bind(conversation_id)
    .bind(session)
    .fetch_all(db)
    .await?;
    let booking_line = match booking {
        Some((id, title, status, starts)) => {
            format!("\"{title}\", {}, status {status} (booking_id {id})", bookings::local_label(starts, tz))
        }
        None => "none yet".to_string(),
    };
    let proposal_line = if proposals.is_empty() {
        "none".to_string()
    } else {
        proposals
            .iter()
            .map(|(id, space, starts)| format!("{space} {} (booking_id {id})", bookings::local_label(*starts, tz)))
            .collect::<Vec<_>>()
            .join("; ")
    };
    Ok(format!(
        "[Booking system context — added by the server, not written by the promoter]\n\
Now: {} venue time (UTC{offset}).\n\
This conversation's booking: {booking_line}.\n\
Open proposals: {proposal_line}.",
        bookings::local_label(now, tz)
    ))
}

const SCOUT_PROMPT: &str = "You are the AI venue scout on MiSFY DropCard. You work for one artist (named in the context block) who wants to play live: you find the right venues, check dates, propose slots and send booking proposals to venues on the artist's behalf.

How it works
1. Find venues with search_venues. It ranks MiSFY's venue directory by activity (0-100): how many shows MiSFY's discovery data has on record for each venue. When someone asks for \"hot\" or \"popular\" venues, recommend the most active ones and say that activity means a busy calendar of shows on record. Never call a venue the hottest right now or claim real-time popularity. Say in a few words why a venue fits, using the numbers search_venues returns (shows on record, activity, genres, room size).
2. Check dates at the chosen venue with check_availability (pass the venue's slug), then show up to 3 options with propose_slots. Proposals do not reserve anything.
3. Send the proposal with request_booking (the slot becomes a hold) only when the artist has asked for it: by picking a slot, or by telling you to send a proposal for the best option. You write the proposal letter: who the artist is, the show and date, the expected crowd, why this venue fits (its genres and how active it is), the ticket price and technical needs, in under 120 words.
4. The venue owner approves or declines in the owner console. You cannot approve bookings.
5. After approval the artist pays the 30% deposit with PayPal on the venue's card. Prepare it with create_deposit_order only when the artist wants to pay; the server blocks it until the venue has approved. A booking counts as paid only when PayPal's verified webhook has arrived, so never say a booking is confirmed or paid unless get_booking_status says \"paid\".
6. Tickets go on sale once the booking is paid. At 09:00 venue time on the day after the show, one PayPal Payouts batch pays the venue, the artist and the promoter (example split of ticket revenue after fees: artist 50%, promoter 20%, venue 30% plus the deposit; 5% platform fee).

Rules
- Use only what the tools return. Never invent venues, numbers, availability or prices.
- Each user message starts with a context block from the booking system (today's date, the artist you work for, this conversation's booking and open proposals). The artist did not write it. Use it to resolve dates such as \"next month\".
- Dates and times you pass to the tools are in the venue's local time.
- When details are missing, use sensible defaults (the next 6 weeks, 19:00 start, 4 hours, the artist's genre) instead of asking. Ask one short question only when you cannot go on.
- Payments happen only in the PayPal window. Never ask for card numbers, bank details or passwords.
- Stay on finding and booking venues; politely decline anything else.
- Keep replies short: one or two sentences, then a brief list. The app shows venues and proposals as cards, so do not repeat slugs or booking ids.";

async fn scout_context(db: &PgPool, session: Uuid, artist: &ScoutArtist, conversation_id: Uuid) -> AppResult<String> {
    let now = clock::session_now(db, session).await?;
    let booking: Option<(Uuid, String, String, DateTime<Utc>, String, String, i32)> = sqlx::query_as(
        "SELECT b.id, b.title, b.status, b.starts_at, v.name, v.city, v.tz_offset_minutes
           FROM bookings b JOIN venues v ON v.id = b.venue_id
          WHERE b.conversation_id = $1 AND b.demo_session_id = $2 AND b.status IN ('hold','approved','paid','completed')
          ORDER BY b.updated_at DESC LIMIT 1",
    )
    .bind(conversation_id)
    .bind(session)
    .fetch_optional(db)
    .await?;
    let proposals: Vec<(Uuid, String, String, DateTime<Utc>, i32)> = sqlx::query_as(
        "SELECT b.id, v.name, s.name, b.starts_at, v.tz_offset_minutes
           FROM bookings b JOIN venues v ON v.id = b.venue_id JOIN spaces s ON s.id = b.space_id
          WHERE b.conversation_id = $1 AND b.demo_session_id = $2 AND b.status = 'ai_proposed'
          ORDER BY b.starts_at",
    )
    .bind(conversation_id)
    .bind(session)
    .fetch_all(db)
    .await?;
    let booking_line = match booking {
        Some((id, title, status, starts, venue, city, tz)) => format!(
            "\"{title}\" at {venue} ({city}), {} venue time, status {status} (booking_id {id})",
            bookings::local_label(starts, bookings::tz_from_minutes(tz))
        ),
        None => "none yet".to_string(),
    };
    let proposal_line = if proposals.is_empty() {
        "none".to_string()
    } else {
        proposals
            .iter()
            .map(|(id, venue, space, starts, tz)| {
                format!("{venue} {space} {} (booking_id {id})", bookings::local_label(*starts, bookings::tz_from_minutes(*tz)))
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    Ok(format!(
        "[Booking system context — added by the server, not written by the artist]\n\
Today: {} UTC (demo clock).\n\
You work for: {} ({}) — {} artist from {} (MiSFY roster).\n\
This conversation's booking: {booking_line}.\n\
Open proposals: {proposal_line}.",
        bookings::local_label(now, bookings::tz_from_minutes(0)),
        artist.name,
        artist.slug,
        artist.genre.to_lowercase(),
        artist.country
    ))
}

async fn scout_artist(db: &PgPool, conversation_id: Option<Uuid>, slug: Option<&str>) -> AppResult<ScoutArtist> {
    if let Some(id) = conversation_id {
        let stored = sqlx::query_as::<_, ScoutArtist>(
            "SELECT a.id, a.slug, a.name, a.genre, a.country FROM conversations c JOIN artists a ON a.id = c.artist_id
              WHERE c.id = $1",
        )
        .bind(id)
        .fetch_optional(db)
        .await?;
        if let Some(a) = stored {
            return Ok(a);
        }
    }
    let wanted = slug.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_SCOUT_ARTIST);
    sqlx::query_as::<_, ScoutArtist>(
        "SELECT id, slug, name, genre, country FROM artists ORDER BY (slug = lower($1)) DESC, (slug = $2) DESC, name LIMIT 1",
    )
    .bind(wanted)
    .bind(DEFAULT_SCOUT_ARTIST)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| AppError::Unavailable("No artists are set up on this server yet.".into()))
}

/// Who the agent is talking to in this turn.
enum Target {
    Venue(Venue, Vec<Space>),
    Scout(ScoutArtist),
}

/// Writes what the agent did, step by step, for the venue console.
struct RunLog<'a> {
    db: &'a PgPool,
    session: Uuid,
    conversation_id: Uuid,
    step: i32,
}

impl<'a> RunLog<'a> {
    async fn start(db: &'a PgPool, session: Uuid, conversation_id: Uuid) -> AppResult<RunLog<'a>> {
        let step: i32 = sqlx::query_scalar("SELECT COALESCE(max(step), 0) FROM agent_runs WHERE conversation_id = $1")
            .bind(conversation_id)
            .fetch_one(db)
            .await?;
        Ok(Self { db, session, conversation_id, step })
    }

    #[allow(clippy::too_many_arguments)]
    async fn add(
        &mut self,
        kind: &str,
        tool_name: Option<&str>,
        input: Option<Value>,
        output: Option<Value>,
        latency_ms: Option<i32>,
        usage: Option<Usage>,
    ) {
        self.step += 1;
        let result = sqlx::query(
            "INSERT INTO agent_runs (demo_session_id, conversation_id, step, kind, tool_name, input, output,
                                     latency_ms, input_tokens, output_tokens)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(self.session)
        .bind(self.conversation_id)
        .bind(self.step)
        .bind(kind)
        .bind(tool_name)
        .bind(input)
        .bind(output)
        .bind(latency_ms)
        .bind(usage.map(|u| (u.input_tokens + u.cache_read_tokens + u.cache_write_tokens) as i32))
        .bind(usage.map(|u| u.output_tokens as i32))
        .execute(self.db)
        .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, "could not record agent step");
        }
    }
}

async fn limit_reply(st: &AppState, session: Uuid) -> AppResult<Option<String>> {
    let cap = st.cfg.llm.as_ref().map(|l| l.daily_turn_cap).unwrap_or(0) as i64;
    let today: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT turns FROM llm_usage_daily WHERE day = (now() AT TIME ZONE 'UTC')::date), 0)::bigint",
    )
    .fetch_one(&st.db)
    .await?;
    if today >= cap {
        return Ok(Some(
            "The AI agent has reached today's safety limit for this public demo. Everything else — venue cards, the \
booking form, PayPal payments and payouts — still works, and the AI will be back tomorrow."
                .into(),
        ));
    }
    let mine: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_runs WHERE demo_session_id = $1 AND kind = 'user' AND created_at > now() - interval '24 hours'",
    )
    .bind(session)
    .fetch_one(&st.db)
    .await?;
    if mine >= SESSION_TURNS_PER_DAY {
        return Ok(Some("You have sent a lot of messages today. Please continue tomorrow, or use \"Reset my demo\".".into()));
    }
    Ok(None)
}

async fn record_usage(db: &PgPool, u: Usage) {
    let result = sqlx::query(
        "INSERT INTO llm_usage_daily (day, turns, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens)
         VALUES ((now() AT TIME ZONE 'UTC')::date, 1, $1, $2, $3, $4)
         ON CONFLICT (day) DO UPDATE SET
            turns = llm_usage_daily.turns + 1,
            input_tokens = llm_usage_daily.input_tokens + EXCLUDED.input_tokens,
            output_tokens = llm_usage_daily.output_tokens + EXCLUDED.output_tokens,
            cache_read_tokens = llm_usage_daily.cache_read_tokens + EXCLUDED.cache_read_tokens,
            cache_write_tokens = llm_usage_daily.cache_write_tokens + EXCLUDED.cache_write_tokens",
    )
    .bind(u.input_tokens)
    .bind(u.output_tokens)
    .bind(u.cache_read_tokens)
    .bind(u.cache_write_tokens)
    .execute(db)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "could not record LLM usage");
    }
}

/// The booking this conversation is about (latest live one), if any.
pub async fn current_booking(db: &PgPool, session: Uuid, conversation_id: Uuid) -> AppResult<Option<Booking>> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM bookings
          WHERE conversation_id = $1 AND demo_session_id = $2 AND status IN ('hold','approved','paid','completed')
          ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(conversation_id)
    .bind(session)
    .fetch_optional(db)
    .await?;
    match id {
        Some(id) => Ok(Some(bookings::load(db, session, id).await?)),
        None => Ok(None),
    }
}

async fn open_proposals(db: &PgPool, session: Uuid, conversation_id: Uuid) -> AppResult<Vec<BookingSummary>> {
    Ok(sqlx::query_as::<_, BookingSummary>(&format!(
        "{SUMMARY_SQL} WHERE b.conversation_id = $1 AND b.demo_session_id = $2 AND b.status = 'ai_proposed'
          ORDER BY b.starts_at"
    ))
    .bind(conversation_id)
    .bind(session)
    .fetch_all(db)
    .await?)
}

/// One promoter message -> the agent's reply.
pub async fn chat(st: &AppState, session: Uuid, req: ChatRequest) -> AppResult<ChatTurn> {
    let message = req.message.trim();
    if message.is_empty() {
        return Err(AppError::BadRequest("message is empty".into()));
    }
    if message.chars().count() > 2000 {
        return Err(AppError::BadRequest("message is too long (2000 characters max)".into()));
    }
    let llm = st
        .llm
        .as_deref()
        .ok_or_else(|| AppError::Unavailable("The AI agent is not configured on this server yet.".into()))?;
    let target = match req.venue_slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(slug) => {
            let venue = venue_by_slug(&st.db, slug).await?;
            let spaces = spaces_of(&st.db, venue.id).await?;
            Target::Venue(venue, spaces)
        }
        None => Target::Scout(scout_artist(&st.db, req.conversation_id, req.artist.as_deref()).await?),
    };
    let (scope, venue_id, artist_id) = match &target {
        Target::Venue(v, _) => (Scope::Venue, Some(v.id), None),
        Target::Scout(a) => (Scope::Scout, None, Some(a.id)),
    };

    let conversation_id = match req.conversation_id {
        Some(id) => {
            let mine: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM conversations
                                 WHERE id = $1 AND demo_session_id = $2 AND scope = $3 AND venue_id IS NOT DISTINCT FROM $4)",
            )
            .bind(id)
            .bind(session)
            .bind(scope.as_str())
            .bind(venue_id)
            .fetch_one(&st.db)
            .await?;
            if !mine {
                return Err(AppError::NotFound);
            }
            id
        }
        None => {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO conversations (id, demo_session_id, venue_id, scope, artist_id, model) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(session)
            .bind(venue_id)
            .bind(scope.as_str())
            .bind(artist_id)
            .bind(&llm.model)
            .execute(&st.db)
            .await?;
            id
        }
    };

    if let Some(reply) = limit_reply(st, session).await? {
        return Ok(ChatTurn {
            conversation_id,
            reply,
            tool_calls: vec![],
            proposals: open_proposals(&st.db, session, conversation_id).await?,
            booking: current_booking(&st.db, session, conversation_id).await?,
            limited: true,
            venues: vec![],
        });
    }

    let stored: Vec<(String, String)> =
        sqlx::query_as("SELECT role, content FROM agent_messages WHERE conversation_id = $1 ORDER BY seq")
            .bind(conversation_id)
            .fetch_all(&st.db)
            .await?;
    let first_seq = stored.len() as i32;
    let history = stored.into_iter().map(|(r, c)| Ok((r, raw(c)?))).collect::<AppResult<Vec<(String, Box<RawValue>)>>>()?;

    let system_text = match &target {
        Target::Venue(venue, spaces) => system_prompt(venue, spaces),
        Target::Scout(_) => SCOUT_PROMPT.to_string(),
    };
    let system = raw(json!([{ "type": "text", "text": system_text }]).to_string())?;
    let tool_defs = raw(tools::definitions(scope).to_string())?;
    let context = match &target {
        Target::Venue(venue, _) => context_block(&st.db, session, venue, conversation_id).await?,
        Target::Scout(artist) => scout_context(&st.db, session, artist, conversation_id).await?,
    };
    let mut new_messages: Vec<(String, Box<RawValue>)> = vec![(
        "user".into(),
        raw(json!([{ "type": "text", "text": context }, { "type": "text", "text": message }]).to_string())?,
    )];

    let mut log = RunLog::start(&st.db, session, conversation_id).await?;
    log.add("user", None, Some(json!({ "text": message })), None, None, None).await;

    let (venue_ref, spaces_ref, artist_ref): (Option<&Venue>, &[Space], Option<&ScoutArtist>) = match &target {
        Target::Venue(v, s) => (Some(v), s.as_slice(), None),
        Target::Scout(a) => (None, &[], Some(a)),
    };
    let mut ctx = ToolContext {
        st,
        session,
        conversation_id,
        venue: venue_ref,
        spaces: spaces_ref,
        artist: artist_ref,
        proposals: vec![],
        venues_found: vec![],
    };
    let mut calls: Vec<ToolCallView> = Vec::new();
    let mut usage = Usage::default();
    let mut reply: Option<String> = None;
    let mut keep = true;

    for _ in 0..MAX_ROUNDS {
        let messages: Vec<Message> = history
            .iter()
            .chain(new_messages.iter())
            .map(|(role, content)| Message { role: role.as_str(), content: content.as_ref() })
            .collect();
        let started = Instant::now();
        let resp = match llm.create(&system, &tool_defs, &messages).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "Claude API call failed");
                log.add("error", None, None, Some(json!({ "error": "The AI service did not answer." })), Some(started.elapsed().as_millis() as i32), None).await;
                record_usage(&st.db, usage).await;
                return Err(AppError::Upstream("The AI agent could not reach the AI service. Please try again in a moment.".into()));
            }
        };
        let latency = started.elapsed().as_millis() as i32;
        usage += resp.usage;

        if resp.stop_reason == "refusal" {
            log.add("blocked", None, None, Some(json!({ "refusal": resp.refusal_category })), Some(latency), Some(resp.usage)).await;
            reply = Some(match scope {
                Scope::Venue => "Sorry, I can't help with that. I can check dates, propose slots and send booking requests for this venue.",
                Scope::Scout => "Sorry, I can't help with that. I can find venues, check their dates and send booking proposals for you.",
            }
            .into());
            keep = false;
            break;
        }
        if resp.stop_reason == "max_tokens" || resp.content.is_empty() {
            log.add("error", None, None, Some(json!({ "error": format!("stopped: {}", resp.stop_reason) })), Some(latency), Some(resp.usage)).await;
            record_usage(&st.db, usage).await;
            return Err(AppError::Upstream("The AI agent's answer was cut off. Please try again.".into()));
        }

        let text = resp
            .content
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n\n");
        let tool_uses: Vec<(String, String, Value)> = resp
            .content
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
            .map(|b| {
                (
                    b.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    b.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    b.get("input").cloned().unwrap_or(Value::Null),
                )
            })
            .collect();
        new_messages.push(("assistant".into(), raw(Value::Array(resp.content).to_string())?));

        let mut step_usage = Some(resp.usage);
        let mut step_latency = Some(latency);
        if !text.trim().is_empty() || tool_uses.is_empty() {
            log.add("assistant", None, None, Some(json!({ "text": text, "model": resp.model })), step_latency.take(), step_usage.take()).await;
        }
        if resp.stop_reason != "tool_use" || tool_uses.is_empty() {
            reply = Some(text);
            break;
        }

        let mut results = Vec::with_capacity(tool_uses.len());
        for (id, name, input) in tool_uses {
            log.add("tool_call", Some(&name), Some(input.clone()), None, step_latency.take(), step_usage.take()).await;
            let t = Instant::now();
            let out = tools::run(&mut ctx, &name, &input).await;
            let shown: Value = serde_json::from_str(&out.content).unwrap_or_else(|_| Value::String(out.content.clone()));
            let kind = if out.status == "blocked" { "blocked" } else { "tool_result" };
            log.add(kind, Some(&name), None, Some(shown), Some(t.elapsed().as_millis() as i32), None).await;
            calls.push(ToolCallView { name: name.clone(), status: out.status, summary: out.summary.clone() });
            let mut block = json!({ "type": "tool_result", "tool_use_id": id, "content": out.content });
            if out.is_error {
                block["is_error"] = json!(true);
            }
            results.push(block);
        }
        new_messages.push(("user".into(), raw(Value::Array(results).to_string())?));
    }

    let reply = match reply {
        Some(r) if !r.trim().is_empty() => r,
        Some(_) => "Done — see the details below.".to_string(),
        None => {
            keep = false;
            "I couldn't finish that in one go. Could you ask again a bit more specifically?".to_string()
        }
    };

    if keep {
        let mut tx = st.db.begin().await?;
        for (i, (role, content)) in new_messages.iter().enumerate() {
            sqlx::query("INSERT INTO agent_messages (conversation_id, seq, role, content) VALUES ($1, $2, $3, $4)")
                .bind(conversation_id)
                .bind(first_seq + i as i32)
                .bind(role)
                .bind(content.get())
                .execute(&mut *tx)
                .await
                .map_err(|e| bookings::conflict_on_constraint(e, "Another message in this conversation is still being answered."))?;
        }
        sqlx::query("UPDATE conversations SET turns = turns + 1, last_turn_at = now() WHERE id = $1")
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    record_usage(&st.db, usage).await;

    // Refresh proposals (one may have been requested in the same turn) and drop withdrawn ones.
    let mut proposals = Vec::new();
    for p in &ctx.proposals {
        if let Ok(s) = bookings::summary(&st.db, p.id).await {
            if s.status != "cancelled" {
                proposals.push(s);
            }
        }
    }
    let venues = std::mem::take(&mut ctx.venues_found);
    Ok(ChatTurn {
        conversation_id,
        reply,
        tool_calls: calls,
        proposals,
        booking: current_booking(&st.db, session, conversation_id).await?,
        limited: false,
        venues,
    })
}

#[derive(Debug, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub text: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ConversationView {
    pub conversation_id: Uuid,
    pub messages: Vec<ChatMessage>,
    pub proposals: Vec<BookingSummary>,
    pub booking: Option<Booking>,
}

/// The visible conversation (for reloading the chat panel).
pub async fn conversation(st: &AppState, session: Uuid, id: Uuid) -> AppResult<ConversationView> {
    let mine: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM conversations WHERE id = $1 AND demo_session_id = $2)")
        .bind(id)
        .bind(session)
        .fetch_one(&st.db)
        .await?;
    if !mine {
        return Err(AppError::NotFound);
    }
    let rows: Vec<(String, Option<Value>, Option<Value>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT kind, input, output, created_at FROM agent_runs
          WHERE conversation_id = $1 AND kind IN ('user','assistant') ORDER BY step",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    let messages = rows
        .into_iter()
        .filter_map(|(kind, input, output, at)| {
            let text = if kind == "user" { input } else { output }
                .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_string))?;
            (!text.trim().is_empty()).then(|| ChatMessage { role: kind, text, at })
        })
        .collect();
    Ok(ConversationView {
        conversation_id: id,
        messages,
        proposals: open_proposals(&st.db, session, id).await?,
        booking: current_booking(&st.db, session, id).await?,
    })
}
