# Oracle Architecture

## Overview

The x402 oracle is an off-chain service that watches the SLA-Escrow program on
Solana, evaluates whether sellers fulfilled their Service Level Agreements, and
posts binding verdicts (`ConfirmOracle`) on-chain. It acts as an **active
guardian** — defaulting to buyer protection when evaluation cannot complete.

Each oracle binary serves one **family** (e.g. `onchain-transfer`,
`api-quality`, `file-delivery`) but the core infrastructure (`oracle-common`)
is shared. Multiple oracle instances can run in parallel; each is assigned to
specific payments via the `Payment.oracle_authority` field set at `FundPayment`
time.

---

## Component Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         Oracle Binary (per family)                       │
│                                                                         │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐               │
│  │ Chain Monitor│   │    Worker    │   │   Settler    │               │
│  │              │──►│              │──►│              │               │
│  │ logsSubscribe│   │ Pipeline +   │   │ ConfirmOracle│               │
│  │ + Backfill   │   │ Guardian     │   │ tx builder   │               │
│  └──────┬───────┘   └──────┬───────┘   └──────┬───────┘               │
│         │                   │                   │                       │
│         │            ┌──────┴───────┐           │                       │
│         │            │   Profile    │           │                       │
│         │            │   Registry   │           │                       │
│         │            │ (evaluators) │           │                       │
│         │            └──────────────┘           │                       │
│         │                                       │                       │
│  ┌──────┴───────────────────────────────────────┴───────┐              │
│  │                    HTTP Server (Axum)                  │              │
│  │  /health  /stats  /metrics  /evaluate                 │              │
│  │  /v1/registry/{info,sla,delivery,blob,{sha256_hex}}   │              │
│  │  /v1/registry/seller/{challenge,register,rotate}      │              │
│  └───────────────────────────────────────────────────────┘              │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
         │                    │                    │
         ▼                    ▼                    ▼
┌─────────────┐     ┌─────────────────┐    ┌──────────────┐
│ Solana RPC  │     │ Evidence        │    │  Postgres    │
│ (HTTP + WS) │     │ Registry        │    │  (ledger)    │
│             │     │ (self-hosted)   │    │              │
└─────────────┘     └─────────────────┘    └──────────────┘
```

---

## Processing Flow (Current — Active Guardian)

```
                         SOLANA CHAIN                    ORACLE (off-chain)
                         ════════════                    ══════════════════

  ┌────────────────────────────────────┐
  │ 1. FundPayment                     │
  │    buyer USDC → escrow PDA         │
  │    payment.oracle_authority = ORA  │
  │    payment.delivery_timestamp = 0  │
  │    payment.resolution_state = 0    │
  └────────────────────────────────────┘
                    │
                    │  (seller delivers off-chain,
                    │   uploads SLA + evidence to registry,
                    │   calls SubmitDelivery)
                    │
                    ▼
  ┌────────────────────────────────────┐
  │ 2. SubmitDelivery                  │
  │    payment.delivery_hash = H       │
  │    payment.delivery_timestamp = T  │
  │    emits DeliverySubmittedEvent     │
  └────────────────────────────────────┘
                    │
                    │  logsSubscribe (program-wide)
                    ▼
                                        ┌───────────────────────────────────┐
                                        │ 3. Chain Monitor                  │
                                        │    • Detects DeliverySubmittedEvent│
                                        │    • Reads Payment account        │
                                        │    • Checks oracle_authority==self│
                                        │    • Pre-fetches SLA bytes        │
                                        │    • Emits EvaluationJob          │
                                        └───────────────┬───────────────────┘
                                                        │
                                                        ▼
                                        ┌───────────────────────────────────┐
                                        │ 4. Worker                         │
                                        │    • Deduplicates by payment_uid  │
                                        │    • Checks is_eligible()         │
                                        │    • Runs pipeline (timeout)      │
                                        └───────────────┬───────────────────┘
                                                        │
                                         ┌──────────────┴──────────────┐
                                         │                             │
                                    Pipeline OK                   Pipeline ERROR
                                         │                             │
                                         ▼                             ▼
                              ┌─────────────────┐        ┌─────────────────────────┐
                              │ 5a. Evaluate    │        │ 5b. Guardian Logic      │
                              │  • Parse SLA    │        │  • is_retriable()?      │
                              │  • Fetch evidence│       │  • Near expiry?         │
                              │  • Run checks   │        │    (now > expires_at    │
                              │  • Compute hash │        │     - 10min margin)     │
                              └────────┬────────┘        └──────────┬──────────────┘
                                       │                            │
                                       │                 ┌──────────┴──────────┐
                                       │                 │                     │
                                       │            Near expiry /         Not near expiry
                                       │            retries exhausted     & retries remain
                                       │                 │                     │
                                       │                 ▼                     ▼
                                       │      ┌──────────────────┐   ┌────────────────┐
                                       │      │ REJECT           │   │ Re-queue       │
                                       │      │ (fail-closed)    │   │ retry_count++  │
                                       │      │ reason=100/101/  │   │ sleep(backoff) │
                                       │      │        102       │   │ → back to (4)  │
                                       │      └────────┬─────────┘   └────────────────┘
                                       │               │
                                       ▼               ▼
                              ┌────────────────────────────────────┐
                              │ 6. Settler                         │
                              │    • Compute resolution_hash       │
                              │    • Build ConfirmOracle ix        │
                              │      (from_uid_bytes variant)      │
                              │    • Sign with oracle keypair      │
                              │    • send_and_confirm (skip        │
                              │      preflight for slot-lag)       │
                              └────────────────┬───────────────────┘
                                               │
                                               ▼
  ┌────────────────────────────────────┐
  │ 7. ConfirmOracle (on-chain)        │
  │    payment.resolution_state = 1|2  │
  │    payment.resolution_hash = H     │
  │    payment.resolution_reason = R   │
  └────────────────────────────────────┘
                    │
                    ▼
  ┌────────────────────────────────────┐
  │ 8. ReleasePayment / RefundPayment  │
  │    (anyone can call once resolved) │
  │    • state=1 → release to seller   │
  │    • state=2 → refund to buyer     │
  └────────────────────────────────────┘
