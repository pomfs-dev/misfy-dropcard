-- AI concierge conversations. The exact API message history is stored append-only as JSON text
-- (not JSONB, which would reorder keys), so every request replays earlier turns byte for byte —
-- this keeps the prompt cache warm and the model's thinking blocks valid across turns.

CREATE TABLE conversations (
    id              UUID PRIMARY KEY,
    demo_session_id UUID NOT NULL REFERENCES demo_sessions(id) ON DELETE CASCADE,
    venue_id        UUID NOT NULL REFERENCES venues(id),
    model           TEXT NOT NULL,
    turns           INT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_turn_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX conversations_session_idx ON conversations(demo_session_id);

CREATE TABLE agent_messages (
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    seq             INT NOT NULL,
    role            TEXT NOT NULL CHECK (role IN ('user','assistant')),
    content         TEXT NOT NULL,   -- JSON array of content blocks, exactly as sent to the API
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, seq)
);

ALTER TABLE llm_usage_daily
    ADD COLUMN cache_read_tokens  BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN cache_write_tokens BIGINT NOT NULL DEFAULT 0;
