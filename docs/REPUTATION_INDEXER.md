# Oracle Reputation Indexer (Architecture Spec)

> Status: **design**, not yet implemented. Targets the existing on-chain
> events emitted by `sla-escrow` `0.3.1`. No program redeploy required.

The reputation indexer turns raw on-chain log data into per-oracle scorecards
that sellers, buyers, Facilitators, and operators can use to make routing
decisions. It is **observation infrastructure**, not enforcement: it does not
gate any transaction, does not run any consensus, does not arbitrate
disputes. It just computes useful aggregates over events the program already
emits.

---

## 1. Why an indexer (and why now)

The `sla-escrow` market is three-sided:

```
        SELLERS                BUYERS
          │ pick which          │ pick which
          │ oracle to advertise │ Facilitators to use
          ▼                     ▼
     ┌──────────────────────────────┐
     │       FACILITATORS           │
     └──────────────────────────────┘
                   │
                   ▼
                ORACLES   ← compete on price floor, latency,
                            accuracy, uptime
```

Every actor needs the same information: **which oracles do what they say they
do?** Today that information is scattered across thousands of confirmed
transactions. The indexer aggregates it into a stable, queryable surface.

The market is already structurally ready for competition (low switching cost,
permissionless oracle deployment). What it lacks is **observable signal**.
This spec describes the simplest possible substrate to surface it.

---

## 2. What's already on-chain (no program changes needed)

The `sla-escrow` program emits the following events (verified in
`sla-escrow/api/src/event.rs`). Each is a Pod log line that any Solana
indexer can decode:

| Event | Trigger | Indexer use |
|---|---|---|
| `PaymentCreatedWithFundEvent` | `FundPayment` | Pin `payment_uid → oracle_authority`, `amount`, `mint`, `expires_at`, `created_at` |
| `DeliverySubmittedEvent` | `SubmitDelivery` | "Job offered to oracle X at time T" — denominator for settlement rate |
| `PaymentOracleConfirmedEvent` | `ConfirmOracle` | "Oracle X settled at time T with reason R, state S" — numerator |
| `PaymentReleasedEvent` | `ReleasePayment` | Confirms `oracle_tip` actually paid; `is_expired` flag |
| `PaymentRefundedEvent` | `RefundPayment` | Confirms `oracle_tip` paid (or zero) and which path closed it |
| `PaymentTTLExtendedEvent` | `ExtendPaymentTTL` | Distinguishes "buyer extended" from "expired without verdict" |

Critically, `PaymentOracleConfirmedEvent` carries `oracle_authority` directly —
the indexer does not need to cross-reference the original `Payment` account to
attribute a verdict to an oracle.

### Resolution-reason taxonomy already in use

`PaymentOracleConfirmedEvent.resolution_reason` (u16) follows
`sla_escrow_api::resolution::ResolutionReason`:

| Range | Meaning |
|---|---|
| `0` | Approval (no specific reason) |
| `1..=7`, `255` | Standard rejection reasons (cross-oracle interoperable) |
| `100..=102` | Active Guardian protective rejects (this crate's `error::guardian_reason`) |
| `200..=219` | Operator-economics rejections (this crate's `error::economic_reason`) |
| `256..=319` | `x402/onchain-transfer/v1` family-specific |
| `320..=383` | `x402/file-delivery/attestation/v1` family-specific |
| `384..=447` | reserved (`x402/compute-result/v1` future) |
| `448..=511` | reserved ecosystem-wide |
| `512..` | per-deployment custom |

The indexer can bucket rejections by these ranges to distinguish:
- **Honest verdicts** (state=2, reason ≤ 7): the oracle disagreed with the seller's evidence.
- **Guardian rejects** (state=2, reason 100-102): SLA / evidence not retrievable, or pipeline timed out.
- **Economic refusals** (state=2, reason 200-219): operator declined for cost-recovery reasons.
- **Family-specific rejects** (state=2, reason ≥ 256): family-defined failure modes (transfer not found, blob too large, etc.).

This bucketing is what makes the scorecard useful. A seller looking at "oracle
X has 92% approval, 5% honest reject, 3% economic refusal" learns something
genuinely actionable.

---

## 3. Architecture

```
                    Solana RPC (mainnet or devnet)
                              │
            ┌─────────────────┴──────────────────┐
            │  logsSubscribe filtered by         │
            │  ESCROW_PROGRAM_ID                 │
            └─────────────────┬──────────────────┘
                              │
                              ▼
                   ┌───────────────────────┐
                   │   Event decoder       │
                   │   (Pod parsers from   │
                   │    sla_escrow_api)    │
                   └──────────┬────────────┘
                              │
                              ▼
                   ┌───────────────────────┐
                   │   Postgres ledger     │  ← idempotent writes keyed
                   │   (one row per event) │     on (signature, log_index)
                   └──────────┬────────────┘
                              │
                ┌─────────────┴─────────────┐
                ▼                           ▼
       ┌────────────────┐        ┌──────────────────┐
       │  SQL views     │        │   HTTP API       │
       │  (scorecards)  │        │   /v1/oracles    │
       └────────────────┘        │   /v1/oracles/X  │
                                 └──────────────────┘
```

### 3.1 Event decoder

A small Rust binary (or a pr402 endpoint, if we want to colocate) that:
1. Subscribes to `logsSubscribe` filtered by `escrow_program_id` at `confirmed` commitment.
2. Parses each `Program data:` line as one of the 19 event variants in `sla_escrow_api::event`.
3. Inserts into a Postgres table keyed on `(signature, log_index)` — idempotent on replay.
4. On startup, backfills using `getSignaturesForAddress` + `getTransaction` for the last N slots so a brief downtime doesn't lose data.

This is essentially the same shape as the existing oracle chain monitor in
`oracle-common::chain`; we're already proven on the read side. ~400 lines of
new code, mostly schema + parser plumbing.

### 3.2 Postgres schema (proposed)

Minimal table set. Materialized views compute the scorecards.

```sql
-- Raw event log (one row per emitted event).
CREATE TABLE oracle_events (
    signature        TEXT NOT NULL,
    log_index        INTEGER NOT NULL,
    slot             BIGINT NOT NULL,
    block_time       BIGINT NOT NULL,
    event_type       TEXT NOT NULL,    -- 'payment_created', 'delivery_submitted', etc.
    payment_uid      BYTEA NOT NULL,
    payload          JSONB NOT NULL,    -- full event struct as JSON
    PRIMARY KEY (signature, log_index)
);
CREATE INDEX ix_oracle_events_payment_uid ON oracle_events (payment_uid);
CREATE INDEX ix_oracle_events_event_type_block_time ON oracle_events (event_type, block_time);

-- Per-payment denormalized roll-up (for fast scorecard queries).
CREATE TABLE oracle_payments (
    payment_uid          BYTEA PRIMARY KEY,
    oracle_authority     TEXT NOT NULL,
    mint                 TEXT NOT NULL,
    amount               BIGINT NOT NULL,
    oracle_fee_bps       INTEGER NOT NULL,
    created_at           BIGINT NOT NULL,
    expires_at           BIGINT NOT NULL,
    delivery_submitted_at BIGINT,
    confirm_timestamp    BIGINT,
    resolution_state     SMALLINT,    -- NULL=pending, 1=approved, 2=rejected
    resolution_reason    INTEGER,
    released             BOOLEAN NOT NULL DEFAULT FALSE,
    refunded             BOOLEAN NOT NULL DEFAULT FALSE,
    oracle_tip_paid      BIGINT      -- raw mint units, NULL until release/refund
);
CREATE INDEX ix_oracle_payments_oracle_authority ON oracle_payments (oracle_authority);
CREATE INDEX ix_oracle_payments_block_time ON oracle_payments (created_at);
```

The `oracle_events` table is the canonical raw record. The `oracle_payments`
table is a denormalized projection updated as new events arrive — fast for
scorecard queries, easy to rebuild from `oracle_events` if needed.

### 3.3 Scorecard queries

Each metric is one SQL view. Five core metrics give a useful starting page.

```sql
-- 1. Per-oracle settlement rate (rolling 30 days)
CREATE VIEW v_oracle_settlement_rate AS
SELECT
    oracle_authority,
    COUNT(*) FILTER (WHERE delivery_submitted_at IS NOT NULL) AS jobs_offered,
    COUNT(*) FILTER (WHERE confirm_timestamp IS NOT NULL) AS jobs_settled,
    CASE WHEN COUNT(*) FILTER (WHERE delivery_submitted_at IS NOT NULL) > 0
        THEN COUNT(*) FILTER (WHERE confirm_timestamp IS NOT NULL)::NUMERIC
             / COUNT(*) FILTER (WHERE delivery_submitted_at IS NOT NULL)
        ELSE NULL
    END AS settlement_rate
FROM oracle_payments
WHERE created_at > EXTRACT(EPOCH FROM NOW() - INTERVAL '30 days')
GROUP BY oracle_authority;

-- 2. Per-oracle approval rate
CREATE VIEW v_oracle_approval_rate AS
SELECT
    oracle_authority,
    COUNT(*) FILTER (WHERE resolution_state = 1) AS approved,
    COUNT(*) FILTER (WHERE resolution_state = 2) AS rejected,
    COUNT(*) FILTER (WHERE resolution_state IS NOT NULL) AS settled,
    CASE WHEN COUNT(*) FILTER (WHERE resolution_state IS NOT NULL) > 0
        THEN COUNT(*) FILTER (WHERE resolution_state = 1)::NUMERIC
             / COUNT(*) FILTER (WHERE resolution_state IS NOT NULL)
        ELSE NULL
    END AS approval_rate
FROM oracle_payments
WHERE created_at > EXTRACT(EPOCH FROM NOW() - INTERVAL '30 days')
GROUP BY oracle_authority;

-- 3. Per-oracle rejection bucket breakdown (key for the seller's decision)
CREATE VIEW v_oracle_rejection_buckets AS
SELECT
    oracle_authority,
    COUNT(*) FILTER (WHERE resolution_state = 2 AND resolution_reason BETWEEN 1 AND 7) AS honest_rejects,
    COUNT(*) FILTER (WHERE resolution_state = 2 AND resolution_reason BETWEEN 100 AND 102) AS guardian_rejects,
    COUNT(*) FILTER (WHERE resolution_state = 2 AND resolution_reason BETWEEN 200 AND 219) AS economic_refusals,
    COUNT(*) FILTER (WHERE resolution_state = 2 AND resolution_reason >= 256) AS family_specific_rejects
FROM oracle_payments
WHERE created_at > EXTRACT(EPOCH FROM NOW() - INTERVAL '30 days')
GROUP BY oracle_authority;

-- 4. Per-oracle settlement latency
CREATE VIEW v_oracle_settlement_latency AS
SELECT
    oracle_authority,
    PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY confirm_timestamp - delivery_submitted_at) AS p50_seconds,
    PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY confirm_timestamp - delivery_submitted_at) AS p95_seconds,
    PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY confirm_timestamp - delivery_submitted_at) AS p99_seconds
FROM oracle_payments
WHERE confirm_timestamp IS NOT NULL
  AND delivery_submitted_at IS NOT NULL
  AND created_at > EXTRACT(EPOCH FROM NOW() - INTERVAL '30 days')
GROUP BY oracle_authority;

-- 5. Per-oracle by-amount-bucket performance (the lane diagnostic)
CREATE VIEW v_oracle_by_amount_bucket AS
SELECT
    oracle_authority,
    CASE
        WHEN amount <  1000000 THEN 'micro'   -- < $1 USDC
        WHEN amount < 10000000 THEN 'small'   -- $1-$10
        WHEN amount < 100000000 THEN 'medium' -- $10-$100
        ELSE 'large'                          -- > $100
    END AS bucket,
    COUNT(*) AS payments_offered,
    COUNT(*) FILTER (WHERE resolution_state IS NOT NULL) AS payments_settled,
    COUNT(*) FILTER (WHERE resolution_state = 2 AND resolution_reason BETWEEN 200 AND 219) AS economic_refusals
FROM oracle_payments
WHERE created_at > EXTRACT(EPOCH FROM NOW() - INTERVAL '30 days')
GROUP BY oracle_authority, bucket;
```

These five views are the core. A seller comparing oracles sees them merged
into one row per oracle. A Facilitator surfaces the same rows on
`/capabilities` so buyers can see which oracles their advertised seller
trusted, and how those oracles have actually performed.

### 3.4 HTTP API

```
GET /v1/oracles
  → list of all oracle_authority pubkeys with summarized scorecards
  → no auth (public observability infrastructure)

GET /v1/oracles/<pubkey>
  → detailed scorecard for one oracle:
    {
      "oracle_authority": "ABC...",
      "window": { "start": ..., "end": ..., "days": 30 },
      "totals": { "offered": 1247, "settled": 1198 },
      "settlement_rate": 0.961,
      "approval_rate": 0.83,
      "rejections": {
        "honest": 178,
        "guardian": 8,
        "economic": 23,
        "family_specific": 12
      },
      "latency_seconds": { "p50": 23, "p95": 87, "p99": 142 },
      "by_amount_bucket": {
        "micro":  { "offered": 412, "settled": 380, "rate": 0.92, "economic_refusals": 22 },
        "small":  { "offered": 503, "settled": 502, "rate": 0.998 },
        "medium": { "offered": 332, "settled": 332, "rate": 1.00 }
      }
    }

GET /v1/oracles/<pubkey>/payments?since=<ts>&until=<ts>&limit=100
  → raw payment-level history for forensic analysis

GET /v1/markets/oracles?profile_id=<x402/.../v1>
  → leaderboard of oracles serving a particular profile, sorted by
    settlement_rate × approval_rate (composite quality score)
```

Stateless, cacheable, public. This is the "yellow pages" surface other
ecosystem actors integrate against.

---

## 4. Why this beats the alternatives

| Alternative | What it achieves | Drawback |
|---|---|---|
| **Oracle's `/v1/policy` only** | Pre-trade transparency | Self-reported. An oracle can claim a low floor and ghost; no one knows until after the fact. |
| **Facilitator pre-flight gate** | Buyer never funds an underpriced payment | Tooling lock-in. A buyer using sla-escrow CLI bypasses entirely. |
| **On-chain reputation table** | Tamper-proof | Requires program bump (Pod layout change), expensive on-chain reads, computed centrally anyway. |
| **This indexer** | Public, derivable, computable by anyone | Off-chain (any party can host their own); requires running a Postgres + decoder |

The indexer wins because:
- **Anyone can run one.** It's pure read infrastructure. If pr402's instance disagrees with a third-party's, both are observable; the chain is the source of truth.
- **No program change.** Today's emitted events are sufficient.
- **Composes with /v1/policy.** Policy says "what I'll do"; reputation says "what I did". Together they're complete.
- **Composes with the Facilitator.** Facilitator can choose which oracles to advertise based on reputation scorecards, without enforcing.

---

## 5. Operational considerations

- **Replay-safe.** Every event row is keyed on `(signature, log_index)`; the decoder is idempotent. Restarts are cheap.
- **No PII.** Public pubkeys + public hashes. No personal data risk.
- **Bandwidth.** `logsSubscribe` is cheap; the program emits ~1 KB per payment lifecycle on average. A busy Facilitator processing 1000 payments/day generates ~1 MB/day of indexer ingress. Trivial.
- **Postgres sizing.** Each `oracle_events` row is ~1 KB JSON. 1M lifetime payments → 5-10 GB. Single-node Postgres handles this comfortably for years.
- **Privacy.** Sellers may not want their `seller` pubkey enumerable. The indexer's API can hash or omit `seller` in public responses while keeping it in the underlying table for forensic queries. (A Facilitator-internal indexer keeps full data; a public mirror redacts.)

---

## 6. Bootstrapping path

We don't have to ship the full design at once. Three increments:

### Increment 1 — Event decoder + raw table only

Just stream events into `oracle_events`. No views, no API. Sets the substrate
in place. ~250 lines of new code. Operators can SQL-query directly until the
HTTP layer arrives.

### Increment 2 — Per-payment roll-up + the five core views

Adds `oracle_payments` table maintained by triggers (or by the decoder
directly). Adds the five SQL views. Operators can query scorecards via SQL.
~150 additional lines.

### Increment 3 — HTTP API

Read-only endpoints listed in §3.4. ~200 additional lines (Axum + serde).
Public consumption surface.

Total: ~600 lines for full reputation infrastructure. Self-contained crate
inside `oracles/oracle-reputation/` (alongside the family crates) or a
dedicated subcommand of pr402. The decision of where to house it depends on
who runs it: if it's pr402-operated, colocate; if multiple parties want to run
their own, make it a standalone binary.

---

## 7. What's NOT in scope

- **Slashing.** No automated penalty mechanism. Reputation is observation; consequences live in seller / buyer routing decisions.
- **Cross-Facilitator reputation aggregation.** Each Facilitator's indexer sees only the chain; if multiple Facilitators run their own indexers, results converge naturally.
- **Sybil resistance.** An oracle operator can spin up multiple authorities to game scorecards. The defense is not technical — it's that buyers / sellers / Facilitators can choose to trust only operators with sustained track record. The indexer surfaces that track record; routing decisions weight it.
- **Real-time alerts.** This is reporting, not paging. Operators wanting alerting layer it on top of the SQL views.

---

## 8. What this unlocks

Once the indexer is live:

1. **Sellers can pick oracles intelligently.** "Show me oracles with >95% settlement rate on micro-payments" is a simple query.
2. **Facilitators can curate `/capabilities` advertisements.** Surface only oracles passing reputation thresholds.
3. **Oracles compete on observable performance.** Bad oracles fade; good oracles get more advertisements.
4. **Buyers can trust the data.** It's derived from on-chain, not Facilitator self-reports.
5. **Operators can debug their own oracle.** "Why did my settlement rate drop yesterday?" — query the event log directly.

This is the meter the market needs. With it, the lever (reputation-based
routing) actually works. Without it, the lever exists but cannot be pulled.

---

## 9. Implementation status

| Component | Status |
|---|---|
| On-chain events emitted | ✓ shipped in `0.3.1` |
| `EconomicRefusal` reason code (200) | ✓ shipped in `oracle-common` (`error::economic_reason::TIP_BELOW_OPERATOR_FLOOR`) |
| Eager economic-refusal verdict | ✓ shipped in `oracle-common::worker` (operator opt-in via `ORACLE_TIP_FLOOR_ENABLED=true`; default OFF) |
| Event decoder | ✓ shipped in `oracle-reputation` Increment 1 |
| Postgres schema | ✓ shipped in `oracle-reputation/migrations/001_init.sql` |
| WebSocket ingester + backfill | ✓ shipped in `oracle-reputation::ingester` |
| Per-payment roll-up table | ☐ Increment 2 |
| SQL scorecard views | ☐ Increment 2 |
| HTTP API | ☐ Increment 3 |
| Refund sweeper (Facilitator-side auto-refund) | Deferred — buyers self-refund; see `pr402/docs/REFUND_SWEEPER.md` for rationale |

When the indexer is implemented, it should not require any change to the
oracle, the Facilitator, or the on-chain program. It is a strictly additive
observability layer.

## 10. Verdict-vs-refund: a critical distinction for scorecards

`PaymentOracleConfirmedEvent` records a verdict. **It does not move tokens.**
Tokens move only when somebody invokes `RefundPayment` (after a rejection) or
`ReleasePayment` (after an approval).

This means the indexer should distinguish three lifecycle states:

1. **Settled**: `PaymentOracleConfirmedEvent` emitted → verdict is on-chain.
2. **Closed-out**: `PaymentReleasedEvent` or `PaymentRefundedEvent` emitted → tokens have actually moved.
3. **Stuck**: Settled but neither released nor refunded → buyer or seller has not yet acted.

For oracle scorecards, the indexer's "settlement rate" should count the
**Settled** transition (verdict on-chain). That's what the oracle has direct
control over. Whether the buyer subsequently submits the `RefundPayment`
(after the on-chain `Config.refund_cooldown_seconds` elapses) is a separate
operational metric — useful for observing buyer-SDK quality and refund
latency in aggregate, but NOT an oracle scorecard input:

```sql
-- Median time from rejection verdict to actual refund (across all rejected
-- payments). With buyer self-refund as the canonical flow, this is bounded
-- below by `Config.refund_cooldown_seconds` (1h floor, 24h current default).
-- Useful for ecosystem-wide UX observability — NOT an oracle metric.
CREATE VIEW v_post_rejection_refund_latency AS
SELECT
    PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY refund_block_time - confirm_timestamp) AS p50_seconds,
    PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY refund_block_time - confirm_timestamp) AS p95_seconds
FROM oracle_payments
WHERE resolution_state = 2
  AND refunded = true
  AND created_at > EXTRACT(EPOCH FROM NOW() - INTERVAL '30 days');
```

If we observe the median latency anchored at the cooldown floor (e.g.
exactly 24h to the second), it means buyer SDKs are respecting the cooldown
and self-refunding promptly — the protocol is healthy. Latency far above
the cooldown means buyer-side refund logic is missing or broken.
