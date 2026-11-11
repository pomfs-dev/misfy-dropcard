-- The AI venue scout. Artists search a directory of venues (data/venue_directory.csv: fictional sample data,
-- or an anonymized export from MiSFY's discovery data) ranked by activity, and the scout books on their behalf.
-- Scout conversations are not tied to one venue.

ALTER TABLE venues
    ADD COLUMN featured        BOOLEAN NOT NULL DEFAULT false,  -- the hand-made sample venue cards
    ADD COLUMN region          TEXT,
    ADD COLUMN kind            TEXT,                            -- NULL when the source does not say
    ADD COLUMN genres          TEXT[] NOT NULL DEFAULT '{}',    -- broad categories, e.g. 'Electronic / DJ'
    ADD COLUMN shows_on_record INT,                             -- shows the discovery data has on record (rounded)
    ADD COLUMN activity        INT CHECK (activity BETWEEN 0 AND 100),  -- percentile of shows_on_record;
                                                                -- NULL = not in the scout's directory
    ADD COLUMN data_source     TEXT NOT NULL DEFAULT 'sample';  -- 'sample' (fictional) | 'discovery' (anonymized)

CREATE INDEX venues_activity_idx ON venues (activity DESC) WHERE activity IS NOT NULL;

ALTER TABLE conversations ALTER COLUMN venue_id DROP NOT NULL;
ALTER TABLE conversations
    ADD COLUMN scope     TEXT NOT NULL DEFAULT 'venue' CHECK (scope IN ('venue', 'scout')),
    ADD COLUMN artist_id UUID REFERENCES artists(id),               -- the artist the scout works for
    ADD CONSTRAINT conversations_scope_venue CHECK ((scope = 'venue') = (venue_id IS NOT NULL));
