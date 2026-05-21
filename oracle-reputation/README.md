# oracle-reputation

Reputation indexer for the x402 SLA-Escrow ecosystem. Stand-alone binary that
subscribes to `sla-escrow` program logs, decodes payment-lifecycle events, and
writes them as raw rows in a Postgres `oracle_events` table.

## What it does

- Subscribes to the on-chain program's `logsSubscribe` stream.
- Decodes nine payment-lifecycle events: `PaymentCreated`, `PaymentFunded`,
  `DeliverySubmitted`, `PaymentOracleConfirmed`, `PaymentReleased`,
  `PaymentRefunded`, `PaymentTTLExtended`, `PaymentClosed`, `PaymentExpired`.
- Inserts each event as a row keyed on `(signature, log_index)` — idempotent
  on replay.
- Backfills up to N recent signatures on startup so a brief downtime doesn't
  lose data.

## What it does NOT do (yet)

This is **Increment 1** of the architecture in `oracles/docs/REPUTATION_INDEXER.md`.
Out of scope here:

- Per-payment denormalized roll-up table (Increment 2).
- SQL views for scorecards (Increment 2).
- HTTP API for public consumption (Increment 3).

Operators get the raw event ledger now and can build scorecards via direct SQL
until the higher increments arrive.

## Deploying

```bash
# 1. Provision Postgres
psql -d oracle_reputation -f migrations/001_init.sql

# 2. Configure
cp .env.example /etc/oracle/reputation.env
# edit DATABASE_URL, SOLANA_*

# 3. Run
cargo run --release --bin oracle-reputation
```

## SQL examples (works today against the raw `oracle_events` table)

```sql
-- Per-oracle settlement count for the last 7 days.
SELECT
    payload->>'oracleAuthority' AS oracle,
    COUNT(*)                    AS verdicts
FROM oracle_events
WHERE event_type = 'payment_oracle_confirmed'
  AND block_time > EXTRACT(EPOCH FROM NOW() - INTERVAL '7 days')
GROUP BY oracle
ORDER BY verdicts DESC;

-- Approval rate per oracle.
SELECT
    payload->>'oracleAuthority' AS oracle,
    COUNT(*) FILTER (WHERE (payload->>'resolutionState')::int = 1) * 1.0
        / COUNT(*) AS approval_rate
FROM oracle_events
WHERE event_type = 'payment_oracle_confirmed'
GROUP BY oracle;

-- Economic refusals (resolution_reason in 200..=219).
SELECT
    payload->>'oracleAuthority' AS oracle,
    COUNT(*) AS economic_refusals
FROM oracle_events
WHERE event_type = 'payment_oracle_confirmed'
  AND (payload->>'resolutionReason')::int BETWEEN 200 AND 219
GROUP BY oracle;
```

See `oracles/docs/REPUTATION_INDEXER.md` for the full design and the
materialized-view layer planned for Increment 2.
