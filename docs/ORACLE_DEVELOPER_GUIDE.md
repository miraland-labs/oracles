# Oracle Developer Guide

> Build your own oracle family for the x402 SLA-Escrow ecosystem.

## Who is this for?

You are a developer with **domain expertise** (DeFi, AI inference, file
delivery, IoT attestation, gaming, etc.) who wants to build an oracle that
evaluates whether sellers fulfilled their Service Level Agreements. Your oracle
will:

1. Watch the SLA-Escrow program for `SubmitDelivery` events.
2. Fetch the SLA document and delivery evidence from the registry.
3. Evaluate whether the evidence satisfies the SLA rules you define.
4. Post a binding verdict (`ConfirmOracle`) on-chain.

The pr402 ecosystem provides the infrastructure (chain monitor, registry,
settler, HTTP server). You provide the **evaluation logic** and the
**normative specification** that defines what "fulfilled" means in your domain.

---

## The Big Picture: SLA-Escrow Payment Lifecycle

Before diving into oracle development, understand the full lifecycle your
oracle participates in. Four actors collaborate:

| Actor | Role |
|-------|------|
| **Buyer** | Pays USDC into escrow, authors the SLA (what they expect) |
| **Seller** | Delivers the service, uploads evidence, calls SubmitDelivery |
| **Oracle (you)** | Evaluates evidence vs SLA, posts binding verdict |
| **pr402 Facilitator** | Discovery, tx assembly, verify/settle (web2 glue) |

### End-to-End Flow (from the oracle's perspective)

```
 BUYER                    SELLER                   ORACLE (you)           CHAIN
 ═════                    ══════                   ════════════           ═════

 1. Probe seller's 402
    ← 402 envelope with
      oracleAuthorities,
      profileId, cluster

 2. Author SLA locally
    (includes payment_uid,
     buyer_nonce, your
     profile_id, domain
     fields)
    → SHA256 → sla_hash

 3. Sign FundPayment
    (sla_hash + oracle
     authority = YOU)
    ─────────────────────────────────────────────────────────────► Payment PDA
                                                                   created
                                                                   (Funded)

                          4. Deliver service
                             (off-chain work)

                          5. Upload SLA bytes
                             to YOUR registry
                             POST /v1/registry/sla

                          6. Upload evidence
                             to YOUR registry
                             POST /v1/registry/delivery

                          7. SubmitDelivery ──────────────────────► Payment.
                             (on-chain)                              delivery_hash
                                                                    delivery_timestamp

                                                   8. logsSubscribe ◄── DeliverySubmittedEvent
                                                      detects event

                                                   9. Read Payment account
                                                      Check: oracle_authority == me?
                                                      Check: delivery_timestamp != 0?
                                                      Check: resolution_state == 0?

                                                  10. Fetch SLA by sla_hash
                                                      from YOUR registry
                                                      (retry with backoff if 404)

                                                  11. Fetch Evidence by delivery_hash
                                                      from YOUR registry
                                                      (retry with backoff if 404)

                                                  12. EVALUATE
                                                      • Parse SLA (your schema)
                                                      • Parse Evidence (your schema)
                                                      • Run your domain checks
                                                      • Produce verdict + reason

                                                  13. Compute resolution_hash
                                                      (canonical envelope recipe)

                                                  14. ConfirmOracle ─────────────► Payment.
                                                      (on-chain tx)                resolution_state
                                                                                   = 1 (approve)
                                                                                   or 2 (reject)

 15. ReleasePayment (if approved) ──────────────────────────────► USDC → seller
     OR RefundPayment (if rejected) ────────────────────────────► USDC → buyer
```

### What the oracle does NOT do

- Never holds or moves funds.
- Never authors SLA or evidence.
- Never interacts with the buyer directly.
- Never extends TTL or modifies payment state (except via ConfirmOracle).

### Active Guardian behavior (your oracle does this automatically)

If the seller withholds SLA or evidence (steps 5-6), your oracle:
- **Retries** fetching with exponential backoff (10s → 20s → … → 120s cap).
- **Rejects** if artifacts remain unavailable within 10 minutes of expiry.
- This protects the buyer from a malicious seller who calls SubmitDelivery
  but never uploads proof.

This behavior is built into `oracle-common` — you get it for free.

---

## The Protocol Document

For the complete cross-actor protocol specification (all four roles, all seven
phases, wire formats, error taxonomy), see:

