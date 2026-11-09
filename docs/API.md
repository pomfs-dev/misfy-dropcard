# HTTP API (contract)

The Rust server serves this API under `/api` and the built frontend (`web/dist`) for every other path.
This file is the contract between the server and the frontend. Status: ✅ live, 🛠 being built (shape is final).

Conventions

- JSON everywhere. Money is an **integer number of cents** (`*_cents`), currency is USD.
- Times are RFC 3339 strings in UTC (`2026-11-20T10:00:00Z`). Show them in **venue time**
  (`tz_offset_minutes`, Seoul = +540) in the UI.
- Errors: non-2xx with `{"error": "human readable message"}`.
- Every `/api` request carries the `dc_sid` cookie (set automatically on the first call). It identifies the
  visitor's **demo session**: their own bookings, conversations and demo clock. Use `credentials: "same-origin"`.
- "Now" for a session = real time + the session's demo clock offset. Always use `now` from the server.

## Session and demo controls

| | Endpoint | Body | Response |
|---|---|---|---|
| ✅ | `GET /api/session` | | `Session` |
| ✅ | `POST /api/demo/clock/advance` | `{"hours": 1..720}` | `Session` (409 past +90 days) |
| ✅ | `POST /api/demo/clock/reset` | | `Session` |
| ✅ | `POST /api/demo/reset` | | `Session` — deletes this visitor's bookings and conversations |
| 🛠 | `GET /api/config` | | `{"paypal_client_id": string\|null, "paypal_env": "sandbox", "currency": "USD", "llm_enabled": bool}` |

```ts
type Session = { session_id: string; now: string; clock_offset_seconds: number };
```

## Venues

| | Endpoint | Response |
|---|---|---|
| ✅ | `GET /api/venues` | `Venue[]` |
| ✅ | `GET /api/venues/{slug}` | `Card` |
| ✅ | `GET /api/venues/{slug}/schedule?from=&to=` | `Schedule` (default window: now−14d … now+75d, max 190 days) |

```ts
type Venue = { id: string; slug: string; name: string; city: string; country: string; tagline: string;
  description: string; tags: string[]; capacity_band: string; tz_offset_minutes: number };
type Space = { id: string; name: string; capacity_min: number; capacity_max: number;
  day_rate_cents: number; sort_order: number };
type Card = { venue: Venue; spaces: Space[];
  upcoming: { id: string; title: string; space: string; starts_at: string; ticket_price_cents: number | null }[] };
type BookingStatus = "ai_proposed" | "hold" | "approved" | "paid" | "completed" | "cancelled";
type ScheduleEvent = { id: string; space_id: string; title: string; status: BookingStatus;
  starts_at: string; ends_at: string;
  mine: boolean;     // created in this visitor's session
  sample: boolean }; // shared fictional sample data
type Schedule = { now: string; from: string; to: string; tz_offset_minutes: number;
  spaces: Space[]; events: ScheduleEvent[] };
```

Other people's holds come back with the title `"On hold"`. AI proposals (`ai_proposed`) do not reserve the slot;
draw them as dashed outlines. Suggested colors: completed = grey, paid = green `#2AD829`, approved = blue `#1E88E5`,
hold = yellow `#EAB308` striped, ai_proposed = dashed outline.

## AI booking concierge

| | Endpoint | Body | Response |
|---|---|---|---|
| 🛠 | `POST /api/agent/chat` | `{"venue_slug": string, "conversation_id"?: string, "message": string}` | `ChatTurn` |
| 🛠 | `GET /api/agent/conversations/{id}` | | `{"conversation_id": string, "messages": ChatMessage[], "booking": Booking \| null}` |

```ts
type ChatMessage = { role: "user" | "assistant"; text: string; at: string };
type ToolCall = { name: string; status: "ok" | "blocked" | "error"; summary: string };
type ChatTurn = {
  conversation_id: string;
  reply: string;              // assistant text (plain text, may contain line breaks and "- " bullets)
  tool_calls: ToolCall[];     // what the agent did this turn, in order (show as small chips)
  proposals: BookingSummary[];// slots the agent proposed this turn (dashed bars on the schedule)
  booking: Booking | null;    // the booking this conversation is about, once the promoter picked a slot
  limited: boolean;           // true if the daily safety cap was reached (reply explains it)
};
```

The agent's tools: `check_availability`, `propose_slots`, `request_booking` (turns a proposal into a hold with an
inquiry), `get_booking_status`, `create_deposit_order` (**only allowed after the venue approved**; otherwise the
call is recorded as `blocked`).

## Bookings

| | Endpoint | Body | Response |
|---|---|---|---|
| 🛠 | `GET /api/bookings/{id}` | | `Booking` (poll this while waiting for the webhook) |
| 🛠 | `POST /api/bookings/{id}/request` | `{"inquiry_text"?: string}` | `Booking` — proposal → hold (409 if the slot was taken) |
| 🛠 | `POST /api/bookings/{id}/cancel` | | `Booking` |

