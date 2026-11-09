-- MiSFY DropCard — initial schema (stand-alone service, PayPal sandbox only)
-- All money is stored as integer minor units (cents). Single currency (USD) to start.

CREATE EXTENSION IF NOT EXISTS btree_gist;

-- Each visitor (judge) gets an isolated demo session: their own bookings and their own demo clock.
CREATE TABLE demo_sessions (
    id                   UUID PRIMARY KEY,
    clock_offset_seconds BIGINT NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE venues (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug               TEXT NOT NULL UNIQUE,
    name               TEXT NOT NULL,
    city               TEXT NOT NULL,
    country            TEXT NOT NULL,
    tagline            TEXT NOT NULL,
    description        TEXT NOT NULL,
    tags               TEXT[] NOT NULL DEFAULT '{}',
    capacity_band      TEXT NOT NULL,              -- capacity is shown as a range only
    tz_offset_minutes  INT NOT NULL DEFAULT 540,   -- venue local time (Asia/Seoul = +09:00, no DST)
    payout_email       TEXT NOT NULL,              -- PayPal sandbox receiver
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE spaces (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    venue_id        UUID NOT NULL REFERENCES venues(id),
    name            TEXT NOT NULL,
    capacity_min    INT NOT NULL,
    capacity_max    INT NOT NULL,
    day_rate_cents  BIGINT NOT NULL CHECK (day_rate_cents > 0),
    sort_order      INT NOT NULL DEFAULT 0,
    CHECK (capacity_max >= capacity_min),
    UNIQUE (venue_id, name)
);

CREATE TABLE artists (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug          TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    genre         TEXT NOT NULL,
    country       TEXT NOT NULL,
    bio           TEXT NOT NULL,
    fee_cents     BIGINT NOT NULL CHECK (fee_cents >= 0),
    payout_email  TEXT NOT NULL
);

CREATE TABLE promoters (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug          TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    email         TEXT NOT NULL,
    payout_email  TEXT NOT NULL
);

-- A booking of one space for one show.
-- Lifecycle: ai_proposed -> hold -> approved -> paid -> completed (or cancelled at any point).
CREATE TABLE bookings (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    demo_session_id     UUID REFERENCES demo_sessions(id) ON DELETE CASCADE,  -- NULL = shared sample data
    session_key         UUID GENERATED ALWAYS AS
                        (COALESCE(demo_session_id, '00000000-0000-0000-0000-000000000000'::uuid)) STORED,
    venue_id            UUID NOT NULL REFERENCES venues(id),
    space_id            UUID NOT NULL REFERENCES spaces(id),
    promoter_id         UUID NOT NULL REFERENCES promoters(id),
    artist_id           UUID REFERENCES artists(id),
    title               TEXT NOT NULL,
    genre               TEXT,
    headcount           INT,
    starts_at           TIMESTAMPTZ NOT NULL,
    ends_at             TIMESTAMPTZ NOT NULL,
    status              TEXT NOT NULL DEFAULT 'ai_proposed'
                        CHECK (status IN ('ai_proposed','hold','approved','paid','completed','cancelled')),
    quote_cents         BIGINT NOT NULL CHECK (quote_cents >= 0),
    deposit_cents       BIGINT NOT NULL CHECK (deposit_cents >= 0),
    ticket_price_cents  BIGINT CHECK (ticket_price_cents > 0),
    -- settlement rule (example split, basis points): ticket revenue after fees is shared by these three
    split_venue_bps     INT NOT NULL DEFAULT 3000,
    split_artist_bps    INT NOT NULL DEFAULT 5000,
    split_promoter_bps  INT NOT NULL DEFAULT 2000,
    platform_fee_bps    INT NOT NULL DEFAULT 500,   -- platform fee, deducted at settlement
    inquiry_text        TEXT,
    created_by          TEXT NOT NULL DEFAULT 'sample' CHECK (created_by IN ('sample','agent','owner','promoter')),
    approved_at         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (ends_at > starts_at),
    CHECK (split_venue_bps + split_artist_bps + split_promoter_bps = 10000),
    CHECK (platform_fee_bps BETWEEN 0 AND 3000),
    -- Live bookings of the same space may not overlap within a session (and within shared data).
    -- Shared-vs-session overlap is checked in the application under an advisory lock.
    -- AI proposals do not reserve the slot.
    CONSTRAINT bookings_no_overlap EXCLUDE USING gist (
        space_id WITH =,
        session_key WITH =,
        tstzrange(starts_at, ends_at) WITH &&
    ) WHERE (status IN ('hold','approved','paid','completed'))
);
CREATE INDEX bookings_venue_time_idx ON bookings(venue_id, starts_at);
CREATE INDEX bookings_session_idx ON bookings(demo_session_id);

-- PayPal Orders v2. request_id is the idempotency key and is also sent as PayPal-Request-Id.
CREATE TABLE orders (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    booking_id        UUID NOT NULL REFERENCES bookings(id) ON DELETE CASCADE,
    kind              TEXT NOT NULL CHECK (kind IN ('deposit','ticket')),
    source            TEXT NOT NULL DEFAULT 'paypal_sandbox' CHECK (source IN ('paypal_sandbox','sample')),
    request_id        UUID NOT NULL UNIQUE,
    paypal_order_id   TEXT UNIQUE,
    paypal_capture_id TEXT UNIQUE,
    quantity          INT NOT NULL DEFAULT 1 CHECK (quantity > 0),
    amount_cents      BIGINT NOT NULL CHECK (amount_cents > 0),
    paypal_fee_cents  BIGINT,
    currency          TEXT NOT NULL DEFAULT 'USD',
    status            TEXT NOT NULL DEFAULT 'created'
                      CHECK (status IN ('created','approved','captured','failed','expired','refunded')),
    payer_email       TEXT,
    payer_country     TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    captured_at       TIMESTAMPTZ
);
-- At most one live deposit order per booking (prevents double payment of the same deposit).
CREATE UNIQUE INDEX orders_one_live_deposit
    ON orders(booking_id) WHERE kind = 'deposit' AND status IN ('created','approved','captured');
CREATE INDEX orders_booking_idx ON orders(booking_id);

-- Every webhook we receive. paypal_event_id is UNIQUE, so the same event is never processed twice.
CREATE TABLE webhook_events (
    id              BIGSERIAL PRIMARY KEY,
    paypal_event_id TEXT NOT NULL UNIQUE,
    event_type      TEXT NOT NULL,
    resource_id     TEXT,
    signature_valid BOOLEAN NOT NULL,
    payload         JSONB NOT NULL,
    received_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at    TIMESTAMPTZ,
    outcome         TEXT,
    error           TEXT
);

-- Double-entry ledger: payments and payouts live in the same ledger.
-- Every transaction (txn_id) sums to zero.
CREATE TABLE ledger_entries (
    id              BIGSERIAL PRIMARY KEY,
    txn_id          UUID NOT NULL,
    booking_id      UUID NOT NULL REFERENCES bookings(id) ON DELETE CASCADE,
    order_id        UUID REFERENCES orders(id) ON DELETE CASCADE,
    payout_item_id  UUID,
    kind            TEXT NOT NULL CHECK (kind IN ('payment','paypal_fee','platform_fee','payout','refund')),
    account         TEXT NOT NULL CHECK (account IN
                    ('payer','paypal_fees','platform_cash','platform_revenue','venue','artist','promoter')),
    amount_cents    BIGINT NOT NULL,
    currency        TEXT NOT NULL DEFAULT 'USD',
    memo            TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ledger_booking_idx ON ledger_entries(booking_id);
CREATE INDEX ledger_txn_idx ON ledger_entries(txn_id);

-- Settlement: one batch per show. sender_batch_id is UNIQUE, so a show is never paid twice.
CREATE TABLE payout_batches (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    booking_id         UUID NOT NULL UNIQUE REFERENCES bookings(id) ON DELETE CASCADE,
    sender_batch_id    TEXT NOT NULL UNIQUE,
    status             TEXT NOT NULL DEFAULT 'planned'
                       CHECK (status IN ('planned','submitted','succeeded','partial','failed')),
    paypal_batch_id    TEXT,
    scheduled_for      TIMESTAMPTZ NOT NULL,       -- 09:00 venue time on the day after the show
    net_cents          BIGINT NOT NULL,            -- money received for the show after PayPal fees and refunds
    platform_fee_cents BIGINT NOT NULL,
    currency           TEXT NOT NULL DEFAULT 'USD',
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    submitted_at       TIMESTAMPTZ,
    completed_at       TIMESTAMPTZ
);

CREATE TABLE payout_items (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    batch_id             UUID NOT NULL REFERENCES payout_batches(id) ON DELETE CASCADE,
    role                 TEXT NOT NULL CHECK (role IN ('venue','artist','promoter')),
    recipient_email      TEXT NOT NULL,
    amount_cents         BIGINT NOT NULL CHECK (amount_cents >= 0),
    sender_item_id       TEXT NOT NULL UNIQUE,
    status               TEXT NOT NULL DEFAULT 'planned'
                         CHECK (status IN ('planned','pending','succeeded','failed','unclaimed','returned')),
    paypal_item_id       TEXT,
    paypal_batch_id      TEXT,                      -- batch of the latest submission (retries go in new batches)
    last_sender_batch_id TEXT,
    attempts             INT NOT NULL DEFAULT 0,
    error                TEXT,
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- What the AI agent did: every message and every tool call, in order.
CREATE TABLE agent_runs (
    id              BIGSERIAL PRIMARY KEY,
    demo_session_id UUID REFERENCES demo_sessions(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL,
    booking_id      UUID REFERENCES bookings(id) ON DELETE SET NULL,
    step            INT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('user','assistant','tool_call','tool_result','blocked','error')),
    tool_name       TEXT,
    input           JSONB,
    output          JSONB,
    latency_ms      INT,
    input_tokens    INT,
    output_tokens   INT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX agent_runs_conv_idx ON agent_runs(conversation_id, step);

-- Daily LLM usage, for the spend cap.
CREATE TABLE llm_usage_daily (
    day           DATE PRIMARY KEY,
    turns         INT NOT NULL DEFAULT 0,
    input_tokens  BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0
);
