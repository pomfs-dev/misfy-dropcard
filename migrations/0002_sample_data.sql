-- Sample data generators. Every venue, artist, promoter and transaction created here is FICTIONAL
-- and marked as sample (bookings.demo_session_id IS NULL, orders.source = 'sample').
-- Sample data never reaches PayPal: background jobs only act on bookings that belong to a demo session.
--
-- refresh_sample_data() rebuilds the shared sample bookings relative to "today", so the calendar
-- and the dashboard look current whenever a judge opens the demo. A nightly cron job calls it.

CREATE OR REPLACE FUNCTION seed_entities() RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    v_blue UUID; v_harbor UUID; v_grey UUID;
BEGIN
    INSERT INTO venues (slug, name, city, country, tagline, description, tags, capacity_band, tz_offset_minutes, payout_email)
    VALUES
      ('blue-lantern', 'Blue Lantern Hall', 'Seoul', 'KR',
       'Three rooms and a rooftop in Mapo, Seoul',
       'An independent live venue with a main hall, a lounge, a rooftop stage and a rehearsal room. Touring acts from Tokyo, Taipei and beyond play here most weekends.',
       ARRAY['Live club','Rooftop','Late night'], '300–500', 540, 'venue@sample.dropcard.test'),
      ('harbor-room', 'Harbor Room', 'Busan', 'KR',
       'A small stage by the harbor',
       'A 200-capacity room for acoustic sets and intimate shows.',
       ARRAY['Acoustic','Bar'], '150–250', 540, 'harbor@sample.dropcard.test'),
      ('grey-matter', 'Grey Matter', 'Tokyo', 'JP',
       'Basement club for electronic nights',
       'A basement club with a strong sound system, built for DJ nights.',
       ARRAY['Club','Electronic'], '200–300', 540, 'grey@sample.dropcard.test')
    ON CONFLICT (slug) DO UPDATE
       SET name = EXCLUDED.name, city = EXCLUDED.city, country = EXCLUDED.country,
           tagline = EXCLUDED.tagline, description = EXCLUDED.description, tags = EXCLUDED.tags,
           capacity_band = EXCLUDED.capacity_band, tz_offset_minutes = EXCLUDED.tz_offset_minutes;

    SELECT id INTO v_blue FROM venues WHERE slug = 'blue-lantern';
    SELECT id INTO v_harbor FROM venues WHERE slug = 'harbor-room';
    SELECT id INTO v_grey FROM venues WHERE slug = 'grey-matter';

    INSERT INTO spaces (venue_id, name, capacity_min, capacity_max, day_rate_cents, sort_order) VALUES
      (v_blue, 'Main Hall',      300, 500, 240000, 1),
      (v_blue, 'Lounge',          80, 120,  90000, 2),
      (v_blue, 'Rooftop',        150, 200, 160000, 3),
      (v_blue, 'Rehearsal Room',  10,  20,  22000, 4),
      (v_harbor, 'Stage',        150, 250, 120000, 1),
      (v_grey, 'Club Floor',     200, 300, 180000, 1)
    ON CONFLICT (venue_id, name) DO UPDATE
       SET capacity_min = EXCLUDED.capacity_min, capacity_max = EXCLUDED.capacity_max,
           day_rate_cents = EXCLUDED.day_rate_cents, sort_order = EXCLUDED.sort_order;

    INSERT INTO artists (slug, name, genre, country, bio, fee_cents, payout_email) VALUES
      ('moonlit-static', 'Moonlit Static', 'Dream pop', 'KR',
       'Hazy guitars and soft vocals. A Seoul trio that sells out small rooms.', 180000, 'artist@sample.dropcard.test'),
      ('hollow-arcade', 'Hollow Arcade', 'Synthwave', 'JP',
       'Analog synths and drum machines from Osaka.', 250000, 'artist2@sample.dropcard.test'),
      ('paper-lanterns-club', 'Paper Lanterns Club', 'Indie folk', 'US',
       'Four-part harmonies and a banjo, touring Asia this winter.', 150000, 'artist3@sample.dropcard.test'),
      ('brass-comet', 'Brass Comet', 'Garage rock', 'GB',
       'Loud, fast and short sets. Big with the late crowd.', 200000, 'artist4@sample.dropcard.test')
    ON CONFLICT (slug) DO UPDATE
       SET name = EXCLUDED.name, genre = EXCLUDED.genre, country = EXCLUDED.country,
           bio = EXCLUDED.bio, fee_cents = EXCLUDED.fee_cents;

    INSERT INTO promoters (slug, name, email, payout_email) VALUES
      ('nightfall-collective', 'Nightfall Collective', 'booking@sample.dropcard.test', 'promoter@sample.dropcard.test')
    ON CONFLICT (slug) DO UPDATE SET name = EXCLUDED.name;