```ts
type BookingSummary = { id: string; title: string; status: BookingStatus; space_id: string; space_name: string;
  starts_at: string; ends_at: string; quote_cents: number; deposit_cents: number;
  headcount: number | null; genre: string | null; created_by: "sample" | "agent" | "owner" | "promoter" };
type Booking = BookingSummary & {
  venue_slug: string;
  artist: { slug: string; name: string } | null;
  ticket_price_cents: number | null;
  inquiry_text: string | null;
  approved_at: string | null;
  deposit_order: { paypal_order_id: string | null; status: string; amount_cents: number;
                   captured_at: string | null } | null;
  settlement: Settlement | null;
  timeline: { at: string; label: string }[];   // e.g. "AI proposed", "Requested", "Approved by venue",
                                               // "Deposit paid (PayPal webhook verified)", "Show ended", "Paid out"
};
```

## Payments (PayPal Orders v2)

| | Endpoint | Body / headers | Response |
|---|---|---|---|
| 🛠 | `POST /api/bookings/{id}/deposit-order` | header `Idempotency-Key: <uuid>` | `{"paypal_order_id": string, "amount_cents": number, "currency": "USD"}` (409 unless status is `approved`) |
| 🛠 | `POST /api/paypal/orders/{paypal_order_id}/capture` | | `{"status": "pending_webhook" \| "captured" \| "failed", "booking": Booking}` |
| 🛠 | `POST /api/paypal/webhook` | (PayPal → server) | `200` |

Frontend flow with the PayPal JS SDK (`@paypal/paypal-js`, client id from `/api/config`):

```ts
paypal.Buttons({
  createOrder: async () => (await post(`/api/bookings/${id}/deposit-order`, {}, { "Idempotency-Key": key })).paypal_order_id,
  onApprove: async ({ orderID }) => {
    await post(`/api/paypal/orders/${orderID}/capture`);
    // The booking becomes "paid" only when the verified webhook arrives: poll GET /api/bookings/{id}
    // and show "Waiting for PayPal confirmation…" until status === "paid".
  },
});
```

Generate one `Idempotency-Key` per booking per page load and reuse it, so a double click never creates two orders.

## Venue owner console

The same visitor can switch to the venue owner role. Owner actions apply to bookings of the visitor's own session.

| | Endpoint | Body | Response |
|---|---|---|---|
| 🛠 | `GET /api/console/{slug}/inbox` | | `Booking[]` — holds waiting for approval |
| 🛠 | `POST /api/console/bookings/{id}/approve` | | `Booking` |
| 🛠 | `POST /api/console/bookings/{id}/decline` | | `Booking` |
| 🛠 | `PATCH /api/console/bookings/{id}` | `{"space_id"?, "starts_at"?, "ends_at"?}` | `Booking` — drag/resize on the scheduler (proposals and holds only; 409 on overlap) |
| 🛠 | `GET /api/console/{slug}/payments` | | `Payments` |
| 🛠 | `GET /api/console/{slug}/payouts` | | `{"batches": Settlement[]}` |
| 🛠 | `POST /api/console/{slug}/settlement/run` | | `{"batches": Settlement[]}` — run due settlements now (uses the session clock) |
| 🛠 | `GET /api/console/{slug}/agent-runs` | | `{"runs": AgentRun[]}` |
| 🛠 | `GET /api/console/{slug}/dashboard` | | `Dashboard` |

```ts
type Settlement = { id: string; booking_id: string; booking_title: string; sender_batch_id: string;
  paypal_batch_id: string | null; status: "planned" | "submitted" | "succeeded" | "partial" | "failed";
  scheduled_for: string; net_cents: number; platform_fee_cents: number; sample: boolean;
  items: { role: "venue" | "artist" | "promoter"; recipient_email: string; amount_cents: number;
           status: "planned" | "pending" | "succeeded" | "failed" | "unclaimed" | "returned";
           paypal_item_id: string | null; attempts: number; error: string | null }[] };
type Payments = {
  orders: { id: string; booking_id: string; booking_title: string; kind: "deposit" | "ticket";
    source: "paypal_sandbox" | "sample"; paypal_order_id: string | null; request_id: string;
    quantity: number; amount_cents: number; paypal_fee_cents: number | null; status: string;
    payer_country: string | null; created_at: string; captured_at: string | null }[];
  webhooks: { id: number; event_type: string; resource_id: string | null; signature_valid: boolean;
    received_at: string; outcome: string | null; error: string | null }[];
};
type AgentRun = { conversation_id: string; step: number; kind: "user" | "assistant" | "tool_call" | "tool_result" |
  "blocked" | "error"; tool_name: string | null; input: unknown; output: unknown; latency_ms: number | null;
  created_at: string };
type Dashboard = {
  shows: { booking_id: string; date: string; title: string; space: string; status: BookingStatus; sample: boolean;
    tickets_sold: number; gross_cents: number; paypal_fees_cents: number; refunds_cents: number;
    platform_fee_cents: number; venue_cents: number; artist_cents: number; promoter_cents: number;
    payout_status: string | null }[];
  by_country: { country: string; tickets: number; gross_cents: number }[];
  totals: { gross_cents: number; paypal_fees_cents: number; platform_fee_cents: number; paid_out_cents: number;
    international_share: number };  // 0..1, share of ticket revenue from buyers outside the venue's country
};
```