**[`SLA_ESCROW_PROTOCOL.md`](./SLA_ESCROW_PROTOCOL.md)**

That document is authoritative for the wire-level interaction. This guide
focuses on what YOU (the oracle developer) need to implement.

---

## Architecture at a Glance

```
┌─────────────────────────────────────────────────────────────┐
│              Your Oracle Binary                              │
│                                                             │
│  ┌────────────────────────────────────────────────────┐     │
│  │  oracle-common (provided — you don't modify this)  │     │
│  │  • Chain monitor (logsSubscribe + backfill)        │     │
│  │  • Worker (retry + Active Guardian)                │     │
│  │  • Settler (ConfirmOracle tx builder)              │     │
│  │  • HTTP server (/health, /v1/registry/*)           │     │
│  │  • Registry (SLA + evidence storage)               │     │
│  │  • Fetcher (content-addressed retrieval + verify)  │     │
│  └────────────────────────────────────────────────────┘     │
│                                                             │
│  ┌────────────────────────────────────────────────────┐     │
│  │  YOUR CODE (~200-500 lines)                        │     │
│  │  • SLA struct (Deserialize + Serialize)            │     │
│  │  • Evidence struct (Deserialize + Serialize)       │     │
│  │  • impl OracleEvaluator (your domain logic)        │     │
│  │  • main.rs (wire evaluator + fetchers + registry)  │     │
│  │  • NORMATIVE.md (your published rules)             │     │
│  └────────────────────────────────────────────────────┘     │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

You write ~3-4 files. `oracle-common` handles everything else.

### Before you start

**Prerequisites:**
- Rust toolchain (stable, 1.75+)
- Familiarity with Solana concepts (PDAs, transactions, RPC)
- A domain where you can define "what does fulfilled mean?" as a set of
  deterministic, verifiable checks
- A Solana keypair funded with SOL (for signing ConfirmOracle txs)

**Read first:**
- [`SLA_ESCROW_PROTOCOL.md`](./SLA_ESCROW_PROTOCOL.md) — the full cross-actor protocol
- [`ARCHITECTURE.md`](./ARCHITECTURE.md) — oracle internals + Active Guardian design
- One existing normative spec (e.g. `oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md`)

---

## Step 1: Define Your Profile

A **profile** is a versioned rule family identified by a canonical string:

```
x402/oracles/<your-family>/<version>
```

Examples:
- `x402/oracles/onchain-transfer/v1` — SPL token transfer verification
- `x402/oracles/api-quality/v1` — HTTP API response quality
- `x402/oracles/file-delivery/attestation/v1` — File/blob delivery
- `x402/oracles/gpu-inference/v1` — (your new family!)

Rules:
- Lowercase, slash-separated, no spaces.
- Version is a single integer (`v1`, `v2`, …). Bump on breaking schema changes.
- Once published, a profile id is **immutable** — never reuse an id with different semantics.

---

## Step 2: Define Your SLA Schema

The SLA is a JSON document the **buyer** authors before signing `FundPayment`.
It describes what the buyer expects the seller to deliver. Your oracle parses
this document and uses it as the "contract" to evaluate against.

### Required envelope fields (all families)

```json
{
  "version": 1,
  "profile_id": "x402/oracles/<your-family>/v1",
  "payment_uid": "<64 lowercase hex chars>",
  "buyer_nonce": "<64 lowercase hex chars (optional but recommended)>",
  // ... your domain-specific fields below ...
}
```

### Your domain fields

Add whatever your evaluation logic needs. Examples:

**GPU Inference oracle:**
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

**IoT Attestation oracle:**
```json
{
  "version": 1,
  "profile_id": "x402/oracles/iot-attestation/v1",
  "payment_uid": "def456...",
  "device_id": "sensor-42",
  "measurement_type": "temperature",
  "min_readings": 10,
  "time_window_seconds": 3600
}
```

### Canonicalization

SLA bytes are hashed with SHA-256 to produce `sla_hash` (stored on-chain in
`Payment.sla_hash`). Both buyer and seller must produce **byte-identical** JSON.
Convention: **sorted keys, compact separators** (`json.dumps(obj, sort_keys=True, separators=(',', ':'))`).

---

## Step 3: Define Your Evidence Schema

The evidence is a JSON document the **seller** uploads after delivering the
service. It proves what was delivered.

### Required envelope fields

```json
{
  "version": 1,
  "profile_id": "x402/oracles/<your-family>/v1",
  "payment_uid": "<64 hex — must match SLA>",
  // ... your domain-specific proof fields ...
}
```

### Your domain fields

**GPU Inference:**
```json
{
  "version": 1,
  "profile_id": "x402/oracles/gpu-inference/v1",
  "payment_uid": "abc123...",
  "response_hash": "sha256-of-response-bytes",
  "latency_ms": 3200,
  "tokens_generated": 847,
  "timestamp": 1779251089
}
```

Evidence bytes are hashed with SHA-256 → `delivery_hash` (stored on-chain via
`SubmitDelivery`).

---

## Step 4: Implement `OracleEvaluator`

This is the core trait you implement:

```rust
use oracle_common::{
    evaluator::{EvaluationContext, OracleEvaluator},
    error::OracleError,
    types::{EvaluationResult, CheckResult, EvidenceKey},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// Your SLA type (matches your schema)
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

// Your Evidence type
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

        // Check 1: profile_id matches
        let profile_ok = sla.profile_id == self.profile_id();
        checks.push(CheckResult {
            name: "profile_id".into(),
            passed: profile_ok,
            detail: format!("expected={}, got={}", self.profile_id(), sla.profile_id),
        });
        all_pass &= profile_ok;

        // Check 2: payment_uid matches
        let uid_ok = sla.payment_uid == evidence.payment_uid;
        checks.push(CheckResult {
            name: "payment_uid_match".into(),
            passed: uid_ok,
            detail: "SLA and evidence payment_uid must match".into(),
        });
        all_pass &= uid_ok;

        // Check 3: latency within SLA bounds
        let latency_ok = evidence.latency_ms <= sla.max_latency_ms;
        checks.push(CheckResult {
            name: "latency".into(),
            passed: latency_ok,
            detail: format!(
                "actual={}ms, max={}ms",
                evidence.latency_ms, sla.max_latency_ms
            ),
        });
        all_pass &= latency_ok;

        // Check 4: minimum tokens generated
        let tokens_ok = evidence.tokens_generated >= sla.min_tokens;
        checks.push(CheckResult {
            name: "min_tokens".into(),
            passed: tokens_ok,
            detail: format!(
                "actual={}, min={}",
                evidence.tokens_generated, sla.min_tokens
            ),
        });
        all_pass &= tokens_ok;

        // Check 5: evidence timestamp freshness
        let fresh_ok = ctx.job.created_at == 0
            || evidence.timestamp >= ctx.job.created_at;
        checks.push(CheckResult {
            name: "freshness".into(),
            passed: fresh_ok,
            detail: "evidence.timestamp >= payment.created_at".into(),
        });
        all_pass &= fresh_ok;

        Ok(EvaluationResult {
            approved: all_pass,
            resolution_reason: if all_pass { 0 } else { 1 },
            checks,
        })
    }
}
```

### Key rules for `evaluate()`

1. **Deterministic**: same inputs → same output. No wall-clock reads, no randomness.
2. **Fail-closed**: if in doubt, reject. Buyer protection is the default.
3. **Check `payment_uid`**: SLA's `payment_uid` must equal evidence's. Prevents cross-payment replay.
4. **Check `profile_id`**: SLA must declare YOUR profile. Prevents mis-routing.
5. **Check freshness**: `evidence.timestamp >= payment.created_at` defeats pre-funding evidence replay.
6. **Return `CheckResult` list**: each check is logged and surfaced in the resolution hash for auditability.

---

## Step 5: Wire It Up (main.rs)

```rust
use std::sync::Arc;
use oracle_common::{
    fetcher::RegistryJsonFetcher,
    profile::{ProfileBinding, ProfileRegistry, RegisteredProfile},
};

