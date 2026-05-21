# SLA-Escrow Protocol — Cross-Actor Reference

**Protocol version**: 1.0
**Audience**: integrators building any of the four roles: **buyer agent**,
**seller**, **oracle operator**, **pr402 facilitator operator**.

This document describes the off-chain + on-chain interaction pattern for the
`sla-escrow` rail. Each role-specific guide
([`SELLER_GUIDE.md`](./SELLER_GUIDE.md), [`BUYER_GUIDE.md`](./BUYER_GUIDE.md),
[`DEPLOYMENT.md`](./DEPLOYMENT.md)) tells *one* role what *they* do; this
document tells everyone what *all four* roles do **together**, so a new
integrator can read one page and understand the whole protocol before
shipping code.

If something here disagrees with a role-specific guide, this document is
authoritative for the wire-level interaction. Per-role guides may add
implementation tips on top.

---

## 1. Overview

Four actors collaborate to settle one SLA-Escrow payment for one piece of work:

```
                     ┌──────────────────────┐
                     │   pr402 facilitator  │
                     │   (HTTP, web2)       │
                     └──────────┬───────────┘
                                │ build-sla-escrow-payment-tx
                                │ /verify  /settle  /capabilities
                                │
            ┌────────────┐      │      ┌────────────┐
            │   Buyer    │◀─────┴─────▶│   Seller   │
            │   (agent)  │  HTTP-402 + │  (service) │
            └─────┬──────┘  out-of-band └─────┬─────┘
                  │  SLA bytes               │  SLA upload, evidence,
                  │                           │  delivery hash
                  ▼                           ▼
            ┌──────────────────────────────────────┐
            │  Oracle (HTTP registry + evaluator)  │
            │  /v1/registry/{sla|delivery|blob}    │
            │  /health, /evaluate, /metrics        │
            └────────────────┬─────────────────────┘
                             │ ConfirmOracle
                             ▼
            ┌──────────────────────────────────────┐
            │  sla-escrow program (Solana on-chain)│
            │  Payment PDA holds the truth         │
            └──────────────────────────────────────┘
```

Three artifacts move on-chain:
1. **`Payment.sla_hash`** — `SHA256(sla_bytes)`, committed when the buyer
   signs `FundPayment`.
2. **`Payment.delivery_hash`** — `SHA256(evidence_bytes)`, committed when
   the seller signs `SubmitDelivery`.
3. **`Payment.resolution_hash`** + `resolution_reason` + `resolution_state` —
   committed when the oracle signs `ConfirmOracle`.

Two artifacts live off-chain in the oracle's content-addressed registry:
1. **The SLA bytes** keyed by `sla_hash`.
2. **The delivery evidence** (JSON or blob) keyed by `delivery_hash`.

The on-chain hashes anchor the off-chain bytes. The registry re-hashes on
read, so any party can independently fetch and verify.

---

## 2. Actors and responsibilities

| Role | Owns | Never does |
|---|---|---|
| **Buyer agent** | Authoring SLA bytes (including `payment_uid` and `buyer_nonce`). Signing `FundPayment`. Reading the verdict. | Never uploads SLA to the registry directly (no bearer). Never authors evidence. Never confirms oracle. |
| **Seller** | Uploading SLA + delivery evidence to the registry (HMAC-bearer). Producing the deliverable. Signing `SubmitDelivery`. | Never authors the SLA (the buyer does). Never invents `payment_uid` or `buyer_nonce`. Never confirms oracle. |
| **Oracle operator** | Running a binary that watches the chain, fetches SLA + evidence, evaluates per its profile, signs `ConfirmOracle`. Hosts the registry endpoint. | Never holds buyer or seller funds. Never authors SLA or evidence. Never extends TTL. |
| **pr402 facilitator** | Discovery (`/capabilities`), tx assembly (`build-sla-escrow-payment-tx`), x402 `/verify` + `/settle`. Optional health gate. | Never sees SLA bytes (only the hash). Never sees evidence bytes. Never signs `FundPayment`, `SubmitDelivery`, or `ConfirmOracle`. |

The buyer is the only role that holds escrowed funds (via the on-chain
escrow PDA). The seller and oracle never custody funds; they only observe
on-chain state and submit instructions.

---

## 3. The seven phases

Each phase lists: **inputs**, **action**, **outputs**, **state changes**.

### Phase 1 — Discovery

