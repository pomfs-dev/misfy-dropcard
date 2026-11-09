-- Lifecycle timestamps, order confirmation source, links from bookings/webhooks to conversations.

ALTER TABLE bookings
    ADD COLUMN conversation_id UUID,
    ADD COLUMN requested_at    TIMESTAMPTZ,
    ADD COLUMN paid_at         TIMESTAMPTZ,
    ADD COLUMN completed_at    TIMESTAMPTZ,
    ADD COLUMN cancelled_at    TIMESTAMPTZ;
CREATE INDEX bookings_conversation_idx ON bookings(conversation_id);

ALTER TABLE orders
    -- how the payment was confirmed: a verified webhook, or the reconciliation job asking PayPal directly
    ADD COLUMN confirmed_by         TEXT CHECK (confirmed_by IN ('webhook','reconciliation')),
    ADD COLUMN capture_requested_at TIMESTAMPTZ,
    ADD COLUMN last_error           TEXT;

ALTER TABLE webhook_events
    ADD COLUMN booking_id UUID REFERENCES bookings(id) ON DELETE SET NULL;
CREATE INDEX webhook_events_booking_idx ON webhook_events(booking_id);

ALTER TABLE payout_batches
    ADD COLUMN last_error TEXT,
    ADD COLUMN attempts   INT NOT NULL DEFAULT 0;

-- Rebuilding sample data must never collide with a visitor's live booking: skip sample shows whose
-- slot is already taken by any live booking. Also fills the new lifecycle timestamps.
CREATE OR REPLACE FUNCTION _sample_show(p_space TEXT, p_artist TEXT, p_title TEXT, p_day DATE,
                                        p_start_hour INT, p_hours INT, p_ticket_cents BIGINT,
                                        p_sold_ratio DOUBLE PRECISION, p_status TEXT) RETURNS UUID
LANGUAGE plpgsql AS $$
DECLARE
    v        venues%ROWTYPE;
    s        spaces%ROWTYPE;
    a_id     UUID;
    p_id     UUID;
    b_id     UUID;
    starts   TIMESTAMPTZ;
    ends     TIMESTAMPTZ;
    deposit  BIGINT;
    target   INT;
    sold     INT := 0;
    qty      INT;
    country  TEXT;
    roll     DOUBLE PRECISION;
    sale_at  TIMESTAMPTZ;
BEGIN
    SELECT * INTO v FROM venues WHERE slug = 'blue-lantern';
    SELECT * INTO s FROM spaces WHERE venue_id = v.id AND name = p_space;
    SELECT id INTO a_id FROM artists WHERE slug = p_artist;
    SELECT id INTO p_id FROM promoters WHERE slug = 'nightfall-collective';

    starts  := (p_day + make_time(p_start_hour, 0, 0)) AT TIME ZONE 'Asia/Seoul';
    ends    := starts + make_interval(hours => p_hours);
    deposit := (s.day_rate_cents * 3000) / 10000;   -- 30% deposit

    IF EXISTS (SELECT 1 FROM bookings
                WHERE space_id = s.id
                  AND status IN ('hold','approved','paid','completed')
                  AND tstzrange(starts_at, ends_at) && tstzrange(starts, ends)) THEN
        RETURN NULL;   -- a visitor already holds this slot
    END IF;

    INSERT INTO bookings (venue_id, space_id, promoter_id, artist_id, title, genre, headcount,
                          starts_at, ends_at, status, quote_cents, deposit_cents, ticket_price_cents,
                          created_by, requested_at, approved_at, paid_at, completed_at,
                          created_at, updated_at)
    VALUES (v.id, s.id, p_id, a_id, p_title, (SELECT genre FROM artists WHERE id = a_id), s.capacity_max,
            starts, ends, p_status, s.day_rate_cents, deposit, p_ticket_cents,
            'sample',
            starts - interval '31 days',
            CASE WHEN p_status IN ('paid','completed') THEN starts - interval '30 days' END,
            CASE WHEN p_status IN ('paid','completed') THEN starts - interval '29 days' END,
            CASE WHEN p_status = 'completed' THEN ends END,
            starts - interval '32 days', starts - interval '30 days')
    RETURNING id INTO b_id;

    IF p_status = 'hold' THEN
        RETURN b_id;
    END IF;

    PERFORM _sample_capture(b_id, 'deposit', 1, deposit, 'KR', starts - interval '29 days');

    target := floor(s.capacity_max * p_sold_ratio);
    WHILE sold < target LOOP
        roll := random();
        qty := CASE WHEN roll < 0.62 THEN 1 WHEN roll < 0.90 THEN 2 ELSE 3 END;
        qty := LEAST(qty, target - sold);
        roll := random();
        country := CASE
            WHEN roll < 0.44 THEN 'KR' WHEN roll < 0.60 THEN 'JP' WHEN roll < 0.72 THEN 'US'
            WHEN roll < 0.80 THEN 'TW' WHEN roll < 0.86 THEN 'SG' WHEN roll < 0.91 THEN 'GB'
            WHEN roll < 0.95 THEN 'AU' ELSE 'DE' END;
        sale_at := starts - make_interval(secs => (random() * 25 * 86400)::int) - interval '1 hour';
        IF sale_at <= now() THEN
            PERFORM _sample_capture(b_id, 'ticket', qty, p_ticket_cents * qty, country, sale_at);
        END IF;
        sold := sold + qty;
    END LOOP;

    IF p_status = 'completed' THEN
        PERFORM _sample_settle(b_id, (p_day + 1 + time '09:00') AT TIME ZONE 'Asia/Seoul');
    END IF;
    RETURN b_id;
END $$;