// In your main():
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

The rest (chain monitor, worker, settler, HTTP server) is provided by
`oracle-common`. Copy the structure from any existing family binary
(`oracle-onchain-transfer/src/main.rs` is the simplest reference).

---

## Step 6: Write Your Normative Spec

Create `your-oracle/spec/<family>-v1/NORMATIVE.md`. This document is the
**public contract** between your oracle and sellers who integrate with it.
It must define:

1. **SLA schema** — every field, its type, whether required/optional, and semantics.
2. **Evidence schema** — same.
3. **Evaluation rules** — numbered, unambiguous checks the oracle performs.
4. **Resolution reason codes** — what each code means for dispute resolution.
5. **Versioning policy** — when you bump the version, what breaks.

See existing specs for format:
- `oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md`
- `oracle-api-quality/spec/api-quality-v1/NORMATIVE.md`
- `oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md`

---

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
|----------|----------|---------|-------------|
| `SOLANA_RPC_URL` | no | `https://api.devnet.solana.com` | HTTP RPC for the cluster you serve |
| `SOLANA_WS_URL` | no | `wss://api.devnet.solana.com` | WebSocket for logsSubscribe |
| `ESCROW_PROGRAM_ID` | no | `sla_escrow_api::ID` | SLA-Escrow program to monitor |
| `ORACLE_KEYPAIR_PATH` | yes | - | Ed25519 keypair path (signs ConfirmOracle) |
| `BIND_ADDR` | no | `127.0.0.1:4020` | Default HTTP server bind address |
| `DATABASE_URL` | no | - | Postgres URL for ledger + dead-letter (recommended) |
| `ORACLE_REGISTRY_BACKEND` | yes | - | Storage backend: `postgres`, `s3`, or `local` |
| `EVIDENCE_REGISTRY_URLS` | no | `http://localhost:4021` | Comma-separated mirror URLs for fallback fetching |
| `EVIDENCE_REGISTRY_URL` | no | `http://localhost:4021` | Single fallback fetch URL (used if `EVIDENCE_REGISTRY_URLS` is empty) |
| `EVIDENCE_REGISTRY_AUTH_HEADER` | no | - | `Authorization` header for GET fetch requests |
| `ORACLE_REGISTRY_MAX_BYTEA_BYTES` | no | `4MB` | Max size of inline documents (SLA/Evidence JSON) |
| `ORACLE_REGISTRY_MAX_BLOB_BYTES` | no | `5GB` | Max size of streamed blobs |
| `ORACLE_RETRY_INITIAL_DELAY_SEC` | no | `10` | First retry delay (seconds) |
| `ORACLE_RETRY_MAX_DELAY_SEC` | no | `120` | Maximum retry backoff cap (seconds) |
| `ORACLE_MAX_RETRY_ATTEMPTS` | no | `30` | Max retries before protective reject |
| `ORACLE_REJECT_SAFETY_MARGIN_SEC` | no | `600` | Safety margin before expiry to issue reject |