END $$;

-- A captured sample payment: order + balanced ledger transaction (payer -> PayPal fee + platform cash).
CREATE OR REPLACE FUNCTION _sample_capture(p_booking UUID, p_kind TEXT, p_qty INT, p_amount BIGINT,
                                           p_country TEXT, p_at TIMESTAMPTZ) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    o   UUID;
    fee BIGINT := (p_amount * 349 + 5000) / 10000 + 49;   -- sample fee model: 3.49% + $0.49
    t   UUID := gen_random_uuid();
BEGIN
    INSERT INTO orders (booking_id, kind, source, request_id, quantity, amount_cents, paypal_fee_cents,
                        status, payer_country, created_at, captured_at)
    VALUES (p_booking, p_kind, 'sample', gen_random_uuid(), p_qty, p_amount, fee,
            'captured', p_country, p_at, p_at)
    RETURNING id INTO o;
    INSERT INTO ledger_entries (txn_id, booking_id, order_id, kind, account, amount_cents, memo, created_at) VALUES
      (t, p_booking, o, 'payment',    'payer',         -p_amount,       'sample ' || p_kind,         p_at),
      (t, p_booking, o, 'paypal_fee', 'paypal_fees',    fee,            'sample PayPal fee',         p_at),
      (t, p_booking, o, 'payment',    'platform_cash',  p_amount - fee, 'sample ' || p_kind || ' net', p_at);
END $$;

-- Sample settlement with the same rule the server uses (see src/settlement.rs):
--   platform fee = round_half_up(net * platform_fee_bps / 10000), taken separately from deposit and tickets
--   deposit (after fee) -> venue
--   tickets (after fee) -> artist floor(x * artist_bps), promoter floor(x * promoter_bps), venue gets the rest
CREATE OR REPLACE FUNCTION _sample_settle(p_booking UUID, p_at TIMESTAMPTZ) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    b         bookings%ROWTYPE;
    dep       BIGINT; tix BIGINT;
    dep_fee   BIGINT; tix_fee BIGINT;
    dep_after BIGINT; tix_after BIGINT;
    a_amt     BIGINT; p_amt BIGINT; v_amt BIGINT;
    batch     UUID;
    item      UUID;
    r         RECORD;
    t         UUID;
BEGIN
    SELECT * INTO b FROM bookings WHERE id = p_booking;
    SELECT COALESCE(sum(l.amount_cents) FILTER (WHERE o.kind = 'deposit'), 0),
           COALESCE(sum(l.amount_cents) FILTER (WHERE o.kind = 'ticket'), 0)
      INTO dep, tix
      FROM ledger_entries l JOIN orders o ON o.id = l.order_id
     WHERE l.booking_id = p_booking AND l.account = 'platform_cash';

    dep_fee   := (dep * b.platform_fee_bps + 5000) / 10000;
    tix_fee   := (tix * b.platform_fee_bps + 5000) / 10000;
    dep_after := dep - dep_fee;
    tix_after := tix - tix_fee;
    a_amt     := CASE WHEN b.artist_id IS NULL THEN 0 ELSE tix_after * b.split_artist_bps / 10000 END;
    p_amt     := tix_after * b.split_promoter_bps / 10000;
    v_amt     := dep_after + (tix_after - a_amt - p_amt);   -- no artist: the artist share stays with the venue

    INSERT INTO payout_batches (booking_id, sender_batch_id, status, scheduled_for, net_cents,
                                platform_fee_cents, created_at, submitted_at, completed_at)
    VALUES (p_booking, 'sample-' || p_booking::text, 'succeeded', p_at, dep + tix,
            dep_fee + tix_fee, p_at, p_at, p_at)
    RETURNING id INTO batch;

    t := gen_random_uuid();
    INSERT INTO ledger_entries (txn_id, booking_id, kind, account, amount_cents, memo, created_at) VALUES
      (t, p_booking, 'platform_fee', 'platform_cash',    -(dep_fee + tix_fee), 'sample platform fee', p_at),
      (t, p_booking, 'platform_fee', 'platform_revenue',   dep_fee + tix_fee,  'sample platform fee', p_at);

    FOR r IN
        SELECT 'venue'::text AS role, v.payout_email AS email, v_amt AS amt FROM venues v WHERE v.id = b.venue_id
        UNION ALL
        SELECT 'artist', a.payout_email, a_amt FROM artists a WHERE a.id = b.artist_id
        UNION ALL
        SELECT 'promoter', p.payout_email, p_amt FROM promoters p WHERE p.id = b.promoter_id
    LOOP
        INSERT INTO payout_items (batch_id, role, recipient_email, amount_cents, sender_item_id, status,
                                  last_sender_batch_id, attempts, updated_at)
        VALUES (batch, r.role, r.email, r.amt, 'sample-' || p_booking::text || '-' || r.role, 'succeeded',
                'sample-' || p_booking::text, 1, p_at)
        RETURNING id INTO item;
        t := gen_random_uuid();
        INSERT INTO ledger_entries (txn_id, booking_id, payout_item_id, kind, account, amount_cents, memo, created_at) VALUES
          (t, p_booking, item, 'payout', 'platform_cash', -r.amt, 'sample payout to ' || r.role, p_at),
          (t, p_booking, item, 'payout', r.role,           r.amt, 'sample payout to ' || r.role, p_at);
    END LOOP;
