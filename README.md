# MiSFY DropCard

**An AI venue scout finds the venue and sends the proposal; PayPal handles the money** — the deposit when the venue
approves, ticket sales, and the payout to the venue, the artist and the promoter on the day after the show.

Built for the PayPal AI Hackathon 2026. A new, standalone service made for MiSFY's venue cards: it shares no code,
keys or infrastructure with the existing MiSFY app, and it only ever talks to the **PayPal sandbox**.

> **Live demo: https://misfy-dropcard.onrender.com** (PayPal sandbox only; the first page is the AI venue scout).
>
> **Status (2026-11-12).** Server, database, PayPal integration (Orders v2, webhooks, Payouts), the AI venue scout,
> settlement and the web frontend are implemented and tested — locally and on the hosted demo against the real PayPal
> sandbox and the real Claude API, and end to end against local stand-ins (see [Testing](#testing)). The demo runs on
> Render (web service, Postgres and a nightly cron job, see `render.yaml`).
>
> **Judges:** the sandbox buyer login for the live demo is in the Devpost testing instructions.

## What it does

| Step | What happens | Built with |
|---|---|---|
| 1. Ask | An artist tells the AI venue scout where and what they want to play: *"The busiest venues in Tokyo for a synthwave show, about 200 people."* | Claude (Anthropic API) with tools |
| 2. Find | The scout searches the venue directory, ranks venues by **activity** (how many shows MiSFY's discovery data has on record for them), checks dates at the best fit and proposes up to three slots. | |
| 3. Propose | When the artist says so, the scout writes the proposal letter and sends it; the slot becomes a hold. The venue owner approves it in the owner console. | Bryntum Scheduler Pro |
| 4. Pay | The artist pays the 30% deposit with PayPal on the venue's card. The booking turns green **only when the verified PayPal webhook arrives**. Tickets go on sale. | PayPal Orders v2, JS SDK, Webhooks |
| 5. Settle | At 09:00 venue time on the day after the show, one PayPal Payouts batch pays the venue, the artist and the promoter their shares. | PayPal Payouts |
| 6. See | The owner dashboard shows sales, buyer countries, fees and payouts, all computed from one ledger. | AG Grid, AG Charts |

Promoters who already know the venue can also book on its card with a booking form (dates, nights, crowd size); the
rest of the flow is the same.

## PayPal APIs used

| PayPal API | Where | What it does here |
|---|---|---|
| **Orders v2** — create, capture, get (`/v2/checkout/orders`) | [`src/paypal.rs`](src/paypal.rs), [`src/payments.rs`](src/payments.rs) | Deposit orders (30% of the quote) and ticket orders. Every create and capture sends a `PayPal-Request-Id`, so a retried or double-clicked request returns the original order instead of a second one. A booking can have only one live deposit order (enforced by a unique index). |
| **Webhooks** — signature verification (`/v1/notifications/verify-webhook-signature`) | [`src/webhooks.rs`](src/webhooks.rs) | `PAYMENT.CAPTURE.COMPLETED` confirms a payment; payout item events confirm payouts. The raw event is forwarded to PayPal **byte for byte** for verification. Events are de-duplicated by event id; unverified events are logged but never applied. |
| **Payouts** — create, get (`/v1/payments/payouts`) | [`src/payouts.rs`](src/payouts.rs) | One batch per show pays venue, artist and promoter. `sender_batch_id` is unique per show and doubles as the request id, so a show can never be paid twice. Failed items are retried in a new batch (max 3 attempts); item status comes from webhooks and from polling. |
| **JavaScript SDK** — Buttons | `web/` | The PayPal checkout window for the deposit and tickets. The browser's approval only asks the server to capture; it never confirms a payment. |
| **Reconciliation** (Orders v2 get) | [`src/payments.rs`](src/payments.rs) `reconcile` | If a webhook is late or missing, a background job asks PayPal directly and records the capture (marked "confirmed by reconciliation"). Orders the buyer approved but never captured are captured server-side; abandoned orders expire after 30 minutes. |

## AI used

**Claude Opus 5.5** (`claude-opus-5-5`, Anthropic Messages API) runs the **AI venue scout** — an agent that works for
one artist and has six tools, implemented in [`src/agent/`](src/agent/):

| Tool | What it does |
|---|---|
| `search_venues` | Searches the venue directory by place (city, country or region), genre category and room size, ranked by activity. |
| `check_availability` | Finds free slots at one venue by date range, weekdays, headcount, start time and length. |
| `propose_slots` | Shows up to three slots at that venue as cards (dashed bars on its schedule, no reservation). |
| `request_booking` | Sends the proposal for the slot the artist chose — with a proposal letter the agent writes — and turns it into a hold. |
| `get_booking_status` | Reads the booking, deposit and payout state. |
| `create_deposit_order` | Prepares the PayPal deposit order — **refused by the server until the venue owner has approved the booking**. |

The same loop also runs a booking concierge for a single venue (`POST /api/agent/chat` with `venue_slug`, see
[`docs/API.md`](docs/API.md)).

Guardrails are in the server, not in the prompt: every tool runs inside the visitor's own demo session; the agent
cannot approve bookings; payment tools are blocked before approval (the refusal is recorded as `blocked`); the agent
never sees card or account details (payment happens in PayPal's window). Every message and tool call — inputs,
outputs, latency, tokens — is stored in `agent_runs` and shown in the venue console. A daily cap
(`LLM_DAILY_TURN_CAP`) keeps the public demo's API spend bounded; venue cards keep working with the booking form.

API details: `anthropic-version: 2026-10-21`, adaptive thinking with an explicit `effort`, server-side refusal
fallback (`fallbacks: "default"`), prompt caching of the stable prefix (system prompt + tools + earlier turns), and an
append-only conversation history replayed byte for byte so thinking blocks stay valid across turns.

## The venue directory

The scout searches [`data/venue_directory.csv`](data/venue_directory.csv), built by
[`scripts/venue_directory.py`](scripts/venue_directory.py) and loaded by `misfy-dropcard seed`:

- **Activity** (0–100) is the percentile of a venue's *shows on record* across the directory. It says how busy a
  venue's calendar has been, not how popular it is right now, and the scout says so.
- Genres are broad categories (Electronic / DJ, Rock / Indie, Vocal / Soul, Pop / Hip-hop, Jazz, Techno / House); the
  scout maps an artist's genre to the closest one.
- The bundled file holds **300 venues in 20 cities from MiSFY's discovery data, pseudonymized**: city, country, genre
  category and shows on record come from the data (names are pseudonyms such as "Demo Venue 151", show counts are
  rounded to tens and capped at 50); rooms, capacity and prices are made up for the demo, because the data has none.
  The three hand-made sample venue cards (Blue Lantern Hall, Harbor Room, Grey Matter) are fictional.
- `python3 scripts/venue_directory.py import EXPORT.csv` rebuilds it from a pseudonymized export (exports with real
  venue names are refused; other identifying columns are dropped). `python3 scripts/venue_directory.py sample` builds a
  fully fictional directory instead, and `VENUE_DIRECTORY_CSV=path` loads a file that stays out of the repository.

## How the money stays correct

- **Confirmation only from PayPal.** A deposit counts when a verified webhook (or reconciliation with the PayPal API)
  says the capture completed — never because the browser said so.
- **Idempotency everywhere.** `PayPal-Request-Id` on every create/capture/payout; `Idempotency-Key` from the browser
  for orders; unique keys in the database for orders, captures, webhook events, payout batches and items.
- **Double-entry ledger.** Payments, PayPal fees, the platform fee, refunds and payouts are written to
  `ledger_entries`; every transaction sums to zero, so a show's cash always reconciles to zero after payout.
- **One settlement rule** ([`src/settlement.rs`](src/settlement.rs), the same arithmetic in the sample-data SQL):
  platform fee (example 5%) taken at settlement; deposit → venue; ticket revenue after fees → artist 50%,
  promoter 20%, venue the rest (remainder cents go to the venue). Unit tests check that every cent is accounted for.
- **No double booking.** A database exclusion constraint plus per-space advisory locks.

## Demo sessions and the demo clock

Every visitor gets an isolated demo session (cookie): their own bookings, conversations and clock, so several judges
can use the demo at the same time. Payouts wait for "the day after the show", so each session can move **its own**
clock forward ("+1 day", "+1 week"). The clock only changes what "now" means for that session's bookings; every PayPal
call is real (sandbox). Venues, artists and transactions marked **Sample** are fictional and never sent to PayPal;
directory data marked **Discovery data** is pseudonymized and rounded.

## Architecture

```
Browser (React + TypeScript)                Rust server (axum, sqlx)                      External
  AI venue scout · venue cards  ──/api──▶   agent loop · venue directory          ──▶   Anthropic Messages API
  Bryntum Scheduler Pro                      bookings · payments · webhooks        ──▶   PayPal sandbox REST
  AG Grid / AG Charts dashboard              settlement jobs (15 s)                ◀──   PayPal webhooks
  PayPal JS SDK buttons                                 │
                                                        ▼
                                               PostgreSQL (ledger, orders, payouts, agent_runs, venues)

Render: web service (Docker) + nightly cron job (sample data refresh) + Render Postgres — see render.yaml
```

## Run locally

Requirements: Rust stable (developed with 1.95), PostgreSQL 14+ (standard builds include the `btree_gist`
extension), Node 22+ for the frontend.

```sh
cp .env.example .env          # set DATABASE_URL; add PayPal sandbox and Anthropic keys to use those features
cargo run -- seed             # runs migrations, loads the venue directory and builds sample data around today
(cd web && npm install && npm run build)
cargo run                     # http://localhost:8080 opens the AI venue scout
```

Without an Anthropic key the scout page says so and every venue card still books with the booking form; without
PayPal keys the payment endpoints answer `503` with a clear message.

**Bryntum Scheduler Pro:** `web/package.json` installs Bryntum's public trial package from npm through the vendor's
alias — `"@bryntum/schedulerpro": "npm:@bryntum/schedulerpro-trial@7.3.8"` — so imports use `@bryntum/schedulerpro` and a
licensed build only changes that one line. It is under Bryntum's commercial trial license, shows a faint watermark and
is time-limited per browser. Do not use `npm install --ignore-scripts` (its install script sets up the trial).
No Bryntum files are committed to this repository; the build bundles the package at deploy time.

### PayPal sandbox setup

1. developer.paypal.com → Apps & Credentials → Sandbox → create an app; put its client id and secret in `.env`.
2. Register the webhook (PayPal needs a public https URL — your deployment, or a tunnel to localhost):
   `cargo run -- paypal-webhook https://<your-host>` creates it (or finds the existing one) and prints
   `PAYPAL_WEBHOOK_ID=...` for `.env`. Without a webhook id the server still confirms payments through
   reconciliation with the PayPal API (within ~15 s instead of instantly).
3. Optional: set `SANDBOX_VENUE_EMAIL`, `SANDBOX_ARTIST_EMAIL`, `SANDBOX_PROMOTER_EMAIL` to sandbox personal accounts
   so payouts land in real sandbox wallets, then run `cargo run -- seed` again.
4. To pay in the PayPal checkout window, log in with one of your own sandbox **personal** accounts
   (developer.paypal.com → Testing Tools → Sandbox Accounts).

## Testing

```sh
cargo test          # settlement arithmetic, money parsing, webhook byte-for-byte forwarding, directory, tools, ...
tests/e2e/run.sh    # the whole flow against local stand-ins for the PayPal sandbox and the Claude API
```

`tests/e2e/run.sh` starts the server with [`mock_paypal.py`](tests/e2e/mock_paypal.py) and
[`mock_claude.py`](tests/e2e/mock_claude.py) (no keys, nothing leaves the machine) and runs
[`scenario.py`](tests/e2e/scenario.py), which checks 20 things, among them: the scout searches venues, checks dates at
the top one and proposes slots; venues come back ranked by activity; the scout sends the proposal (hold); the server
blocks the payment tool before approval; a double click creates one PayPal order; capture alone does not confirm a
booking — the verified webhook does; a redelivered webhook is ignored and a forged one rejected; one payout batch pays
venue, artist and promoter, and settling again never pays twice; payouts + fees equal gross to the cent; every request
to the Claude API keeps an identical, cacheable prefix per agent and an append-only history.
The stand-ins are for tests only — the hosted demo talks to the real PayPal sandbox and the real Claude API.

## API

The HTTP API is documented in [`docs/API.md`](docs/API.md).

## Project layout

```
src/
  main.rs          commands: serve | seed | cycle | paypal-webhook
  paypal.rs        PayPal REST client (OAuth, Orders v2, webhook verification, Payouts)
  payments.rs      deposit/ticket orders, capture, reconciliation
  webhooks.rs      PayPal webhook endpoint
  payouts.rs       settlement planning, payout batches, retries
  settlement.rs    the settlement rule (pure, unit-tested)
  ledger.rs        double-entry ledger
  bookings.rs      availability, proposals, holds, approval
  directory.rs     the venue directory the scout searches
  agent/           Claude agents: API client, tools, conversation loop
  routes/          HTTP handlers
data/              venue directory (pseudonymized discovery data)
scripts/           venue directory builder
migrations/        schema and sample-data generator
web/               frontend (React, Bryntum Scheduler Pro, AG Grid, PayPal JS SDK)
```

## License

[MIT](LICENSE)