**S3 Backend Configuration (Only required when `ORACLE_REGISTRY_BACKEND=s3`):**

| Variable | Required | Description |
|----------|----------|-------------|
| `ORACLE_REGISTRY_S3_ENDPOINT` | yes | S3 endpoint URL |
| `ORACLE_REGISTRY_S3_BUCKET` | yes | Target S3 bucket name |
| `ORACLE_REGISTRY_S3_ACCESS_KEY` | yes | S3 access key |
| `ORACLE_REGISTRY_S3_SECRET_KEY` | yes | S3 secret key |
| `ORACLE_REGISTRY_S3_REGION` | no (defaults to `us-east-1`) | S3 region |

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

---

## Step 8: Get Listed on pr402

Once your oracle is running and healthy:

1. **Register your oracle pubkey** with the pr402 operator (add to
   `PR402_ORACLE_AUTHORITIES` in the facilitator's parameters table).
2. **Publish your normative spec** at a stable URL (GitHub recommended).
3. **Advertise your profile** — pr402's `/capabilities` will list your
   `profileId` + `operatorPubkey` under `slaEscrowOracleProfiles[]`.

Sellers can then reference your oracle in their 402 envelopes:
```json
{
  "oracleAuthorities": ["<your-oracle-pubkey>"],
  "profileId": "x402/oracles/gpu-inference/v1"
}
```

---

## How Sellers Integrate With Your Oracle

As an oracle developer, you are responsible for making it easy for sellers to
integrate. Here's what you must provide:

### What sellers need from you

| Artifact | Where | Purpose |
|----------|-------|---------|
| **NORMATIVE.md** | Published URL (GitHub) | Sellers read this to know what SLA fields to support and what evidence to produce |
| **Registry endpoint** | Your deployed oracle's `/v1/registry/*` | Sellers upload SLA + evidence here |
| **Oracle pubkey** | Your keypair's public key | Sellers include this in their 402 envelope's `oracleAuthorities[]` |
| **Profile ID** | e.g. `x402/oracles/gpu-inference/v1` | Sellers include this in their 402 envelope's `profileId` |
| **SLA JSON Schema** | In your NORMATIVE.md or as a separate `.json` | Sellers validate their SLA construction against this |
| **Evidence JSON Schema** | Same | Sellers validate their evidence before upload |

### Seller's integration steps (what you document for them)

1. **Register with your registry** — `POST /v1/registry/seller/register` to get
   a bearer token for uploads. One-time setup.
2. **Emit the 402 envelope** — include your `profileId` and your pubkey in
   `oracleAuthorities[]` so buyers know which oracle will evaluate.
3. **On paid GET (after buyer pays)**:
   - Reconstruct the canonical SLA from request params + extracted `payment_uid`.
   - Upload SLA bytes: `POST /v1/registry/sla` (your registry).
   - Deliver the service (your domain work).
   - Build evidence JSON conforming to your schema.
   - Upload evidence: `POST /v1/registry/delivery` (your registry).
   - Call `SubmitDelivery` on-chain with `delivery_hash = SHA256(evidence_bytes)`.
4. **Wait for your verdict** — your oracle evaluates and posts ConfirmOracle.
5. **Call ReleasePayment** (if approved) to collect USDC.

### What happens if the seller misbehaves

| Seller behavior | Your oracle's response |
|-----------------|----------------------|
| Uploads valid SLA + evidence, delivers honestly | Evaluate fairly → APPROVE |
| Uploads evidence that doesn't satisfy SLA | Evaluate → REJECT (buyer refunds) |
| Calls SubmitDelivery but never uploads SLA/evidence | Active Guardian retries → REJECT before expiry |
| Never calls SubmitDelivery | Oracle never triggered; `delivery_timestamp=0` blocks release; buyer refunds after expiry |

---

## The Stable Interface (What Won't Change)

These are the **fixed contracts** between your oracle and the rest of the
ecosystem. Code against these confidently:

### On-chain (sla-escrow program)
- `SubmitDelivery` instruction: `[seller, bank, config, escrow, payment]` + `delivery_hash[32]`
- `ConfirmOracle` instruction: `[oracle, bank, config, escrow, payment]` + `delivery_hash[32] + resolution_hash[32] + reason[2] + state[1]`
- `Payment` account layout: `payment_uid[32]`, `sla_hash[32]`, `delivery_hash[32]`, `oracle_authority[32]`, `expires_at[i64]`, `resolution_state[u8]`, etc.
- PDA seeds: `[b"payment", uid_bytes, bank]`, `[b"escrow", mint, bank]`

### Off-chain (oracle-common)
- `OracleEvaluator` trait: `type Sla`, `type Evidence`, `fn profile_id()`, `async fn evaluate()`
- `EvaluationResult`: `approved: bool`, `resolution_reason: u16`, `checks: Vec<CheckResult>`
- Resolution hash recipe: `x402/oracles/resolution-envelope/v1` (fixed key order, SHA-256)
- Registry HTTP API: `POST /v1/registry/sla`, `POST /v1/registry/delivery`, `GET /v1/registry/<hash>`

### Wire conventions
- SLA `profile_id` field: exact-match dispatch (no aliases, no prefix match)
- `payment_uid`: 64 lowercase hex chars (32 raw bytes from on-chain `Payment.payment_uid`)
- Canonical JSON: sorted keys, compact separators, UTF-8

---

## Reference Implementations

| Family | Crate | Profile ID | Complexity |
|--------|-------|-----------|------------|
| On-chain Transfer | `oracle-onchain-transfer` | `x402/oracles/onchain-transfer/v1` | Medium (RPC balance checks) |
| API Quality | `oracle-api-quality` | `x402/oracles/api-quality/v1` | High (HTTP probing + TLS) |
| File Delivery | `oracle-file-delivery` | `x402/oracles/file-delivery/attestation/v1` | Low (hash + size check) |

Start by reading `oracle-onchain-transfer` — it's the most straightforward
evaluator (~200 lines of domain logic).

---

## Operator Economics: Recommended `oracle_fee_bps`

The on-chain program enforces only an upper bound (`MAX_ORACLE_FEE_BPS = 500`,
i.e. 5%). It has no built-in floor. Operators and Facilitators are responsible
for picking a tip that pays for the cost of issuing `ConfirmOracle` and any
external evaluation work.

### Cost breakdown per verdict (Solana mainnet, ~$150/SOL)

| Cost line | Typical |
|-----------|---------|
| Solana base fee (5000 lamports) | ~$0.00075 |
| Priority fee (congested periods) | $0.003–$0.015 |
| RPC `getTransaction` / `getAccountInfo` | $0.000–$0.005 (depending on provider) |
| Active Guardian retries (worst case ~30 fetches) | up to $0.030 in RPC budget |

A single happy-path verdict costs roughly $0.005–$0.020 to land on-chain.
Anything below that is a net loss for the operator and signals that the rate
is too low.

### Tiering convention (recommended defaults)

The pr402 reference Facilitator opens canonical Escrows with a default of
**100 bps (1%)** for stable-mint payments. This is the recommended starting
point for any new Facilitator or operator-managed Escrow.

| Escrow size (USDC equivalent) | Suggested `oracle_fee_bps` | Tip on a typical payment |
|------------------------------|----------------------------|--------------------------|
| < $5 (micro-payments)         | 200–300 (2–3%)             | $0.02–$0.06              |
| $5 – $50 (default lane)       | **100 (1%)**               | $0.05–$0.50              |
| $50 – $500                    | 50 (0.5%)                  | $0.25–$2.50              |
| > $500 (large escrows)        | 25 (0.25%)                 | $1.25+                   |

50 bps as a single global default is **not** recommended — it does not cover
priority fees during congestion for sub-$5 payments. Bump to 100 bps as the
neutral default and reserve 50 bps for high-value lanes where the absolute tip
is already large.

### Operator self-protection

If you operate an oracle and want to refuse underpriced jobs without waiting
for a redesign of the on-chain program, set:

```bash
ORACLE_MIN_VERDICT_TIP_LAMPORTS=20000  # ~$0.003 at $150/SOL
```

When the projected tip on a pending payment is below this floor, your oracle
declines to subscribe. The Active Guardian will eventually issue a fail-closed
REJECT before expiry, the buyer is refunded, and the seller learns to fund
escrows with a competitive `oracle_fee_bps`. Market discovery without a
program upgrade.

### Why not put a floor on-chain?

Adding `oracle_min_fee_amount` to `Escrow` and `Payment` would be a Pod-layout
change — it shifts byte offsets and invalidates all existing accounts. For now,
the off-chain self-protection knob plus the tiering convention above achieves
the same outcome without forcing a migration of the live program.

---

## FAQ

**Q: How do I get paid as an oracle operator?**
A: Via `oracle_fee_bps` — a per-escrow setting (recommended default 100 bps =
1% of payment amount; see the tiering table above). The oracle tip is deducted
from the escrowed amount on **both** `ReleasePayment` AND `RefundPayment` — as
long as `resolution_state != 0` (i.e., you issued a verdict). This means you
earn for doing your job regardless of whether you approved or rejected. The
tip goes to your oracle pubkey's token account (USDC ATA for SPL payments, or
direct lamports for SOL payments). If `resolution_state == 0` (you never
issued a verdict — e.g., payment expired without oracle action), no tip is
paid.