```

---

## Event Subscription Model

The oracle subscribes to **all** `logsSubscribe` events for the sla-escrow
program id. This is program-wide — every `SubmitDelivery` from every seller
arrives at every oracle instance.

**Filtering** happens in `read_payment()`:
```
if payment.oracle_authority != self.oracle_pubkey → skip (not my job)
if payment.delivery_timestamp == 0               → skip (no delivery yet)
if payment.resolution_state != 0                 → skip (already resolved)
```

On a busy program with many oracles, each instance receives N events but only
processes its own subset. This is bandwidth-inefficient at scale but correct.
Solana's `logsSubscribe` does not support per-account filtering within a
program.

---

## Active Guardian: Retry + Fail-Closed

When the pipeline fails due to missing SLA or evidence bytes (seller hasn't
uploaded, or registry is temporarily down), the worker does NOT drop the job.
Instead:

### Retry path
- Exponential backoff: 10s → 20s → 40s → … → 120s cap.
- Up to 30 attempts (configurable).
- Job is re-queued to the channel after sleeping.

### Fail-closed path (buyer protection)
Triggered when:
- `now > expires_at - ORACLE_REJECT_SAFETY_MARGIN_SEC` (default 600s / 10 min), OR
- All retry attempts exhausted.

Action: oracle issues `ConfirmOracle` with `resolution_state = 2` (REJECTED)
and a guardian-specific `resolution_reason`:

| Code | Name | Meaning |
|------|------|---------|
| 100 | SLA_UNAVAILABLE | SLA bytes not retrievable after retries |
| 101 | EVIDENCE_UNAVAILABLE | Evidence bytes not retrievable after retries |
| 102 | EVALUATION_TIMEOUT | Pipeline did not complete within safety margin |

After rejection, the buyer can call `RefundPayment` themselves once
`Config.refund_cooldown_seconds` has elapsed since funding. The current
pr402 deployment uses **24 hours**; the on-chain admin may shorten via
`UpdateConfig` to as little as **1 hour** (the program-enforced floor at
`MIN_REFUND_COOLDOWN_WHEN_ENABLED_SECONDS`). The cooldown is **policy,
not promise** — buyer SDKs should read it live from the `Config` PDA.

The seller and bank authority can refund without waiting for the
cooldown, which is why operators occasionally do mass-recovery via the
admin path; for steady-state agentic flows the buyer's self-refund is
the canonical path. See `pr402/docs/REFUND_SWEEPER.md` for why pr402
does NOT run an auto-sweep on the buyer's behalf.

### Oracle's stricter cutoff

```
Timeline:
├─── FundPayment ──────────────────────────────────────── expires_at ──►
│                                                                       │
│    ┌── On-chain delivery_cutoff (5 min) ──┐                           │
│    │  Seller must SubmitDelivery before    │                           │
│    │  expires_at - 5min                    │                           │
│    └──────────────────────────────────────┘                           │
│                                                                       │
│         ┌── Oracle reject margin (10 min) ──┐                         │
│         │  Oracle rejects if no verdict by   │                         │
│         │  expires_at - 10min                │                         │
│         └────────────────────────────────────┘                         │
│                                                                       │
│    Seller's effective window: FundPayment → (expires_at - 10min)      │
│    = TTL - 10min (e.g. 3600 - 600 = 3000s = 50 min)                  │
└───────────────────────────────────────────────────────────────────────┘
```

The 5-min gap between oracle margin and on-chain cutoff absorbs:
- Oracle tx confirmation time (~10s devnet, ~2s mainnet).
- Worker queue processing delay.
- RPC latency spikes.

---

## Configuration (Environment Variables)

### Core
| Variable | Default | Description |
|----------|---------|-------------|
| `SOLANA_RPC_URL` | `https://api.devnet.solana.com` | HTTP RPC endpoint |
| `SOLANA_WS_URL` | `wss://api.devnet.solana.com` | WebSocket endpoint for logsSubscribe |
| `ESCROW_PROGRAM_ID` | `sla_escrow_api::ID` | SLA-Escrow program to monitor |
| `ORACLE_KEYPAIR_PATH` | required | Ed25519 keypair for signing ConfirmOracle |
| `BIND_ADDR` | `127.0.0.1:4020` | HTTP server listen address |
| `DATABASE_URL` | optional | Postgres for ledger + dead-letter (recommended) |

