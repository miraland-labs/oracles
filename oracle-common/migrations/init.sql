-- oracle-common ledger bootstrap (PostgreSQL 15+).
--
-- One canonical schema for every oracle family. Per-family deployments run their own
-- DATABASE_URL (per design.md §Per-Family Postgres Isolation). Re-running this script
-- is safe: every CREATE uses IF NOT EXISTS.
--
-- Run before starting the binary:
--   psql "$DATABASE_URL" -f migrations/init.sql

-- ---------------------------------------------------------------------------
-- oracle_jobs: one row per on-chain payment_uid this oracle has been asked to settle.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS oracle_jobs (
    id                    BIGSERIAL PRIMARY KEY,
    payment_uid           TEXT NOT NULL UNIQUE,
    payment_pubkey        TEXT NOT NULL,
    mint                  TEXT NOT NULL,
    amount                BIGINT NOT NULL,
    sla_hash              TEXT NOT NULL,
    delivery_hash         TEXT NOT NULL,
    oracle_authority      TEXT NOT NULL,
    profile_id            TEXT,
    expires_at            TIMESTAMPTZ NOT NULL,
    status                TEXT NOT NULL DEFAULT 'detected',
    attempts              INTEGER NOT NULL DEFAULT 0,
    locked_at             TIMESTAMPTZ,
    started_at            TIMESTAMPTZ,
    completed_at          TIMESTAMPTZ,
    last_error            TEXT,
    settlement_signature  TEXT,
    resolution_hash       TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_oracle_jobs_status
    ON oracle_jobs (status ASC, updated_at ASC);
CREATE INDEX IF NOT EXISTS idx_oracle_jobs_payment_pubkey
    ON oracle_jobs (payment_pubkey ASC);
CREATE INDEX IF NOT EXISTS idx_oracle_jobs_oracle_authority
    ON oracle_jobs (oracle_authority ASC);
CREATE INDEX IF NOT EXISTS idx_oracle_jobs_profile_id
    ON oracle_jobs (profile_id ASC);

-- ---------------------------------------------------------------------------
-- oracle_verdicts: one-to-one with oracle_jobs once settled.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS oracle_verdicts (
    id                    BIGSERIAL PRIMARY KEY,
    oracle_job_id         BIGINT NOT NULL REFERENCES oracle_jobs (id) ON DELETE CASCADE,
    approved              BOOLEAN NOT NULL,
    resolution_reason     INTEGER NOT NULL,
    resolution_hash       TEXT NOT NULL,
    checks                JSONB NOT NULL,
    registry_sources      JSONB,
    settlement_signature  TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT oracle_verdicts_one_row_per_job UNIQUE (oracle_job_id)
);

CREATE INDEX IF NOT EXISTS idx_oracle_verdicts_resolution_hash
    ON oracle_verdicts (resolution_hash ASC);

-- ---------------------------------------------------------------------------
-- oracle_lifecycle_events: append-only audit log.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS oracle_lifecycle_events (
    id             BIGSERIAL PRIMARY KEY,
    oracle_job_id  BIGINT REFERENCES oracle_jobs (id) ON DELETE CASCADE,
    payment_uid    TEXT NOT NULL,
    event          TEXT NOT NULL,
    payload        JSONB,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_oracle_lifecycle_events_job
    ON oracle_lifecycle_events (oracle_job_id ASC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_oracle_lifecycle_events_payment_uid
    ON oracle_lifecycle_events (payment_uid ASC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_oracle_lifecycle_events_event
    ON oracle_lifecycle_events (event ASC);

-- ---------------------------------------------------------------------------
-- oracle_parameters: k/v runtime state (e.g., chain.last_seen_slot).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS oracle_parameters (
    id             BIGSERIAL PRIMARY KEY,
    param_name     TEXT NOT NULL,
    param_value    TEXT NOT NULL,
    inactive       BOOLEAN NOT NULL DEFAULT FALSE,
    effective_from TIMESTAMPTZ,
    expires_at     TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS uniq_oracle_parameters_param_name
    ON oracle_parameters (param_name ASC);

-- ---------------------------------------------------------------------------
-- oracle_seller_keys: registered seller wallets and their bearer-token digests.
-- The raw token is returned only at create / rotate time; only SHA256(token) is stored.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS oracle_seller_keys (
    id              BIGSERIAL PRIMARY KEY,
    wallet_pubkey   TEXT NOT NULL,
    bearer_sha256   TEXT NOT NULL,
    label           TEXT,
    revoked         BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at    TIMESTAMPTZ,
    CONSTRAINT oracle_seller_keys_unique UNIQUE (wallet_pubkey, bearer_sha256)
);

CREATE INDEX IF NOT EXISTS idx_oracle_seller_keys_wallet
    ON oracle_seller_keys (wallet_pubkey ASC);

-- ---------------------------------------------------------------------------
-- oracle_deliveries: catalog of sla / delivery / blob registrations.
-- (sha256_hex, kind) is globally unique because we are content-addressed.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS oracle_deliveries (
    id              BIGSERIAL PRIMARY KEY,
    sha256_hex      TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('sla', 'delivery', 'blob')),
    size_bytes      BIGINT NOT NULL,
    content_type    TEXT,
    seller_key_id   BIGINT REFERENCES oracle_seller_keys (id) ON DELETE SET NULL,
    profile_id      TEXT,
    storage_backend TEXT NOT NULL,
    storage_key     TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT oracle_deliveries_unique UNIQUE (sha256_hex, kind)
);

CREATE INDEX IF NOT EXISTS idx_oracle_deliveries_seller
    ON oracle_deliveries (seller_key_id ASC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_oracle_deliveries_profile_id
    ON oracle_deliveries (profile_id ASC);

-- ---------------------------------------------------------------------------
-- oracle_artifacts: inline storage for the postgres backend.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS oracle_artifacts (
    sha256_hex      TEXT PRIMARY KEY,
    bytes           BYTEA NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ---------------------------------------------------------------------------
-- oracle_registered_profiles: convenience cache of profiles registered at startup.
-- Truth is in code; this row supports `/registry/info` introspection.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS oracle_registered_profiles (
    profile_id      TEXT PRIMARY KEY,
    operator_pubkey TEXT NOT NULL,
    normative_url   TEXT,
    binary_version  TEXT,
    last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
