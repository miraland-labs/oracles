# Seller Integration Guide

You're a seller. A buyer paid you through `sla-escrow`, the funds are
locked on-chain, and an oracle decides whether you get paid. This guide
walks you through what you need to do.

> **Normative reference.** Seller obligations are specified in
> [`spec/sla-escrow-protocol/v1`](../spec/sla-escrow-protocol/v1/NORMATIVE.md)
> §4. Wire-level registry calls in
> [`spec/registry-http-api/v1`](../spec/registry-http-api/v1/NORMATIVE.md).
> SLA envelope rules in
> [`spec/sla-document/v1`](../spec/sla-document/v1/NORMATIVE.md).
> The on-chain `SubmitDelivery` instruction bytes you build are
> specified in
> [`spec/sla-escrow-onchain-abi/v1`](../spec/sla-escrow-onchain-abi/v1/NORMATIVE.md)
> §5.7 — useful for non-Rust sellers or multi-cluster Rust binaries.

## 30-second summary

You upload **what was promised** (an SLA) and **what you delivered**
(evidence) to the oracle's registry. You commit the delivery hash
on-chain via `SubmitDelivery`. The oracle re-fetches the bytes by hash,
checks them, and either approves your payout or rejects it.

## 1. Pick your delivery scenario

Three reference oracles ship today.

| You sell | Profile | Evidence shape |
|---|---|---|
| JSON HTTP API | `x402/oracles/api-quality/v1` | Small SLA + small delivery JSON |
| On-chain SPL transfer / swap | `x402/oracles/onchain-transfer/v1` | Small SLA + evidence JSON pointing at a Solana tx |
| Large file | `x402/oracles/file-delivery/attestation/v1` | Small SLA + the file itself (streamed) |

## 2. Two authoring patterns

You'll implement one or both (per spec §4.1):

**Direct authoring.** Buyer constructs the SLA JSON locally and hands
you the bytes. You upload them verbatim to the registry. Simpler to
implement seller-side; requires the buyer to assemble per-profile JSON.

**Delegated authoring (HTTP 402).** Buyer sends intent-bearing
parameters in the request URL or body. You produce the SLA JSON from
those parameters using your seller-side template, hash it, and return
the hash in `accepts[].extra.slaHash` of your 402 response. The buyer
signs `FundPayment` with that hash. This is the pattern used by
`spl-token-balance-serverless` (the production reference).

