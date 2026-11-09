# MiSFY DropCard

A link card for live-music venues where an **AI agent handles the booking** and **PayPal handles the money**:
the deposit when the venue approves, ticket sales, and the payout to the venue, the artist and the promoter
on the day after the show.

> **Status (2026-11-09): work in progress for the PayPal AI Hackathon.**
> Done: database schema, sample data, server skeleton (venue card, schedule, demo sessions, demo clock),
> settlement rule with tests. In progress: PayPal Orders v2 + webhooks, Payouts, the AI booking agent,
> the scheduler and dashboard screens. This README is updated as features land.

## The flow

1. **AI booking concierge** — a promoter asks in plain English ("a Friday night in December for about 300 people,
   synthwave"). The agent checks availability across the venue's spaces, proposes open slots and drafts the inquiry.
2. **Venue schedule** — spaces are rows, bookings are bars: confirmed, paid, on hold, and AI-proposed slots.
3. **PayPal deposit** — when the venue owner approves, the promoter pays the deposit with PayPal.
   A booking is confirmed **only by a verified PayPal webhook**, never by the browser.
4. **PayPal payout** — at 09:00 venue time on the day after the show, one Payouts batch pays the venue,
   the artist and the promoter their shares. Fees are deducted at settlement.
5. **Owner dashboard** — sales, buyer countries and settlements, built with AG Grid.

## PayPal APIs used

_Being implemented — this section will list each API and where it is used._

- Orders v2 (create, capture) with `PayPal-Request-Id` idempotency keys
- Webhooks with signature verification (`/v1/notifications/verify-webhook-signature`)
- Payouts (batch payouts, item status, retries)
- PayPal JavaScript SDK (checkout buttons)

## AI used

_Being implemented._ An LLM agent with tool calling (Anthropic Claude). The agent's tools read availability,
place AI-proposed holds, draft inquiries and — only after the venue has approved — create the PayPal deposit order.
Every tool call is recorded and shown in the venue console.

## Money rules (the same in code and in sample data)

See [`src/settlement.rs`](src/settlement.rs):

- Money for a show = captured PayPal payments − PayPal fees − refunds, tracked separately for the deposit and for tickets.
- Platform fee (example: 5%) is deducted at settlement.
- Deposit → venue. Ticket sales → artist 50%, promoter 20%, venue the rest (example split; remainder cents go to the venue).
- Every money movement is a balanced double-entry transaction in one ledger (`ledger_entries`).

## Demo sessions and the demo clock

Every visitor gets their own demo session (a cookie), so several judges can try the demo at the same time without
seeing each other's bookings. Settlement waits for "the day after the show", so each session can move **its own**
demo clock forward. The clock only changes what "now" means for that session's bookings; every PayPal call is real
(sandbox).

All venues, artists, promoters and transactions marked **Sample** are fictional and never sent to PayPal.

## Run locally

Requirements: Rust stable (developed with 1.95), PostgreSQL 14+ (with the `btree_gist` extension, included in standard builds).

```sh
cp .env.example .env            # set DATABASE_URL
cargo run -- seed               # runs migrations, creates sample data around today
cargo run                       # http://localhost:8080
```

## License

[MIT](LICENSE)