### Active Guardian
| Variable | Default | Description |
|----------|---------|-------------|
| `ORACLE_RETRY_INITIAL_DELAY_SEC` | 10 | First retry backoff (seconds) |
| `ORACLE_RETRY_MAX_DELAY_SEC` | 120 | Backoff cap (seconds) |
| `ORACLE_MAX_RETRY_ATTEMPTS` | 30 | Max retries before forced reject |
| `ORACLE_REJECT_SAFETY_MARGIN_SEC` | 600 | Seconds before expiry to issue reject |

### Evidence Registry & Storage
| Variable | Default | Description |
|----------|---------|-------------|
| `EVIDENCE_REGISTRY_URLS` | `http://localhost:4021` | Comma-separated mirror URLs for fallback fetching |
| `EVIDENCE_REGISTRY_URL` | `http://localhost:4021` | Single fallback fetch URL (used if `EVIDENCE_REGISTRY_URLS` is empty) |
| `EVIDENCE_REGISTRY_AUTH_HEADER` | optional | `Authorization` header for GET fetch requests |
| `ORACLE_REGISTRY_BACKEND` | required | Upload storage backend: `postgres`, `s3`, or `local` |
| `ORACLE_REGISTRY_MAX_BYTEA_BYTES` | 4MB | Max size of inline documents (SLA/Evidence JSON) |
| `ORACLE_REGISTRY_MAX_BLOB_BYTES` | 5GB | Max size of streamed blobs |

### S3 Backend Configuration (Required if `ORACLE_REGISTRY_BACKEND=s3`)
| Variable | Default | Description |
|----------|---------|-------------|
| `ORACLE_REGISTRY_S3_ENDPOINT` | required | S3 service endpoint URL |
| `ORACLE_REGISTRY_S3_BUCKET` | required | Target S3 bucket name |
| `ORACLE_REGISTRY_S3_ACCESS_KEY` | required | S3 access key |
| `ORACLE_REGISTRY_S3_SECRET_KEY` | required | S3 secret key |
| `ORACLE_REGISTRY_S3_REGION` | `us-east-1` | S3 region |

---

## Security Properties

