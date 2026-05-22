# Oracle Developer Guide

> Build your own oracle family for the x402 SLA-Escrow ecosystem.

## Who is this for?

You are a developer with **domain expertise** (DeFi, AI inference, file
delivery, IoT attestation, gaming, etc.) who wants to build an oracle
that evaluates whether sellers fulfilled their Service Level
Agreements. Your oracle will:

1. Watch the SLA-Escrow program for `SubmitDelivery` events.
2. Fetch the SLA document and delivery evidence from the registry.
3. Evaluate whether the evidence satisfies the SLA rules you define.
4. Post a binding verdict (`ConfirmOracle`) on-chain.

The pr402 ecosystem provides the infrastructure (chain monitor,
registry, settler, HTTP server). You provide the **evaluation logic**
and the **normative specification** that defines what "fulfilled"
means in your domain.

> **Normative reference.** Oracle obligations are specified in
> [`spec/sla-escrow-protocol/v1`](../spec/sla-escrow-protocol/v1/NORMATIVE.md)
> §5. Your registry serves [`spec/registry-http-api/v1`](../spec/registry-http-api/v1/NORMATIVE.md).
> Your SLA documents extend [`spec/sla-document/v1`](../spec/sla-document/v1/NORMATIVE.md).
> The on-chain instruction set you produce (`ConfirmOracle`) and the
> account state you read (`Payment`) are specified in
> [`spec/sla-escrow-onchain-abi/v1`](../spec/sla-escrow-onchain-abi/v1/NORMATIVE.md).

## Big picture

Four actors collaborate around your oracle:

| Actor | Role |
|---|---|
| Buyer | Pays into escrow, authors (or accepts) the SLA |
| Seller | Delivers the service, uploads SLA + evidence, calls SubmitDelivery |
| **Oracle (you)** | Evaluates evidence vs SLA, posts binding verdict |
| pr402 | Discovery, tx assembly, optional health gate |