**Goal**: buyer learns which oracles a seller trusts, and which oracles
a facilitator advertises.

**Buyer**:
1. Calls the seller's API; gets HTTP 402 with `accepts[]` listing one or
   more `scheme: "v2:solana:sla-escrow"` lines.
2. Reads `accepts[].extra.oracleProfiles[]` for `(profileId,
   operatorPubkey, registryUrl, normativeSpecUrl)` per oracle the seller
   trusts.
3. Optionally hits `GET <pr402>/api/v1/facilitator/capabilities` →
   `slaEscrowOracleProfiles[]` to cross-check the seller's claims and to
   read any default operator the deployment recommends.
4. Picks one `(profileId, operatorPubkey)` pair. The buyer is free to
   pick the seller's default or any other listed entry; the buyer is the
   one paying.

**Output**: a chosen `(profileId, oracleAuthority, registryUrl)` triple.

**No on-chain or registry state changes yet.**

### Phase 2 — SLA authorship (buyer-side)

**Goal**: the buyer produces the exact bytes that will be hashed into
`Payment.sla_hash`.

**Buyer**:
1. Asks pr402 for a `payment_uid` (one of three options):
   - Pre-generate locally: `openssl rand -hex 32` → 64 hex chars.
   - Let pr402 generate: omit `paymentUid` in the build request; pr402
     returns one in the response (this requires authoring the SLA *after*
     the build, hashing, and re-calling build with the hash — an extra
     round-trip).
   - Pre-generate via pr402's helper: pass `paymentUid` you choose.
   - **Recommended**: pre-generate locally (option 1) so the SLA is fully
     authored before any HTTP call to pr402.
2. Generates `buyer_nonce`: `openssl rand -hex 32` (optional but
   recommended; defends against cross-SLA replay when SLA terms are
   identical across buyers).
3. Authors the SLA bytes. The shape is per-profile (see §4); every
   profile requires `version`, `profile_id`, `payment_uid`. Optional
   `buyer_nonce` is universal across profiles.
4. Computes `sla_hash = SHA256(sla_bytes)` locally. **Compute over the
   exact bytes you will send to the seller** — don't re-serialize.

**Output**: a sealed `sla.json` byte sequence + its `sla_hash`.

**No on-chain or registry state changes yet.**

### Phase 3 — SLA upload (seller-mediated)

**Goal**: the buyer-authored SLA bytes land in the oracle's
content-addressed registry so anyone can fetch them later.

**Buyer**: hands `sla.json` to the seller via any out-of-band channel
(HTTP request body, email, IM). The bytes are not secret — they describe
public terms — but the seller is the one with the registry's bearer
token.

**Seller**: uploads to the oracle's registry:

```bash
SLA_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/sla" \
    -H "Authorization: Bearer $SELLER_TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq -r .sha256)
```

**Output**: the registry stores `sla.json` keyed by its SHA-256 in
`oracle_artifacts` (postgres backend) or in S3/MinIO (blob backend), and
returns the hash + a `/v1/registry/<hash>` URL.

**Buyer verifies**: the returned `sha256` MUST equal the buyer's local
`sla_hash`. If not, abort — the seller uploaded different bytes.

**Off-chain state change**: one new row in `oracle_deliveries` with
`kind = 'sla'`, plus the artifact bytes in `oracle_artifacts` (or S3/MinIO).
**No on-chain state change yet.**

### Phase 4 — FundPayment (buyer-side)

**Goal**: the buyer locks tokens into escrow, committing to `sla_hash`
and binding the payment to the chosen oracle.

**Buyer**: calls pr402:

```bash
BUILD_BODY=$(jq -n \
  --arg payer "$BUYER_PUBKEY" \
  --argjson accepted "$ACCEPTED" \
  --argjson resource "$RESOURCE" \
  --arg slaHash "$SLA_HASH" \
  --arg oracleAuthority "$ORACLE_AUTHORITY" \
  --arg paymentUid "$PAYMENT_UID" \
  '{payer:$payer, accepted:$accepted, resource:$resource,
    slaHash:$slaHash, oracleAuthority:$oracleAuthority,
    paymentUid:$paymentUid}')

UNSIGNED=$(curl -fsS -X POST "$PR402/api/v1/facilitator/build-sla-escrow-payment-tx" \
    -H "Content-Type: application/json" \
    -d "$BUILD_BODY")
```