END $$;

-- One sample show: booking + deposit + ticket sales (+ settlement when it is in the past).
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

    INSERT INTO bookings (venue_id, space_id, promoter_id, artist_id, title, genre, headcount,
                          starts_at, ends_at, status, quote_cents, deposit_cents, ticket_price_cents,
                          created_by, approved_at, created_at, updated_at)
    VALUES (v.id, s.id, p_id, a_id, p_title, (SELECT genre FROM artists WHERE id = a_id), s.capacity_max,
            starts, ends, p_status, s.day_rate_cents, deposit, p_ticket_cents,
            'sample',
            CASE WHEN p_status IN ('paid','completed') THEN starts - interval '30 days' END,
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

CREATE OR REPLACE FUNCTION refresh_sample_data() RETURNS TEXT LANGUAGE plpgsql AS $$
DECLARE
    today    DATE := (now() AT TIME ZONE 'Asia/Seoul')::date;
    fri0     DATE;   -- Friday of the current week (venue time)
    fri      DATE;
    artists  TEXT[] := ARRAY['moonlit-static','hollow-arcade','paper-lanterns-club','brass-comet'];
    titles   TEXT[] := ARRAY['Moonlit Static — Winter Haze','Hollow Arcade — Night Drive',
                             'Paper Lanterns Club — Asia Tour','Brass Comet — Loud & Late'];
    prices   BIGINT[] := ARRAY[3200, 3800, 2800, 3000];
    i        INT;
    w        INT;
    n        INT;
BEGIN
    PERFORM setseed(0.42);
    DELETE FROM bookings WHERE demo_session_id IS NULL;   -- cascades to sample orders, ledger, payouts

    fri0 := today + (5 - extract(isodow FROM today)::int);

    -- History: the last 8 weeks, two shows a weekend, all settled.
    FOR w IN 1..8 LOOP
        fri := fri0 - 7 * w;
        i := 1 + (w % 4);
        PERFORM _sample_show('Main Hall', artists[i], titles[i], fri, 19, 4, prices[i],
                             0.55 + random() * 0.4, 'completed');
        i := 1 + ((w + 1) % 4);
        PERFORM _sample_show(CASE WHEN w % 2 = 0 THEN 'Rooftop' ELSE 'Lounge' END,
                             artists[i], titles[i], fri + 1, 20, 3, prices[i] - 500,
                             0.6 + random() * 0.35, 'completed');
    END LOOP;

    -- Upcoming: paid shows with presales (green) and holds waiting for approval (striped).
    -- Keep the next 3 days and the AI demo dates mostly free so a live booking has room.
    PERFORM _sample_show('Main Hall',      'hollow-arcade',       'Hollow Arcade — Night Drive',      fri0 + 7,  19, 4, 3800, 0.45, 'paid');
    PERFORM _sample_show('Rooftop',        'paper-lanterns-club', 'Paper Lanterns Club — Asia Tour',  fri0 + 8,  18, 3, 2800, 0.30, 'paid');
    PERFORM _sample_show('Lounge',         'moonlit-static',      'Moonlit Static — Acoustic',        fri0 + 13, 20, 3, 2600, 0.25, 'paid');
    PERFORM _sample_show('Main Hall',      'brass-comet',         'Brass Comet — Loud & Late',        fri0 + 14, 19, 4, 3000, 0.20, 'paid');
    PERFORM _sample_show('Rehearsal Room', 'brass-comet',         'Brass Comet — Rehearsal',          fri0 + 12, 14, 4, NULL, 0, 'paid');
    PERFORM _sample_show('Main Hall',      'moonlit-static',      'Moonlit Static — Winter Haze II',  fri0 + 21, 19, 4, 3200, 0.10, 'hold');
    PERFORM _sample_show('Rooftop',        'hollow-arcade',       'Hollow Arcade — Sunset Set',       fri0 + 22, 17, 3, 3500, 0.10, 'hold');

    SELECT count(*) INTO n FROM bookings WHERE demo_session_id IS NULL;
    RETURN format('sample bookings: %s (today %s, venue time)', n, today);
END $$;