Sequence (oracle's perspective):

1. Seller uploads SLA + evidence to YOUR registry.
2. Seller calls `SubmitDelivery` on-chain.
3. Your binary's chain monitor sees `DeliverySubmittedEvent` via
   `logsSubscribe`.
4. You read the `Payment` PDA, confirm `oracle_authority == self.pubkey`
   and `delivery_timestamp != 0` and `resolution_state == 0`.
5. Fetch SLA + evidence from the registry by hash; re-verify SHA-256.
6. Run your domain evaluator; produce verdict + reason + checks.
7. Compute `resolution_hash` over the canonical envelope.
8. Submit `ConfirmOracle`.

What the oracle does NOT do: hold or move funds, author SLA or
evidence, interact with buyer directly, modify payment state except
via `ConfirmOracle`.

### Active Guardian (built into oracle-common)

If the seller withholds SLA or evidence, your oracle:

- Retries fetching with exponential backoff (10s → 120s cap, 30
  attempts).
- Issues a protective REJECT (resolution_reason 100/101/102) if
  artifacts remain unavailable within `ORACLE_REJECT_SAFETY_MARGIN_SEC`
  (default 600s) of `expires_at`.

This protects buyers from a malicious seller calling `SubmitDelivery`
without uploading proof. You get this for free.

## Architecture at a glance

```
Your Oracle Binary
├── oracle-common (provided; you don't modify)
│   ├── Chain monitor (logsSubscribe + backfill)
│   ├── Worker (retry + Active Guardian)
│   ├── Settler (ConfirmOracle tx builder)
│   ├── HTTP server (/health, /v1/policy, /v1/registry/*)
│   ├── Registry (SLA + evidence storage)
│   └── Fetcher (content-addressed retrieval + verify)
│
└── YOUR CODE (~200-500 lines)
    ├── SLA struct (Deserialize + Serialize)
    ├── Evidence struct (Deserialize + Serialize)
    ├── impl OracleEvaluator (your domain logic)
    ├── main.rs (wire evaluator + fetchers + registry)
    └── NORMATIVE.md (your published rules)
```

You write ~3-4 files. `oracle-common` handles everything else.

## Prerequisites

- Rust toolchain (stable, 1.75+).
- Familiarity with Solana concepts (PDAs, transactions, RPC).
- A domain where "fulfilled" can be defined as deterministic,
  verifiable checks.
- A Solana keypair funded with SOL (for ConfirmOracle gas).

Read first:

- [`spec/sla-escrow-protocol/v1`](../spec/sla-escrow-protocol/v1/NORMATIVE.md) (your obligations)
- [`spec/sla-escrow-onchain-abi/v1`](../spec/sla-escrow-onchain-abi/v1/NORMATIVE.md) (`ConfirmOracle` bytes, `Payment` layout, PDA seeds)
- [`spec/registry-http-api/v1`](../spec/registry-http-api/v1/NORMATIVE.md) (HTTP contract you serve)
- [`spec/sla-document/v1`](../spec/sla-document/v1/NORMATIVE.md) (SLA envelope)
- [`ARCHITECTURE.md`](./ARCHITECTURE.md) (oracle internals)
- One existing per-family normative (e.g. `oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md`)

## Quick start (5 minutes to a running oracle on devnet)

```bash
# 1. Clone and build
cd oracles/
cargo build --release --bin oracle-onchain-transfer

# 2. Generate a devnet keypair (fund with ~0.1 SOL for gas)
solana-keygen new -o /tmp/oracle-dev.json
solana airdrop 0.1 $(solana-keygen pubkey /tmp/oracle-dev.json) --url devnet

# 3. Start with minimal config
ORACLE_KEYPAIR_PATH=/tmp/oracle-dev.json \
ORACLE_REGISTRY_BACKEND=local \
SOLANA_RPC_URL=https://api.devnet.solana.com \
SOLANA_WS_URL=wss://api.devnet.solana.com \
TRANSFER_CLUSTER=devnet \
cargo run --release --bin oracle-onchain-transfer

# 4. Verify it's alive
curl -s http://localhost:4020/health | jq .status
curl -s http://localhost:4020/v1/policy | jq .
```

For your own family, replace `oracle-onchain-transfer` with your new
crate and follow Steps 1–8.

## Step 1: Define your profile

A **profile** is a versioned rule family identified by a canonical
string per `spec/sla-document/v1` §5.1:

```
x402/oracles/<your-family>/<version>
```

Examples: `x402/oracles/onchain-transfer/v1`, `x402/oracles/api-quality/v1`,
`x402/oracles/file-delivery/attestation/v1`, `x402/oracles/gpu-inference/v1`.

Rules:

- Lowercase, slash-separated, no spaces.
- Version is a single integer (`v1`, `v2`, …). Bump on breaking schema
  changes.
- Once published, a profile id is **immutable** — never reuse with
  different semantics.
- Each oracle binary registers exactly one profile (per spec §5.1).

## Step 2: Define your SLA schema

The SLA extends the cross-family envelope from
[`sla-document/v1`](../spec/sla-document/v1/NORMATIVE.md) §5 with your
domain fields.

Required envelope (all families):

```json
{
  "version": 1,
  "profile_id": "x402/oracles/<your-family>/v1",
  "payment_uid": "<64 lowercase hex chars>",
  "buyer_nonce": "<64 hex chars (optional but recommended)>"
}
```

Add your domain fields. Example for a hypothetical GPU inference oracle:

```json
{
  "version": 1,
  "profile_id": "x402/oracles/gpu-inference/v1",
  "payment_uid": "abc123...",
  "model": "llama-3-70b",
  "max_latency_ms": 5000,
  "min_tokens": 500,
  "prompt_hash": "sha256-of-prompt-bytes"
}
```

The bytes-to-hash binding (per `sla-document/v1` §3): the buyer's
`sla_hash` is SHA-256 over the exact bytes uploaded to the registry. No
canonicalization is performed by the protocol; sellers using delegated
authoring MUST produce deterministic bytes themselves (sorted-keys is
the standard approach).

## Step 3: Define your evidence schema

Required envelope:

```json
{
  "version": 1,
  "profile_id": "x402/oracles/<your-family>/v1",
  "payment_uid": "<64 hex — must match SLA>"
}
```

Plus your domain proof fields. Evidence bytes hashed with SHA-256
become `delivery_hash`, anchored on-chain via `SubmitDelivery`.

## Step 4: Implement `OracleEvaluator`

```rust
use oracle_common::{
    evaluator::{EvaluationContext, OracleEvaluator},
    error::OracleError,
    types::{EvaluationResult, CheckResult},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInferenceSla {
    pub version: u32,
    pub profile_id: String,
    pub payment_uid: String,
    pub model: String,
    pub max_latency_ms: u64,
    pub min_tokens: u64,
    pub prompt_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInferenceEvidence {
    pub version: u32,
    pub profile_id: String,
    pub payment_uid: String,
    pub response_hash: String,
    pub latency_ms: u64,
    pub tokens_generated: u64,
    pub timestamp: i64,
}

pub struct GpuInferenceEvaluator;

#[async_trait]
impl OracleEvaluator for GpuInferenceEvaluator {
    type Sla = GpuInferenceSla;
    type Evidence = GpuInferenceEvidence;

    fn profile_id(&self) -> &'static str {
        "x402/oracles/gpu-inference/v1"
    }

    async fn evaluate(
        &self,
        ctx: &EvaluationContext<'_>,
        sla: &Self::Sla,
        evidence: &Self::Evidence,
    ) -> Result<EvaluationResult, OracleError> {
        let mut checks = Vec::new();
        let mut all_pass = true;

        // profile_id matches
        let profile_ok = sla.profile_id == self.profile_id();
        checks.push(CheckResult { name: "profile_id".into(), passed: profile_ok,
            detail: format!("expected={}, got={}", self.profile_id(), sla.profile_id) });
        all_pass &= profile_ok;

        // payment_uid matches between SLA and evidence
        let uid_ok = sla.payment_uid == evidence.payment_uid;
        checks.push(CheckResult { name: "payment_uid_match".into(), passed: uid_ok,
            detail: "SLA and evidence payment_uid must match".into() });
        all_pass &= uid_ok;

        // latency within SLA bounds
        let latency_ok = evidence.latency_ms <= sla.max_latency_ms;
        checks.push(CheckResult { name: "latency".into(), passed: latency_ok,
            detail: format!("actual={}ms, max={}ms", evidence.latency_ms, sla.max_latency_ms) });
        all_pass &= latency_ok;

        // freshness: evidence must not predate the payment
        let fresh_ok = ctx.job.created_at == 0 || evidence.timestamp >= ctx.job.created_at;
        checks.push(CheckResult { name: "freshness".into(), passed: fresh_ok,
            detail: "evidence.timestamp >= payment.created_at".into() });
        all_pass &= fresh_ok;

        Ok(EvaluationResult {
            approved: all_pass,
            resolution_reason: if all_pass { 0 } else { 1 },
            checks,
        })
    }
}
```

### Rules for `evaluate()`

1. **Deterministic.** Same inputs → same output. No wall-clock reads,
   no randomness.
2. **Fail-closed.** If in doubt, reject. Buyer protection is the
   default.
3. **Check `payment_uid` match** between SLA and evidence (cross-payment
   replay defense).
4. **Check `profile_id`** exact-match (prevents mis-routing).
5. **Check freshness**: `evidence.timestamp >= payment.created_at`
   defeats pre-funding evidence replay.
6. **Return `CheckResult` list.** Each check is logged and surfaced in
   the resolution hash for auditability.

## Step 5: Wire it up (main.rs)

```rust
use std::sync::Arc;
use oracle_common::{
    fetcher::RegistryJsonFetcher,
    profile::{ProfileBinding, ProfileRegistry, RegisteredProfile},
};

let evaluator = Arc::new(GpuInferenceEvaluator);
let sla_fetcher = Arc::new(RegistryJsonFetcher::<GpuInferenceSla>::new(
    http.clone(), fetcher_cfg.clone(),
));
let evidence_fetcher = Arc::new(RegistryJsonFetcher::<GpuInferenceEvidence>::new(
    http.clone(), fetcher_cfg.clone(),
));

let mut profiles = ProfileRegistry::new();
profiles.register(RegisteredProfile {
    profile_id: "x402/oracles/gpu-inference/v1",
    run: Arc::new(ProfileBinding {
        evaluator,
        sla_fetcher,
        evidence_fetcher,
    }),
});
```

The rest (chain monitor, worker, settler, HTTP server) is provided.
Copy structure from any existing family binary;
`oracle-onchain-transfer/src/main.rs` is the simplest reference.

## Step 6: Write your normative spec

Create `your-oracle/spec/<family>-v1/NORMATIVE.md`. Define:

1. **SLA schema** — every field, type, required/optional, semantics.
2. **Evidence schema** — same.
3. **Evaluation rules** — numbered, unambiguous checks.
4. **Resolution reason codes** — what each code means.
5. **Versioning policy** — when you bump, what breaks.

### Resolution reason code ranges (u16)

| Range | Owner |
|---|---|
| `0` | Approval (no reason) |
| `1..=7`, `255` | Standard rejection reasons (cross-oracle interoperable) |
| `100..=102` | Active Guardian protective rejects (built into oracle-common) |
| `200..=219` | Operator-economics refusals (built into oracle-common) |
| `256..=319` | `x402/onchain-transfer/*` family |
| `320..=383` | `x402/file-delivery/*` family |
| `384..=447` | Reserved (`x402/compute-result/*` future) |
| `448..=511` | Reserved ecosystem-wide |
| `512..=65535` | Per-deployment / your custom family |

If your family is new, use `512+` and document codes in your
`NORMATIVE.md`. Contact ecosystem maintainers to reserve a dedicated
range once your family stabilizes.

Reference style: existing per-family normatives under
`oracles/oracle-*/spec/<profile>/NORMATIVE.md`.

## Step 7: Deploy

### Docker

```dockerfile
FROM rust:1.82 AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --bin oracle-gpu-inference

FROM debian:bookworm-slim
COPY --from=builder /src/target/release/oracle-gpu-inference /usr/local/bin/oracle
ENTRYPOINT ["/usr/local/bin/oracle"]
```

### Environment variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `SOLANA_RPC_URL` | no | devnet | HTTP RPC for the cluster you serve |
| `SOLANA_WS_URL` | no | devnet | WebSocket for logsSubscribe |
| `ESCROW_PROGRAM_ID` | no | `sla_escrow_api::ID` | SLA-Escrow program to monitor |
| `ORACLE_KEYPAIR_PATH` | yes | - | Ed25519 keypair (signs ConfirmOracle) |
| `BIND_ADDR` | no | `127.0.0.1:4020` | HTTP server bind address |
| `DATABASE_URL` | no | - | Postgres URL (recommended) |
| `ORACLE_REGISTRY_BACKEND` | yes | - | `postgres`, `s3`, or `local` |
| `EVIDENCE_REGISTRY_URLS` | no | `http://localhost:4021` | Mirror URLs for fallback fetching |
| `ORACLE_REGISTRY_MAX_BYTEA_BYTES` | no | `4 MiB` | Max SLA / evidence JSON size |
| `ORACLE_REGISTRY_MAX_BLOB_BYTES` | no | `5 GiB` | Max blob size |
| `ORACLE_RETRY_INITIAL_DELAY_SEC` | no | `10` | First retry delay |
| `ORACLE_RETRY_MAX_DELAY_SEC` | no | `120` | Retry backoff cap |
| `ORACLE_MAX_RETRY_ATTEMPTS` | no | `30` | Max retries before protective reject |
| `ORACLE_REJECT_SAFETY_MARGIN_SEC` | no | `600` | Safety margin before expiry |
| `ORACLE_TIP_FLOOR_ENABLED` | no | `false` | Master switch for tip-floor gate |
| `ORACLE_MIN_VERDICT_TIP_DEFAULT_RAW` | no | (USDC fallback) | Default tip floor (raw mint units) |
| `ORACLE_MIN_VERDICT_TIP_BY_MINT_RAW` | no | `{}` | Per-mint tip floor JSON map |

S3 backend (only when `ORACLE_REGISTRY_BACKEND=s3`):

| Variable | Required | Description |
|---|---|---|
| `ORACLE_REGISTRY_S3_ENDPOINT` | yes | S3 endpoint URL |
| `ORACLE_REGISTRY_S3_BUCKET` | yes | Target bucket |
| `ORACLE_REGISTRY_S3_ACCESS_KEY` | yes | S3 access key |
| `ORACLE_REGISTRY_S3_SECRET_KEY` | yes | S3 secret key |
| `ORACLE_REGISTRY_S3_REGION` | no | Defaults to `us-east-1` |

### Systemd unit

```ini
[Unit]
Description=x402 oracle-gpu-inference
After=network-online.target

[Service]
ExecStart=/usr/bin/docker run --rm --name oracle-gpu-inference \
  --network host --env-file /etc/oracle/gpu-inference.env \
  oracle-gpu-inference:current
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

## Step 8: Get listed on pr402

Once your oracle is healthy:

1. Register your oracle pubkey with the pr402 operator (`PR402_ORACLE_AUTHORITIES`).
2. Publish your normative spec at a stable URL.
3. pr402's `/capabilities` will list your `profileId` + `operatorPubkey`
   under `slaEscrowOracleProfiles[]`.

## Testing

### Unit tests

Your evaluator is pure: same SLA + same evidence → same verdict.

```rust
#[tokio::test]
async fn rejects_when_latency_exceeds_sla() {
    let evaluator = GpuInferenceEvaluator;
    let sla = GpuInferenceSla { max_latency_ms: 100, /* ... */ };
    let evidence = GpuInferenceEvidence { latency_ms: 200, /* ... */ };
    let ctx = /* mock EvaluationContext */;
    let result = evaluator.evaluate(&ctx, &sla, &evidence).await.unwrap();
    assert!(!result.approved);
}
```

### Manual evaluation (live oracle, no chain)

```bash
curl -X POST http://localhost:4020/evaluate \
  -H "Authorization: Bearer $ORACLE_OPERATOR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"sla": {...}, "evidence": {...}}'
```

### Devnet end-to-end

Reference scripts:

- `oracle-onchain-transfer/tests/devnet/transfer_v1.sh`
- `spl-token-balance-serverless/scripts/test-buy-spl-token-devnet.sh`

## What sellers need from you

| Artifact | Where | Purpose |
|---|---|---|
| `NORMATIVE.md` | Stable URL (GitHub) | SLA fields + evidence shape |
| Registry endpoint | Your oracle's `/v1/registry/*` | Sellers upload SLA + evidence |
| Policy endpoint | Your oracle's `GET /v1/policy` | Tip-floor + supported profiles |
| Oracle pubkey | Keypair public key | Sellers list in `oracleAuthorities[]` |
| Profile ID | e.g. `x402/oracles/gpu-inference/v1` | Sellers list in 402 envelope |
| SLA / evidence JSON Schemas | In `NORMATIVE.md` or as `.json` | Sellers validate before upload |

## Reference implementations

| Family | Crate | Profile ID | Complexity |
|---|---|---|---|
| On-chain Transfer | `oracle-onchain-transfer` | `x402/oracles/onchain-transfer/v1` | Medium (RPC balance checks) |
| API Quality | `oracle-api-quality` | `x402/oracles/api-quality/v1` | High (HTTP probing + TLS) |
| File Delivery | `oracle-file-delivery` | `x402/oracles/file-delivery/attestation/v1` | Low (hash + size check) |

Start by reading `oracle-onchain-transfer` — the most straightforward
evaluator (~200 lines of domain logic).

## Operator economics: recommended `oracle_fee_bps`

The on-chain program enforces only an upper bound (5%). It has no
built-in floor. Operators pick a tip that pays for ConfirmOracle and
any external evaluation work.

### Cost per verdict (Solana mainnet, ~$100/SOL)

| Cost line | Typical |
|---|---|
| Solana base fee (5000 lamports) | ~$0.0005 |
| Priority fee (congested periods) | $0.002–$0.010 |
| RPC calls | $0.000–$0.005 (per-provider) |
| Active Guardian retries (worst case ~30 fetches) | up to $0.030 in RPC budget |

A single happy-path verdict costs **$0.003–$0.015**. Anything below is
a net loss.

### Current default

The pr402 reference Facilitator opens its canonical USDC Escrow at
**100 bps (1%)**. At that rate:

- $5 payment → $0.05 tip (comfortably above cost).
- $50 payment → $0.50 (generous).
- $0.50 payment → $0.005 (marginal but viable).

### Operator self-protection (off by default)

```bash
ORACLE_TIP_FLOOR_ENABLED=true                    # master switch
ORACLE_MIN_VERDICT_TIP_DEFAULT_RAW=5000          # ~$0.005 USDC
ORACLE_MIN_VERDICT_TIP_BY_MINT_RAW='{
  "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v": 5000,
  "So11111111111111111111111111111111111111112": 30000
}'
```

When the projected tip is below the resolved floor, your oracle issues
`ConfirmOracle` rejection (`resolution_state = 2`, `resolution_reason
= 200` / `TIP_BELOW_OPERATOR_FLOOR`). Cost is one Solana tx; the
rejection is observable to reputation indexers.

### Resolution order

1. `ORACLE_MIN_VERDICT_TIP_BY_MINT_RAW[<mint>]` if present.
2. `ORACLE_MIN_VERDICT_TIP_DEFAULT_RAW` if set.
3. USDC convention (`5000` raw) for USDC mints.
4. `0` (accept) for unrecognized mints.

The daemon never consults a price feed. Maintain stable USD-equivalent
floors via a sidecar cron writing into the parameters table.

## FAQ

**How do I get paid?** Via `oracle_fee_bps` (per-escrow, recommended
default 100 bps). The tip is deducted on **both** `ReleasePayment` AND
`RefundPayment`, as long as `resolution_state != 0`. You earn for
doing your job regardless of verdict. If `resolution_state == 0` (no
verdict — e.g. expired without action), no tip is paid.

**Can I run multiple profiles in one binary?** No (per spec §5.1
§7.3). One profile per binary; one keypair per profile. Multi-profile
deployments run multiple binaries.

**Who pays ConfirmOracle gas?** Your oracle keypair. You earn back via
`oracle_fee_bps`.

**What if evaluation needs external APIs (e.g. calling the seller's
endpoint)?** Use `ctx.http` (the shared reqwest client). Keep timeouts
tight (`evaluation_timeout_ms` defaults to 30s). On failure return
`Err(OracleError::Evaluation(...))`; Active Guardian retries.

**What if I disagree with a buyer's SLA terms?** You don't have to
serve every SLA. Wrong `profile_id` is refused at dispatch. Unreasonable
domain fields can be rejected with a specific reason code documented in
your `NORMATIVE.md`.

**How do sellers discover my oracle?** Through pr402's `/capabilities`
endpoint listing `slaEscrowOracleProfiles[]`. Each entry includes your
`profileId`, `operatorPubkey`, and a link to your normative spec.