**pr402** returns an unsigned `VersionedTransaction` containing one
`FundPayment` instruction with `(payment_uid, sla_hash, mint, amount,
oracle_authority, ttl_seconds, …)` plus a pre-filled `verifyBodyTemplate`
for the next phase.

**Buyer signs and submits** the transaction.

**On-chain state change**: a new `Payment` PDA is created with:
- `payment_uid`, `sla_hash`, `oracle_authority`, `amount`, `mint`
  (committed verbatim from the instruction).
- `created_at` = current block timestamp.
- `expires_at` = `created_at + ttl_seconds`.
- `state = 0` (Funded), `resolution_state = 0` (Pending).
- `delivery_hash = [0u8; 32]`, `resolution_hash = [0u8; 32]` (set in
  later phases).

The buyer's tokens move to the escrow ATA derived from `(escrow_pda,
mint)`.

### Phase 5 — Work + delivery upload (seller-side)

**Goal**: the seller produces the deliverable and uploads evidence to the
registry.

**Seller**:
1. Re-fetches the SLA bytes from the registry to read what the buyer
   committed to:
   ```bash
   SLA=$(curl -fsS "$ORACLE/v1/registry/$SLA_HASH")
   PAYMENT_UID=$(echo "$SLA" | jq -r .payment_uid)
   BUYER_NONCE=$(echo "$SLA" | jq -r '.buyer_nonce // empty')
   ```
   The registry re-hashes the bytes before serving, so you can trust them.
2. Performs the work. The shape of "the work" depends on the profile
   (call an HTTP API and capture the response, broadcast a Solana
   transfer, generate a file).
3. Authors evidence per the profile. Echo `payment_uid` (and
   `buyer_nonce` when present) verbatim. The oracle compares.
4. Uploads evidence:
   ```bash
   DELIVERY_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/delivery" \
       -H "Authorization: Bearer $SELLER_TOKEN" \
       -H "Content-Type: application/json" \
       --data-binary @delivery.json | jq -r .sha256)
   ```
   File-delivery profiles use `POST /v1/registry/blob` with the file
   bytes directly; the file *is* the evidence.

**Output**: a registry-stored evidence artifact + its `delivery_hash`.

**Off-chain state change**: one new row in `oracle_deliveries` with
`kind = 'delivery'` (or `'blob'` for file-delivery), plus the artifact
in `oracle_artifacts` or S3/MinIO. **No on-chain state change yet.**

### Phase 6 — SubmitDelivery (seller-side)

**Goal**: the seller anchors `delivery_hash` on-chain so the oracle can
detect the work is ready.

**Seller**: signs and submits a `SubmitDelivery` instruction:
```bash
sla-escrow submit-delivery \
    --seller /path/to/seller-keypair.json \
    --payment-uid "$PAYMENT_UID" \
    --delivery-hash "$DELIVERY_HASH"