**Q: Can I run multiple profiles in one binary?**
A: The architecture supports it (register multiple profiles in the registry),
but v1 convention is one profile per binary for operational isolation.

**Q: Who pays for ConfirmOracle gas?**
A: Your oracle keypair pays (~5000 lamports per tx, plus priority fees during
congestion). You earn back via `oracle_fee_bps` (configured per-escrow;
recommended default 100 bps for the $5–$50 lane — see the tiering table above).

**Q: What if my evaluation needs external APIs (e.g. calling the seller's endpoint)?**
A: Use `ctx.http` (the shared reqwest client). Keep timeouts tight
(`evaluation_timeout_ms` defaults to 30s). If the external call fails, return
`Err(OracleError::Evaluation(...))` — the Active Guardian will retry.

**Q: What if I disagree with the buyer's SLA terms?**
A: You don't have to serve every SLA. If `sla.profile_id != self.profile_id()`,
the pipeline refuses at dispatch. If the SLA's domain fields are unreasonable
(e.g. `max_latency_ms: 1`), your evaluator can reject with a specific reason
code and document that in your normative spec.

**Q: How do sellers discover my oracle?**
A: Through pr402's `/capabilities` endpoint which lists
`slaEscrowOracleProfiles[]`. Each entry includes your `profileId`,
`operatorPubkey`, and a link to your normative spec.