| Property | Guarantee |
|----------|-----------|
| **Buyer protection** | If seller withholds artifacts, oracle rejects before expiry → buyer refunds |
| **Seller fairness** | If registry is temporarily down, oracle retries → artifacts arrive → fair evaluation |
| **No false rejects** | 10-min margin + 30 retries over ~20 min means honest sellers who upload within `TTL - 10min` always get evaluated |
| **Oracle SOL bounded** | Only 1 tx per job (approve or reject); retries are off-chain sleeps |
| **Idempotent** | If `resolution_state` already set, ConfirmOracle tx fails harmlessly on-chain |
| **Deterministic** | Same job + same registry state → same verdict across runs |
| **Isolation** | Oracle A ignores payments assigned to oracle B (filtered by `oracle_authority`) |

---

## Threat Model

| Attack | Mitigation |
|--------|-----------|
| Seller withholds SLA/evidence, waits for expiry self-release | Guardian rejects before expiry → buyer refunds |
| Seller submits garbage `delivery_hash` | Oracle fetches evidence by hash → 404 → guardian rejects |
| Seller never calls SubmitDelivery | `delivery_timestamp == 0` → on-chain blocks release → buyer refunds after expiry |
| Oracle goes offline entirely | "Oracle ghosted" on-chain fallback releases to seller after expiry (acceptable: oracle uptime is the operator's SLA to buyers) |
| Registry DDoS prevents oracle from fetching | Retry with backoff; if still down at margin → reject (fail-closed protects buyer) |
| Replay of old evidence | Evaluator checks `evidence.timestamp >= payment.created_at` (Wave A §1.1) |

---

## Deployment Topology

```
┌─────────────────────────────────────────────────────────┐
│                    loy-app01 (Huawei Cloud)              │
│                                                         │
│  ┌─────────────────────────────────────────────────┐    │
│  │ oracle-onchain-transfer-devnet (Docker)         │    │
│  │   BIND_ADDR=127.0.0.1:4021                     │    │
│  │   ESCROW_PROGRAM_ID=s5zkKiy8...                 │    │
│  │   SOLANA_RPC_URL=https://devnet.helius-rpc.com  │    │
│  │   SOLANA_WS_URL=wss://api.devnet.solana.com    │    │
│  └─────────────────────────────────────────────────┘    │
│                                                         │
│  ┌─────────────────────────────────────────────────┐    │
│  │ oracle-onchain-transfer-mainnet (Docker)        │    │
│  │   BIND_ADDR=127.0.0.1:4031                     │    │
│  │   ESCROW_PROGRAM_ID=SEscZ6n2...                 │    │
│  │   SOLANA_RPC_URL=https://mainnet.helius-rpc.com │    │
│  │   SOLANA_WS_URL=wss://api.mainnet-beta.solana   │    │
│  └─────────────────────────────────────────────────┘    │
│                                                         │
│  ┌─────────────────────────────────────────────────┐    │
│  │ nginx (TLS termination)                         │    │
│  │   oracle-devnet.159-138-5-240.nip.io → :4021   │    │
│  │   oracle-mainnet.159-138-5-240.nip.io → :4031  │    │
│  └─────────────────────────────────────────────────┘    │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

---

## Crate Structure

```
oracles/
├── oracle-common/          # Shared: chain monitor, worker, settler, server, types
│   └── src/
│       ├── chain.rs        # logsSubscribe + backfill
│       ├── worker.rs       # Job loop + Active Guardian retry/reject
│       ├── settler.rs      # ConfirmOracle tx builder + resolution hash
│       ├── pipeline.rs     # Dispatch by profile_id → evaluator
│       ├── server.rs       # Axum HTTP (health, stats, evaluate, registry)
│       ├── config.rs       # Env-driven config
│       ├── error.rs        # Error taxonomy + is_retriable + guardian codes
│       ├── types.rs        # EvaluationJob, RuntimeHealth, etc.
│       ├── profile.rs      # ProfileRegistry + ProfileBinding
│       ├── evaluator.rs    # OracleEvaluator trait
│       └── fetcher.rs      # EvidenceFetcher trait
├── oracle-onchain-transfer/  # Family: SPL token transfer verification
├── oracle-api-quality/       # Family: HTTP API quality evaluation
├── oracle-file-delivery/     # Family: File/blob delivery attestation
├── scripts/docker/           # Dockerfile, systemd units, deploy script
└── docs/                     # This file + DEPLOYMENT.md, OPERATIONS.md, etc.
```