```

The on-chain program enforces:
- `payment.seller == ix_signer` (only the bound seller may submit).
- `payment.state == Funded` and `resolution_state == Pending`.
- `payment.expires_at - delivery_cutoff_seconds >= clock.unix_timestamp`
  — submission must arrive at least `delivery_cutoff_seconds` before
  expiry, so the oracle has a guaranteed window to evaluate before the
  deadline.

**On-chain state change**:
- `Payment.delivery_hash = <provided>`.
- `Payment.delivery_timestamp = clock.unix_timestamp`.
- A `DeliverySubmittedEvent` is emitted in `Program data:` logs (the
  oracle subscribes to this).

### Phase 7 — Oracle adjudication

**Goal**: the oracle observes the on-chain delivery, fetches the
artifacts, evaluates them against the SLA, and signs `ConfirmOracle`.

**Oracle binary**:
1. Receives the `DeliverySubmittedEvent` via WebSocket subscription to
   `logsSubscribe` filtered on the program ID.
2. Reads the `Payment` account directly from RPC; checks
   `oracle_authority == self.pubkey` and pending state.
3. Fetches **both** artifacts from its own registry (content-addressed,
   no auth required):
   ```
   GET /v1/registry/<sla_hash>      → reads buyer_nonce from SLA
   GET /v1/registry/<delivery_hash> → reads buyer_nonce from evidence
   ```
4. Verifies `payment_uid` and `buyer_nonce` (when present) match between
   SLA and evidence and against `Payment.payment_uid`.
5. Runs the profile-specific check battery (`OracleEvaluator::evaluate`):
   freshness (`evidence ≥ Payment.created_at` so the seller can't replay
   evidence from before the buyer funded escrow), profile checks (status
   code / latency / size / mime / on-chain delta), and cross-payment
   replay protection (the same `tx_signature` or `delivery_hash` may not
   settle two different payments).
6. Computes `resolution_hash` over a canonical envelope of
   `(profile_id, payment_uid, sla_hash, delivery_hash, verdict, checks)`.
   Anyone holding the SLA + evidence bytes can recompute and verify.
7. Signs and submits `ConfirmOracle` with `(approve, resolution_reason,
   resolution_hash)`.

**On-chain state change**:
- `Payment.resolution_state = 1` (Approved) or `2` (Rejected).
- `Payment.resolution_reason = <reason u16>`.
- `Payment.resolution_hash = <32 bytes>`.
- A `PaymentOracleConfirmedEvent` is emitted.

### Phase 8 — Settlement

**Goal**: tokens move out of escrow per the verdict.

After `ConfirmOracle`:
- **Approved**: any party (buyer, seller, or pr402's `/settle`) can call
  `ReleasePayment`. Tokens flow to `Payment.seller` (the merchant
  payout wallet, which may be a SplitVault for fee sharding).
- **Rejected, before `expires_at`**: only the **buyer**, **seller**, or
  **bank authority** may call `RefundPayment`, and the buyer is gated by
  `Config.refund_cooldown_seconds` (the seller and bank authority have no
  cooldown). The cooldown's program-enforced range is `0` (disabled — not
  permitted in the live mainnet/devnet deployment) or `[3600, 604800]`
  seconds (1 hour to 7 days). The current pr402 deployment runs at
  `86400` (24 hours); operators may shorten this via `UpdateConfig` to
  as little as one hour. **Buyers should always read the live
  `Config.refund_cooldown_seconds` from chain — don't hard-code it.**
- **Expired without `ConfirmOracle`**: any of buyer/seller/bank authority
  may call `RefundPayment` after `expires_at` with no cooldown. Buyer
  reclaims.
- **Expired but oracle Rejected**: same as the rejected-pre-expiry case;
  buyer/seller/bank authority can refund without cooldown.

**Why the cooldown matters for sellers**: between FundPayment and
SubmitDelivery, only the buyer can refund (the seller hasn't bound
delivery yet). The cooldown gives sellers a guaranteed safe window to do
off-chain work without the buyer pulling the rug. Setting the cooldown
to zero would re-open the "fund-then-immediately-refund" attack and is
why the program forbids it in practice.

**Why no automatic facilitator-side refund**: pr402 deliberately does
not run an auto-sweep that calls `RefundPayment` on rejection's behalf.
That would require holding the bank authority key warm in serverless
infrastructure with too broad a permission set. Buyer self-refund (after
the cooldown) is the canonical flow; see `pr402/docs/REFUND_SWEEPER.md`
for the rationale.

**On-chain state change**:
- `Payment.state` flips to `1` (Released) or `2` (Refunded).
- A `PaymentReleasedEvent` / `PaymentRefundedEvent` is emitted.
- `Payment.closed_at` is set.

The Payment account remains rent-exempt until anyone calls `ClosePayment`
(after `closure_delay_seconds`); rent flows back to the buyer.

---

## 4. Field requirements per profile

The base SLA shape is the same across profiles:

| Field | Required | Source | Notes |
|---|---|---|---|
| `version` | yes | buyer | Currently `1`. |
| `profile_id` | yes | buyer | Must match the chosen profile exactly (e.g. `x402/oracles/api-quality/v1`). |
| `payment_uid` | yes | buyer | 64 hex chars. Hex of `Payment.payment_uid`. Binds this SLA to one and only one on-chain payment; the seller's evidence must echo it. |
| `buyer_nonce` | optional | buyer | 64 hex chars (32 random bytes). Defends cross-SLA replay when SLA terms are otherwise identical between two buyers. The seller's evidence must echo it when set. |

Profile-specific SLA fields:

### `x402/oracles/api-quality/v1`
- `endpoint` (string), `method` (string)
- `min_status_code` / `max_status_code` (u16)
- `max_latency_ms` (u64)
- `required_fields[]`, `response_schema` (JSON Schema, optional)
- `min_body_length` (optional)

Evidence: `status_code`, `latency_ms`, `response_body`,
`response_headers` (optional), `timestamp`, `payment_uid` (echoed),
`buyer_nonce` (echoed when SLA carried one).

### `x402/oracles/onchain-transfer/v1`
- `cluster` (`mainnet-beta` / `devnet` / `testnet`)
- `expected_transfers[]` with `mint`, `recipient_owner`, `min_amount`,
  `direction` (`in` / `out`)
- `deadline_unix` (optional)

Evidence: `version`, `profile_id`, `tx_signature` (base58 Solana
signature), `asserted_transfers[]`, `submitted_at`, `payment_uid`
(echoed), `buyer_nonce` (echoed).

### `x402/oracles/file-delivery/attestation/v1`
- `expected_size_bytes_min` / `expected_size_bytes_max` (u64)
- `expected_mime` (optional)
- `expected_extension` (optional)
- `attestor_pubkey` (optional). When set, the v1 evaluator records that
  the SLA expects a signed attestation but the v1 streaming-evidence
  path does not yet carry signatures, so the check is recorded as
  failed. Sellers should leave it unset for the v1 attestation profile.

Evidence is the file bytes themselves (uploaded via `POST
/v1/registry/blob`). The oracle re-hashes on read and verifies size +
MIME. `payment_uid` / `buyer_nonce` echoing is via the SLA only in this
profile — the file *is* the evidence and there is no JSON envelope to
inject the echo into. The on-chain `sla_hash` commits the SLA bytes
(including the nonce), which is enough for cross-SLA replay defense at
the SLA layer.

---

## 5. Trust boundaries

What each role must trust, and what is content-addressed (no trust):

| Anchor | Trust required? | Why |
|---|---|---|
| `Payment.sla_hash` (on-chain) | None | The chain is the ground truth. |
| `Payment.delivery_hash` (on-chain) | None | Same. |
| `Payment.resolution_hash` (on-chain) | None — but verifiable | Anyone with SLA + evidence bytes can recompute and detect a lying oracle. |
| Registry `GET /v1/registry/<hash>` | None | Re-hashed on read; mismatch returns `500`. |
| Registry `POST /v1/registry/sla` | Seller's bearer token | Only authenticated sellers may upload; the bytes themselves are still content-addressed. |
| `oracle_authority` choice | Trust the oracle | The buyer chose this oracle; if it lies, the on-chain `resolution_hash` lets a third party prove it. Recourse is operator reputation, not the chain. |
| pr402's `slaHash` field | Trust pr402 | pr402 only relays; it doesn't author SLA bytes. The on-chain `Payment.sla_hash` is the buyer's own commitment via signing. |

The protocol's only single-point-of-trust is the chosen oracle. Two
properties limit that trust:

1. **Cross-payment replay protection** — the same `tx_signature`
   (`onchain-transfer`) or `delivery_hash` (`file-delivery`) cannot
   settle two different payments. A faulty seller is detected by the
   oracle on the second attempt; a faulty oracle that ignores the rule
   is detected by any third party recomputing `resolution_hash` from the
   on-chain envelope.
2. **Per-payment buyer_nonce** — when the SLA carries a nonce, no two
   buyers can be attacked with the same SLA template (their hashes
   differ even if every other byte matches). A seller who tries to
   replay evidence from one buyer against another is caught at
   evaluation.

---

## 6. Resolution reason codes

When the oracle submits a `ConfirmOracle` transaction, it writes a `resolution_reason` (u16) to the on-chain `Payment` account. These reason codes are divided into two ranges:
1. **Standard codes (`0` to `255`)**: well-known, interoperable reasons defined by the core `sla-escrow` protocol.
2. **Custom codes (from `256` onwards)**: family-specific or deployment-specific codes defined by individual oracle profiles.

### Standard resolution reasons (0..=255)
| Code | Name | Meaning |
|---|---|---|
| `0` | `None` | Default for approvals, or when no specific rejection reason applies |
| `1` | `StatusCodeOutOfRange` | HTTP status code fell outside the SLA-specified range |
| `2` | `LatencyExceeded` | Response latency exceeded the SLA threshold |
| `3` | `SchemaValidationFailed` | Response body failed JSON Schema validation |
| `4` | `RequiredFieldsMissing` | One or more required fields were missing from the response |
| `5` | `BodyTooShort` | Response body was shorter than the minimum length |
| `6` | `HashMismatch` | Delivery evidence hash did not match the on-chain commitment |
| `7` | `EvidenceUnavailable` | Off-chain evidence could not be fetched or was unavailable |
| `100` | `SLA_UNAVAILABLE` | Active Guardian: SLA bytes not retrievable from registry after retries |
| `101` | `EVIDENCE_UNAVAILABLE` | Active Guardian: Evidence bytes not retrievable from registry after retries |
| `102` | `EVALUATION_TIMEOUT` | Active Guardian: Pipeline timeout or max retries exhausted without a verdict |
| `255` | `GeneralRejection` | Catch-all for standard rejections |

### Custom family ranges
Custom reason codes are partitioned per oracle family to avoid overlaps:
- **`256..=319`**: `x402/onchain-transfer/*`
- **`320..=383`**: `x402/file-delivery/*`
- **`384..=447`**: Reserved for future `x402/compute-result/*`
- **`448..=511`**: Reserved for ecosystem-wide additions
- **`512..=65535`**: Available for deployment-local customization

#### On-chain Transfer Family (`x402/onchain-transfer/*`) custom codes:
| Code | Name | Meaning |
|---|---|---|
| `256` | `TRANSFER_TX_NOT_FOUND` | SPL transfer transaction signature not found on-chain |
| `257` | `TRANSFER_TX_FAILED` | SPL transfer transaction failed execution |
| `258` | `TRANSFER_AMOUNT_INSUFFICIENT` | Transferred token amount is less than expected by SLA |
| `259` | `TRANSFER_MINT_MISMATCH` | Token mint in the transfer transaction does not match SLA |
| `260` | `TRANSFER_DEADLINE_EXCEEDED` | Transfer block time exceeds the SLA deadline |
| `261` | `TRANSFER_CLUSTER_MISMATCH` | RPC cluster in SLA does not match oracle cluster |
| `262` | `TRANSFER_RECIPIENT_NOT_RESOLVABLE` | Expected recipient token account / owner is not resolvable |
| `263` | `TRANSFER_DIRECTION_MISMATCH` | Transfer direction does not match SLA expectation (`in` / `out`) |
| `264` | `TRANSFER_EVIDENCE_PREDATES_PAYMENT` | Freshness check failed: transaction block time predates `Payment.created_at` |
| `265` | `TRANSFER_TX_SIGNATURE_REUSED` | Replay defense: signature was already settled for a different payment |
| `266` | `TRANSFER_PAYMENT_UID_MISMATCH` | Evidence's `payment_uid` does not match the target `Payment.payment_uid` |
| `267` | `TRANSFER_BUYER_NONCE_MISMATCH` | Nonce check failed: evidence did not echo SLA's `buyer_nonce` |
| `268` | `TRANSFER_BLOCK_TIME_MISSING` | Transaction is missing block time and strict block time checking is enabled |
| `269` | `TRANSFER_SENDER_MISMATCH` | Pinned `sender_owner` was not found or did not sign a negative delta |

#### File Delivery Family (`x402/file-delivery/*`) custom codes:
| Code | Name | Meaning |
|---|---|---|
| `320` | `BLOB_SIZE_OUT_OF_RANGE` | Stored blob size does not fit SLA bounds |
| `321` | `BLOB_MIME_MISMATCH` | Stored blob MIME type does not match SLA |
| `322` | `BLOB_ATTESTOR_SIGNATURE_INVALID` | Stored blob attestor signature is invalid (recorded as failed in v1) |
| `323` | `BLOB_UPLOAD_INCOMPLETE` | Blob upload is incomplete on the storage backend |
| `324` | `BLOB_PREDATES_PAYMENT` | Freshness check failed: blob registry timestamp predates `Payment.created_at` |
| `325` | `BLOB_DELIVERY_HASH_REUSED` | Replay defense: delivery hash was already settled for a different payment |
| `326` | `BLOB_PAYMENT_UID_MISMATCH` | Evidence's `payment_uid` does not match the target `Payment.payment_uid` |
| `327` | `BLOB_BUYER_NONCE_MISMATCH` | Nonce check failed: evidence did not echo SLA's `buyer_nonce` |

---

## 7. Failure modes

| Failure | Detected by | When | Recovery |
|---|---|---|---|
| Buyer authors SLA, never sends to seller | nobody | n/a | No payment was made — nothing to recover. |
| Seller uploads SLA bytes that differ from buyer's | buyer | After hash returned by registry mismatches local hash. | Buyer aborts before signing FundPayment. |
| Buyer signs FundPayment, never gets delivery | buyer | After `expires_at` | `RefundPayment` returns escrow to buyer. |
| Seller submits stale evidence (taken before payment) | oracle | At evaluation; rejected because `evidence.timestamp < Payment.created_at`. | Buyer calls `RefundPayment` after `refund_cooldown_seconds` elapses. |
| Seller reuses one tx for two payments (`onchain-transfer`) | oracle | At evaluation of the second payment; rejected because the `tx_signature` was already settled for a different `payment_uid`. | `RefundPayment` after `expires_at`. |
| Seller reuses one blob for two payments (`file-delivery`) | oracle | At evaluation of the second payment; rejected because the `delivery_hash` was already settled for a different `payment_uid`. | Same. |
| Oracle goes offline | buyer | Delivery sits without verdict; `expires_at` passes. | `RefundPayment`. (When the pr402 health gate is enabled, pr402 refuses to bind to oracles that were known-offline at build time, returning HTTP 503.) |
| Oracle returns wrong verdict | third-party auditor | Recompute `resolution_hash` from SLA + evidence + verdict envelope; compare to on-chain. | Off-chain dispute via operator reputation. The protocol does not currently support on-chain dispute. |
| pr402 returns a tampered `slaHash` | buyer | Buyer hashes locally; mismatch with what they hand to seller. | Buyer aborts. |
| Buyer tries to fund with wrong oracle | pr402 | At build time: `oracleAuthority` not in `accepted.extra.oracleAuthorities[]` → 400. | Buyer fixes their request. |
| pr402 health gate enabled, oracle unhealthy | pr402 | At build time: returns HTTP 503 `oracle_unhealthy`. | Buyer retries against another profile in `oracleProfiles[]`. |

---

## 8. Versioning

The protocol's identity is the `profile_id`:
`x402/oracles/<family>/<profile>/<version>`.

**Profile bumps** (`v1` → `v2`):
- Required when the SLA or evidence wire shape changes in a way that
  breaks old evaluators (e.g. adding a required field that v1
  evaluators wouldn't compute, or removing a field v1 relied on).
- Each version has its own `NORMATIVE.md` and is registered as a
  separate profile in pr402. v1 and v2 may coexist; buyers and sellers
  declare which they speak via `accepts[].extra.oracleProfiles[]` and
  the `profile_id` field of the SLA.
- **Older clients reading newer SLA bytes** see optional fields they
  don't recognize and can ignore them safely (`#[serde(default)]` is
  the wire-format guarantee). Required fields, by contrast, can only
  appear in a version bump.

**Protocol version** (top of this doc):
- Bumps when the cross-actor flow itself changes: a new phase, a
  reassignment of who authors what, a new on-chain instruction in the
  happy path, etc. Independent from any code version and from any
  profile version.
- Profile-version bumps that don't change the cross-actor flow do NOT
  require a protocol-version bump.

---

## 9. Quick sequence reference

```
Phase   Buyer                       Seller                 Oracle             Chain (sla-escrow)
─────   ─────                       ──────                 ──────             ─────────────────
  1     read 402 + capabilities   ←  publishes 402         (idle)             (idle)
        pick (profile, oracle)
  2     gen payment_uid + nonce
        author sla.json
        sla_hash = SHA256(bytes)

  3     hand bytes ───────────────▶ POST /v1/registry/sla ─────────────────▶
                                    (registry stores; returns hash)
        verify hash ← ←  ← seller hands hash + url back ←
  4     POST build-sla-escrow…
        (pr402 → unsigned tx)
        sign + submit ────────────────────────────────────────────────────▶ FundPayment
                                                                              Payment.sla_hash
                                                                              Payment.created_at
                                                                              state=Funded

  5                                 GET /v1/registry/<sla_hash>
                                    (read payment_uid + nonce)
                                    do the work
                                    POST /v1/registry/delivery ───────────▶
                                                                  delivery_hash returned
  6                                 sla-escrow submit-delivery ───────────▶ SubmitDelivery
                                                                              Payment.delivery_hash
                                                                              event emitted
  7                                                        observes event
                                                           GET sla + delivery
                                                           verifies binding
                                                           runs evaluator
                                                           ConfirmOracle ──▶ Payment.resolution_*
                                                                              state still Funded
                                                                              event emitted
  8     observe event              observe event
        if approved → ReleasePayment (anyone may call) ──────────────────▶ tokens → seller
        if rejected → RefundPayment (buyer/seller/admin) ──────────────────▶ tokens → buyer
        if expired without verdict → RefundPayment ─────────────────────▶ tokens → buyer
```

The on-chain anchor is `Payment.sla_hash`. Everything else is
content-addressed and verifiable by anyone, anytime.

---

## 9. Related documents

- [`SELLER_GUIDE.md`](./SELLER_GUIDE.md) — concrete shell recipes per
  profile, registration flow, common pitfalls.
- [`BUYER_GUIDE.md`](./BUYER_GUIDE.md) — buyer-side shell recipes,
  oracle-selection guidance.
- [`DEPLOYMENT.md`](./DEPLOYMENT.md) — operator runbook for oracle
  bring-up.
- [`OPERATIONS.md`](./OPERATIONS.md) — day-2 oracle ops.
- Each profile's `NORMATIVE.md` — per-profile rules, field definitions,
  resolution-reason codes.
- pr402's `agent-integration.md` (served at the deployed facilitator)
  — pr402-side details for buyers calling the build / verify / settle
  endpoints.

---

## Changelog

- **1.0**: initial publication. Buyer-authored SLA with mandatory
  `payment_uid` and optional `buyer_nonce`; cross-payment replay
  protection (no `tx_signature` or `delivery_hash` may settle two
  different payments); evidence-freshness lower bound (evidence
  timestamp / observed `block_time` must be at or after
  `Payment.created_at`); pr402 optional oracle health gate gating
  `/capabilities` annotations and `/build-sla-escrow-payment-tx`
  binding.
- **1.1** (2026-05-20): Protocol-aligned updates from devnet E2E validation.
  - **`paymentUidHex`**: pr402's `build-sla-escrow-payment-tx` now accepts
    `paymentUidHex` (64 lowercase hex chars) as the canonical way for buyers
    to specify the on-chain `Payment.payment_uid` bytes. When set, pr402 uses
    those 32 bytes verbatim — no `sanitize_uid` text encoding. Mutually
    exclusive with the legacy `paymentUid` (string) field. Response includes
    `paymentUidHex` so callers who didn't pass it can read back the canonical
    bytes. The SLA's `payment_uid` field MUST equal this hex.
  - **Seller uploads SLA on paid path**: in the current implementation, the
    seller's paid-path handler (not the buyer) uploads the canonical SLA bytes
    to the registry after reconstructing them from the buyer's request params +
    the extracted `payment_uid`. The buyer never directly touches the registry.
    Phase 3 in §3 is updated: the "buyer hands SLA to seller" step is now
    implicit (the seller reconstructs from the 402 envelope fields + on-chain
    `payment_uid`).
  - **Active Guardian**: the oracle now retries SLA/evidence fetch with
    exponential backoff (10s initial, 120s cap, 30 attempts). If artifacts
    remain unavailable within `ORACLE_REJECT_SAFETY_MARGIN_SEC` (default 600s /
    10 min) before `expires_at`, the oracle issues a protective REJECT
    (`resolution_state=2`, `resolution_reason=100/101/102`). This closes the
    "seller withholds artifacts → oracle ghosted → seller self-releases" attack
    vector. See `oracles/docs/ARCHITECTURE.md` §Active Guardian.
  - **Oracle's stricter cutoff**: the oracle's reject margin (10 min) is
    intentionally larger than the on-chain `delivery_cutoff_seconds` (5 min).
    Sellers must deliver and upload well before the deadline; last-second
    delivery is risky.
  - **`cluster` field in SLA**: the `TransferSla` (onchain-transfer family)
    now requires a `cluster` field (`"devnet"` / `"mainnet-beta"` /
    `"testnet"`) so the oracle can verify it matches its own RPC cluster.
  - **Escrow PDA as `payTo`**: for sla-escrow, `accepts[].payTo` MUST be the
    per-asset escrow PDA (derived from `[b"escrow", USDC_mint, bank_pda]`),
    NOT the merchant wallet. The merchant wallet goes in
    `accepts[].extra.merchantWallet`. pr402's verify path enforces this.
