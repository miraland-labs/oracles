# Oracles: The Trust Layer for Machine Commerce on Solana

**Reading time:** ~3 minutes.
**Audience:** developers, domain experts, and operators considering becoming
an x402 oracle provider.

---

When two AI agents pay each other for instant work — a single inference, an
API call, a data lookup — Solana settles in under two seconds. The x402
**`exact`** rail handles that perfectly.

But machine commerce isn't always instant. An agent that asks for a
trained model, a generated video, or a verified on-chain transfer is
**owed work that takes time**. Funds need to be locked while the seller
works, and released only when delivery is confirmed.

That's the **`sla-escrow`** rail — and confirming delivery is what
**oracles** do.

## What an oracle does in one sentence

An oracle is a small standalone service that watches the
[`sla-escrow`](https://github.com/miraland-labs/sla-escrow) program for
delivery events, fetches the seller's hash-bound evidence from a registry,
runs profile-specific checks against the buyer's SLA, and submits an
on-chain verdict that releases the buyer's funds — or refunds them.

The chain stays minimal. All the domain logic — what counts as a "good"
JSON response, a "valid" token transfer, a "correct" video file — lives
**off-chain**, in the oracle.

## Three reference oracles, today

The [`oracles`](https://github.com/miraland-labs/oracles) workspace ships
three sibling binaries built on a shared `oracle-common` library:

| Profile                                             | What it adjudicates                                            |
| --------------------------------------------------- | -------------------------------------------------------------- |
| `x402/oracles/api-quality/v1`                       | JSON HTTP API responses (status, latency, schema, fields)      |
| `x402/oracles/onchain-transfer/v1`                  | SPL token transfers / swaps re-derived from `getTransaction`   |
| `x402/oracles/file-delivery/attestation/v1`         | Streamed large files (size, MIME, SHA-256 attestation)         |

Each binary is independently deployable, holds its own keypair, and
serves exactly one profile. A bug in one cannot regress another.

## How the process works

Five steps, identical for every profile:

1. **Watch** — the oracle subscribes to the chain over WebSocket and waits for `DeliverySubmittedEvent`.
2. **Fetch** — it retrieves the SLA and delivery bytes from a content-addressed registry; every fetch verifies SHA-256 *before* parsing.
3. **Evaluate** — the profile-specific check battery runs deterministically against the parsed inputs.
4. **Settle** — the oracle submits `ConfirmOracle` on-chain with `approved` (`true` / `false`), a numeric `resolution_reason`, and a deterministic `resolution_hash` that any third party can recompute to verify the verdict.
5. **Get paid** — the on-chain program pays the oracle a **verdict-neutral tip** (default 50 bps) regardless of whether it approves or rejects. Oracles are paid for **adjudicating**, not for outcomes — that's the incentive design that keeps them honest.

## What "oracle" feels like to build

We chose three constraints up front, and they shape the whole design:

- **One profile = one binary**. No multi-tenant evaluator pools to debug. If your domain logic explodes, only your binary is affected.
- **No on-chain code changes for new oracles**. Add a new family by writing one Rust trait impl, registering one profile id, and starting the binary. No program upgrade.
- **Deterministic verdicts**. Same SLA + same delivery = same `resolution_hash` across every oracle that runs the profile. Verifiable by anyone holding the bytes.

The shared `oracle-common` library handles the boring parts: chain
monitor, registration HTTP API, three storage backends (Postgres / S3 /
local filesystem), Postgres ledger, retry logic, manual evaluation
endpoint, Prometheus metrics, systemd installer, MinIO bootstrap. **You
write the evaluator. We wrote everything else.**

## How to become an oracle developer

Five steps, end-to-end:

1. **Pick your domain.** ML model accuracy. Uptime monitoring. Content moderation. Financial data integrity. Anything where "did the seller deliver what they promised" is a tractable computation.
2. **Clone the closest sibling.** Three reference crates in [`oracles`](https://github.com/miraland-labs/oracles): `oracle-api-quality`, `oracle-onchain-transfer`, `oracle-file-delivery`. Pick the one whose evidence shape resembles yours.
3. **Implement `OracleEvaluator`.** Two methods: a `profile_id()` returning your stable id (e.g. `x402/oracles/<domain>/v1`), and `evaluate(ctx, sla, evidence)` returning approve / reject + a numeric resolution reason. Your domain expertise goes here — and only here.
4. **Register your profile** at startup, configure your DB and registry, deploy with `oracles/scripts/install.sh <family> <binary> <env-file>` on Ubuntu 24.04.
5. **Get advertised**. After a brief facilitator-side onboarding, pr402 lists your oracle on `GET /capabilities` for sellers and buyers to discover. Sellers reference you in their HTTP-402 challenge. Buyers pick you. You earn tips on every verdict.

The full bring-up runbook is in [`oracles/docs/DEPLOYMENT.md`](../DEPLOYMENT.md).
Day-2 operations — monitoring, rotations, backup, incident playbooks —
are in [`oracles/docs/OPERATIONS.md`](../OPERATIONS.md).

## How a seller submits a delivery to an oracle

For sellers integrating with an oracle that's already running, the loop
is **three HTTP calls plus one on-chain instruction**:

```bash
# 1. Upload the SLA you'll be evaluated against.
curl -X POST $ORACLE/v1/registry/sla \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json

# 2. Upload the delivery evidence (or stream the blob).
curl -X POST $ORACLE/v1/registry/delivery \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @delivery.json

# 3. Submit the on-chain delivery. The oracle takes it from here.
sla-escrow submit-delivery \
    --seller ./seller-keypair.json \
    --payment-uid "$PAYMENT_UID" \
    --delivery-hash "$DELIVERY_HASH"
```

The seller never has to understand the oracle's evaluator. They just hand
over hash-bound bytes that match the SLA they advertised. Three copy-paste
recipes (one per category) live in
[`oracles/docs/SELLER_GUIDE.md`](../SELLER_GUIDE.md).

## Why this matters

Oracles are the only piece of x402 that has **no dominant single
implementation**. The `exact` rail has UniversalSettle. The `sla-escrow`
program is one program. But oracles are intentionally plural — every
domain wants its own.

That's the recruitment ask: **bring your domain expertise; the rails are
built.** A small team can ship a production oracle in a week if their
evaluator logic is well-understood. Each verdict earns a tip. Each
profile id we publish is one more category of asynchronous work that
machine commerce can now settle.

If you can answer "did the seller deliver what they promised" as a
deterministic function, you can ship an oracle.

---

## Start here

- **Repo:** [`miraland-labs/oracles`](https://github.com/miraland-labs/oracles)
- **Quick-start:** any of the three `oracle-*/README.md`
- **Seller integration:** [`oracles/docs/SELLER_GUIDE.md`](../SELLER_GUIDE.md)
- **Buyer integration:** [`oracles/docs/BUYER_GUIDE.md`](../BUYER_GUIDE.md)
- **Architectural source of truth:**
  [`.kiro/specs/multi-category-oracle-architecture/design.md`](https://github.com/miraland-labs/x402/blob/main/.kiro/specs/multi-category-oracle-architecture/design.md)

The pr402 facilitator on `https://ipay.sh` (Mainnet) and
`https://preview.ipay.sh` (Devnet) advertises live oracles via `GET
/api/v1/facilitator/capabilities`. Watch that endpoint to see the network
grow — and to know when yours has joined it.
