-- oracle-reputation schema · Increment 1
--
-- Single raw events table. Materialized roll-ups and HTTP API arrive in later
-- increments without changing this layer. Idempotent on replay: every event
-- row is keyed on (signature, log_index) so a restart re-streams without
-- duplicates.

CREATE TABLE IF NOT EXISTS oracle_events (
    signature        TEXT       NOT NULL,
    log_index        INTEGER    NOT NULL,
    slot             BIGINT     NOT NULL,
    block_time       BIGINT     NOT NULL,
    event_type       TEXT       NOT NULL,
    payment_uid      BYTEA      NOT NULL,
    payload          JSONB      NOT NULL,
    inserted_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (signature, log_index)
);

CREATE INDEX IF NOT EXISTS ix_oracle_events_payment_uid
    ON oracle_events (payment_uid);

CREATE INDEX IF NOT EXISTS ix_oracle_events_event_type_block_time
    ON oracle_events (event_type, block_time);

CREATE INDEX IF NOT EXISTS ix_oracle_events_slot
    ON oracle_events (slot);

-- Cursor table — last processed slot, so a restart can backfill since.
CREATE TABLE IF NOT EXISTS oracle_reputation_cursor (
    id           SMALLINT PRIMARY KEY DEFAULT 1,
    last_slot    BIGINT,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (id = 1)
);

INSERT INTO oracle_reputation_cursor (id, last_slot)
VALUES (1, NULL)
ON CONFLICT DO NOTHING;