Delegated authoring **MUST** produce deterministic SLA bytes (spec §4.6
#1) — given the same inputs, the seller's generator must produce
byte-identical output, because the SLA is built twice (once on the
unpaid 402 path to compute the hash, once on the paid path to upload).
A sorted-keys / no-whitespace canonicalizer is the standard approach.
Reference: [`spl-token-balance-serverless/src/sla_builder.rs`](../../spl-token-balance-serverless/src/sla_builder.rs).

## 3. Find the oracle

The buyer's HTTP-402 challenge from your service should include the
oracle authority and registry URL the buyer wants:

```json
{
  "scheme": "v2:solana:sla-escrow",
  "extra": {
    "oracleProfiles": [{
      "profileId": "x402/oracles/api-quality/v1",
      "operatorPubkey": "OracLe...",
      "registry": "https://oracle.example.com/v1/registry"
    }]
  }
}
```

You bake these values into your `paymentRequirements` once and forget.

A copy-paste helper generates the entire `oracleProfiles[]` entry from a
running oracle:

```bash
bash oracles/scripts/seller-emit-oracle-profile.sh \
    https://oracle.example.com
```

Built-in oracle: when a pr402 deployment has its built-in oracle
enabled, `GET /capabilities` advertises an `oracle-onchain-transfer`
instance the facilitator operator runs. For SPL transfer use cases this
is the simplest path.

## 4. Get a bearer token (one-time setup)

The registry needs to know which seller you are. You prove it by signing
a challenge with your wallet keypair, exactly once.

```bash
bash oracles/scripts/seller-register.sh \
    https://oracle.example.com \
    /path/to/seller-keypair.json
# → prints: BEARER=<long-base58-token>
# Save as $SELLER_TOKEN; the oracle never returns it again.
```

The flow under the hood (spec §7 of the registry HTTP API):

1. `GET /v1/registry/seller/challenge?wallet=<pubkey>` → `{challenge, expires_at}`
2. Sign the **raw UTF-8 bytes** of `challenge` with Ed25519 (spec §7.4 — **NOT** SIMD-0048 envelope; `solana sign-offchain-message` is non-conformant for this).
3. `POST /v1/registry/seller/register` with `{wallet, signature, challenge}` → `{id, token}`

Lose the token, run `POST /v1/registry/seller/rotate` to get a new one.

## 5. The full happy path

### 5.A. JSON API quality

```bash
ORACLE="https://oracle.example.com"
TOKEN="$SELLER_TOKEN"

# Direct authoring: the buyer hands you sla.json bytes.
# Delegated authoring: you produced sla.json from the buyer's
# parameters using your deterministic template (spec §4.6).

# 1. Upload SLA. Registry returns SHA-256.
SLA_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/sla" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq -r .sha256)

# 2. Re-fetch SLA to read payment_uid + buyer_nonce. Content-addressed,
#    safe to trust — the registry re-hashes on read.
SLA=$(curl -fsS "$ORACLE/v1/registry/$SLA_HASH")
PAYMENT_UID=$(echo "$SLA" | jq -r .payment_uid)
BUYER_NONCE=$(echo "$SLA" | jq -r '.buyer_nonce // empty')

# 3. Perform the work.

# 4. Author evidence. Echo payment_uid (and buyer_nonce when present) verbatim.
cat > delivery.json <<EOF
{
  "version": 1,
  "profile_id": "x402/oracles/api-quality/v1",
  "status_code": 200,
  "latency_ms": 240,
  "response_body": {"result": "..."},
  "timestamp": $(date +%s),
  "payment_uid": "$PAYMENT_UID"$([ -n "$BUYER_NONCE" ] && echo ",
  \"buyer_nonce\": \"$BUYER_NONCE\"")
}
EOF

# 5. Upload delivery.
DELIVERY_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/delivery" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @delivery.json | jq -r .sha256)

# 6. Submit on-chain. Must arrive at least delivery_cutoff_seconds before expiry.
sla-escrow submit-delivery \
    --seller /path/to/seller-keypair.json \
    --payment-uid "$PAYMENT_UID" \
    --delivery-hash "$DELIVERY_HASH"
```

### 5.B. On-chain SPL transfer / swap

You **already broadcast** the transfer (that's the deliverable); the
evidence references the tx signature.

```bash
SLA_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/sla" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq -r .sha256)

SLA=$(curl -fsS "$ORACLE/v1/registry/$SLA_HASH")
PAYMENT_UID=$(echo "$SLA" | jq -r .payment_uid)
BUYER_NONCE=$(echo "$SLA" | jq -r '.buyer_nonce // empty')

# Broadcast the transfer.
TX_SIG="$(spl-token transfer ... --output json | jq -r .signature)"

cat > delivery.json <<EOF
{
  "version": 1,
  "profile_id": "x402/oracles/onchain-transfer/v1",
  "tx_signature": "$TX_SIG",
  "asserted_transfers": [{
    "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "recipient_owner": "BUYER_WALLET_PUBKEY",
    "claimed_delta": "1000000"
  }],
  "submitted_at": $(date +%s),
  "payment_uid": "$PAYMENT_UID"$([ -n "$BUYER_NONCE" ] && echo ",
  \"buyer_nonce\": \"$BUYER_NONCE\"")
}
EOF

DELIVERY_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/delivery" ... | jq -r .sha256)
sla-escrow submit-delivery --payment-uid "$PAYMENT_UID" --delivery-hash "$DELIVERY_HASH" ...
```

The oracle calls `getTransaction(jsonParsed)` against `tx_signature`,
re-derives the pre/post token deltas, and approves only when the
observed delta meets `min_amount`. The chain is the ground truth.

**Token-2022 transfer-fee mints**: the recipient receives the post-fee
net amount, not what you debited. The buyer's `min_amount` MUST be the
net the recipient will receive; you broadcast a slightly higher gross.
Check before broadcasting:

```bash
spl-token display "$MINT_ADDRESS"
# Look for a non-zero "Transfer fee" line.
```

See [onchain-transfer-v1 NORMATIVE §6.1](../oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md#61-token-2022-transfer-fee-handling).

#### Idempotency contract for service-side sellers

For long-running services (not one-shot shells), you need to guard
against crashes between broadcast and `submit_delivery`. The seller MUST
durably persist `(payment_uid, tx_signature, broadcast_at)` after
`send_and_confirm_transaction` returns and before evidence upload, so a
restart can resume rather than re-broadcast.

`SubmitDelivery` is itself idempotent at the program level — calling it
twice with the same `(payment_uid, delivery_hash)` is a no-op rejection.
But broadcast + submit as a sequence is NOT safe without the persisted
`tx_signature`. Single-shot shell flows are exempt because a crash means
the buyer eventually refunds via TTL.

Recommended persistence (Postgres):

```sql
CREATE TABLE seller_payments (
  payment_uid    BYTEA  PRIMARY KEY,
  tx_signature   TEXT   NOT NULL,
  broadcast_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO seller_payments (payment_uid, tx_signature)
VALUES ($1, '') ON CONFLICT (payment_uid) DO NOTHING;
-- After broadcast succeeds:
UPDATE seller_payments SET tx_signature = $2 WHERE payment_uid = $1
  AND tx_signature = '';
```

### 5.C. Large file delivery

The file itself **is** the delivery. Use `/v1/registry/blob` (no JSON
wrapper).

```bash
SLA_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/sla" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq -r .sha256)

# Stream-upload the file. Cap is max_blob_bytes (default 5 GiB; check /v1/registry/info).
DELIVERY_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/blob" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: video/mp4" \
    --data-binary @output.mp4 | jq -r .sha256)

sla-escrow submit-delivery --payment-uid "$PAYMENT_UID" --delivery-hash "$DELIVERY_HASH" ...
```

For file-delivery, `payment_uid` and `buyer_nonce` binding lives in the
SLA only — the file IS the evidence; there's no JSON envelope to inject
echo fields into. The on-chain `sla_hash` commits the SLA bytes
(including the nonce), which is enough for cross-SLA replay defense.

## 6. What the oracle checks

Each profile has a normative spec. Read the relevant one once.

| Profile | Spec |
|---|---|
| `api-quality/v1` | [oracle-api-quality/spec/api-quality-v1/NORMATIVE.md](../oracle-api-quality/spec/api-quality-v1/NORMATIVE.md) |
| `onchain-transfer/v1` | [oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md](../oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md) |
| `file-delivery/attestation/v1` | [oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md](../oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md) |

High-level rules:

- **api-quality** approves when status is in range, latency under cap,
  required fields present, schema validates, body length above min.
- **onchain-transfer** approves when on-chain delta for `(mint,
  recipient_owner)` is at least `min_amount`, tx is on `cluster`, and
  `meta.err == null`.
- **file-delivery** approves when blob size is in `[min, max]` and (if
  set) sniffed MIME matches `expected_mime`.

If rejected, `resolution_reason` (u16) explains why. Code ranges:
0–7 + 100–102 + 255 are interoperable standard codes; 256–319 are
onchain-transfer custom; 320–383 are file-delivery custom; 384+ is
reserved.

## 7. Common pitfalls

**Hash on-chain doesn't match registry.** Single most common error.
Cause: you hashed one byte sequence but uploaded a different one
(trailing newlines, BOM, pretty-print vs minified). Fix: always use the
SHA-256 the registry returned in the upload response as your on-chain
`delivery_hash`. Don't recompute from local bytes.

**`profile_id` mismatch.** SLA must declare `profile_id` matching the
oracle's profile exactly. Missing or wrong → instant reject.

**Bearer expired or revoked.** `401 Unauthorized` from any
`POST /v1/registry/...`. Re-run `seller-register.sh`.

**Wrong cluster.** `onchain-transfer` binaries are cluster-pinned via
`TRANSFER_CLUSTER`. Your `sla.cluster` must match. Symptom:
`TRANSFER_CLUSTER_MISMATCH` (code 261).

**Late delivery.** Submission must arrive at least
`delivery_cutoff_seconds` before `expires_at`. Default 5 minutes.
Symptom: `DeliveryTooLateForOracle`.

**File too large.** `413 Payload Too Large`. Check `GET /v1/registry/info`
for the deployment's `max_blob_bytes`.

**Non-deterministic SLA in delegated authoring.** The unpaid-path hash
won't match the paid-path bytes. Symptom: `EvidenceUnavailable` or
hash mismatch at evaluation. Fix: use a sorted-keys canonicalizer (see
`spl-token-balance-serverless/src/sla_builder.rs`).

## 8. Quick sanity check before shipping

- [ ] `seller-register.sh` returned a bearer; saved as `$SELLER_TOKEN`.
- [ ] `POST /v1/registry/sla` returned 200 with a 64-hex `sha256`.
- [ ] `GET /v1/registry/<that-sha256>` returns your exact SLA bytes.
- [ ] `POST /v1/registry/delivery` (or `/blob`) returned 200 with a `sha256`.
- [ ] You used **that response sha256** as your on-chain
      `delivery_hash`, not a hash you computed yourself.
- [ ] (Delegated authoring) Same parameters → same SLA bytes,
      reproducibly.

## 9. FAQ

**Do I need to run my own oracle?** No. Use whichever oracle the
buyer's HTTP-402 advertises. Running an oracle is a different role
(operator) — see [`DEPLOYMENT.md`](./DEPLOYMENT.md).

**Oracle goes down.** Settlement waits. The chain monitor catches up
via startup backfill on restart; you don't lose the deliverable.
Multiple `oracleProfiles[]` entries let buyers pick alternates.

**What gets stored on-chain?** Hashes only. `sla_hash`,
`delivery_hash`, `resolution_hash` are SHA-256 fingerprints. Bytes live
in the registry.

**Oracle sees customer data?** Whatever you upload. Don't upload
secrets; encrypt at the application layer if needed.

**Why bearer auth?** Registry pollution prevention; binds uploads to
seller wallet for audit. One-time setup; keep indefinitely.

**Token rotation in flight?** Old and new tokens both work briefly.
On-chain `payment_uid` flow is independent of which bearer was used.

**History.** `GET /stats` and `GET /health` are public. Per-payment
verdict history is operator policy.

## Appendix — minimal seller-register helper

A small `seller-register.sh` is shipped at
[`oracles/scripts/seller-register.sh`](../scripts/seller-register.sh).

TypeScript sketch:

```ts
const { challenge } = await fetch(
  `${oracle}/v1/registry/seller/challenge?wallet=${wallet.publicKey.toBase58()}`
).then(r => r.json());

// MUST sign raw UTF-8 bytes, not SIMD-0048 envelope.
const sigBytes = nacl.sign.detached(
  new TextEncoder().encode(challenge),
  wallet.secretKey
);

const { token } = await fetch(`${oracle}/v1/registry/seller/register`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    wallet: wallet.publicKey.toBase58(),
    signature: bs58.encode(sigBytes),
    challenge,
  }),
}).then(r => r.json());
```
