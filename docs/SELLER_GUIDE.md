# Seller Integration Guide

You're a seller. A buyer paid you through `sla-escrow`, the funds are locked
on-chain, and an **oracle** decides whether you get paid. This guide walks
you through the three things you actually need to do, in plain language.

> **30-second summary.** You upload **what you promised** (an SLA) and **what
> you delivered** (evidence) to a registry the oracle trusts. You commit the
> hashes of those bytes on-chain. The oracle re-fetches the bytes by hash,
> checks them, and either approves your payout or rejects it.

If you've done HTTP-402 + `pr402` once before, this is just two extra HTTP
calls and one on-chain `submit_delivery`.

> **First time integrating?** Read [`SLA_ESCROW_PROTOCOL.md`](./SLA_ESCROW_PROTOCOL.md)
> first — that's the cross-actor reference that shows how buyer, seller,
> oracle, and pr402 fit together. This guide gives you the seller's
> recipes; the protocol doc gives you the big picture.

---

## 1. Pick your delivery scenario

Three reference oracles ship today. Pick the one that matches what you sell:

| You sell…                                                | Use this oracle / profile                                  | What you upload                                              |
| -------------------------------------------------------- | ---------------------------------------------------------- | ------------------------------------------------------------ |
| **A JSON HTTP API** (status, latency, schema)            | `oracle-api-quality` / `x402/oracles/api-quality/v1`               | A small SLA JSON + a small delivery JSON                     |
| **An on-chain SPL token transfer or swap**               | `oracle-onchain-transfer` / `x402/oracles/onchain-transfer/v1`     | A small SLA JSON + a small evidence JSON pointing at a real Solana tx |
| **A large file** (video, dataset, generated artifact)    | `oracle-file-delivery` / `x402/oracles/file-delivery/attestation/v1` | A small SLA JSON + the **file itself** (streamed)            |

Don't overthink the choice. If your delivery is a JSON response you compute
on the fly, pick api-quality. If it's a Solana transaction you broadcast,
pick onchain-transfer. If it's a file, pick file-delivery.

## 2. Find the oracle's address

The buyer's HTTP-402 challenge from your service includes (or should
include) the oracle authority and registry URL the buyer wants you to use.
A typical `accepts[]` entry looks like:

```json
{
  "scheme": "v2:solana:sla-escrow",
  "extra": {
    "oracleProfiles": [{
      "profileId": "x402/oracles/api-quality/v1",
      "operatorPubkey": "OracLe...",
      "registry": "https://oracle-api.example.com/v1/registry"
    }]
  }
}
```

Whatever URL is in `registry` is where you upload your bytes. The
`operatorPubkey` is what the buyer will use as `oracle_authority` when they
fund the escrow.

If you advertise your own service via pr402, you bake these values into
your `paymentRequirements` once and forget. See
[`oracle-common/docs/PR402_CONTRACT.md`](../oracle-common/docs/PR402_CONTRACT.md)
for the normative seller-side advertisement shape.

> **Don't hand-type the JSON.** A copy-paste helper script generates the
> entire `oracleProfiles[]` entry from a running oracle:
>
> ```bash
> bash oracles/scripts/seller-emit-oracle-profile.sh \
>     https://oracle-api.example.com
> ```
>
> Output is a single JSON object with `profileId`, `operatorPubkey`,
> `registry`, plus `normativeSpecUrl` and `cluster` when the oracle
> advertises them. Paste it directly into `accepts[].extra.oracleProfiles[]`.
> No typos, no guessed pubkeys.

> **Built-in oracle on the pr402 facilitator.** When a pr402 deployment
> has its built-in oracle enabled, `GET /capabilities` advertises an
> `oracle-onchain-transfer` instance the facilitator operator runs
> themselves — visible under `slaEscrowOracleProfiles[]` with profile
> id `x402/oracles/onchain-transfer/v1` and a `defaultOperatorPubkey`.
>
> If you sell SPL token transfers (the AetherVane Zodiac shape: pre-fund
> $X USDC, deliver Y tokens of mint M to recipient R), the simplest path
> is:
>
> - Tell buyers to use the facilitator's default by leaving them to read
>   `slaEscrowOracleProfiles[]`, OR
> - Reference the same `(profileId, operatorPubkey, registry)` triple
>   yourself in `accepts[].extra.oracleProfiles[]` so buyers don't have
>   to look it up.
>
> You're free to advertise a different oracle (your own, or an
> ecosystem one) instead — for trust reasons, performance reasons, or
> because the built-in oracle's deployment serves a different cluster
> than yours. The oracle-selection rules don't change. For other profiles
> (api-quality, file-delivery), the facilitator does NOT ship a built-in
> oracle; you pick from ecosystem operators.

## 3. Get a bearer token (one-time setup)

The registry needs to know which seller you are. You prove it by signing a
challenge with your wallet keypair, exactly once. The oracle returns a
bearer token you keep and reuse.

A copy-paste helper script:
[`oracles/scripts/seller-register.sh`](../scripts/seller-register.sh).

```bash
# One-line: get a bearer token bound to your wallet.
sudo bash oracles/scripts/seller-register.sh \
    https://oracle-api.example.com \
    /path/to/seller-keypair.json
# → prints: BEARER=<long-base58-token>
# Save it as $SELLER_TOKEN; the oracle never returns it again.
```

What that helper does under the hood (so you can write your own in
TypeScript / Python / whatever):

```text
1. GET  /v1/registry/seller/challenge?wallet=<your-wallet-pubkey>
        → {"challenge": "<base58-32B>", "expires_at": "..."}
2. Sign the challenge bytes with your wallet keypair (Ed25519).
3. POST /v1/registry/seller/register
        body: {"wallet": "<pubkey>", "signature": "<base58-sig>", "challenge": "<base58>"}
        → {"id": 1, "token": "<bearer>"}
```

The oracle stores only `SHA256(token)`. Lose the token, run
`POST /v1/registry/seller/rotate` to get a new one (revokes the old).

## 4. The full happy path — one shell script per scenario

> **Who authors the SLA bytes?** **The buyer**, not the seller. The buyer
> bakes a fresh per-payment `payment_uid` (and an optional `buyer_nonce`) into
> the SLA before hashing it, so each FundPayment is cryptographically tied to
> exactly one SLA document. The seller's role is *upload mechanic*: the buyer
> hands you the final SLA bytes, you `POST /v1/registry/sla` with your bearer
> token, and the registry returns the hash both sides verify locally.
>
> When the buyer wants extra protection (defends against cross-SLA replay
> when two buyers happen to produce identical SLA terms), they include a
> `buyer_nonce` (32 random bytes, hex). The on-chain `sla_hash` commits to
> the nonce by hash, the registry stores the bytes content-addressed, and you
> simply echo `payment_uid` (and `buyer_nonce` when present) verbatim in
> `delivery.json` after re-fetching the SLA back via
> `GET /v1/registry/<sla_hash>`.
>
> Practical effect for the recipes below: every flow now starts with
> `SLA_BYTES=$(curl https://oracle.example.com/v1/registry/$SLA_HASH)` so you
> read the bytes the buyer authored, instead of writing them yourself.

### 4.A. JSON API quality (the most common case)

```bash
ORACLE="https://oracle-api.example.com"
TOKEN="$SELLER_TOKEN"

# 1. Buyer authors and signs the SLA off-band:
#       (a) calls pr402 to obtain a payment_uid for this funding,
#       (b) generates a 32-byte buyer_nonce (optional, recommended),
#       (c) writes the JSON below using your published terms,
#       (d) computes sla_hash = SHA256(bytes) locally,
#       (e) hands the bytes to you (HTTP, IM, S3, anywhere — the bytes are
#           public information once funded; only the order matters).
#
# Buyer-authored sla.json that you receive:
# {
#   "version": 1,
#   "profile_id": "x402/oracles/api-quality/v1",
#   "payment_uid": "<64-hex-payment_uid>",
#   "buyer_nonce": "<64-hex-32-byte-nonce-or-omitted>",
#   "endpoint": "https://my-api.example.com/v1/inference",
#   "method": "POST",
#   "min_status_code": 200,
#   "max_status_code": 299,
#   "max_latency_ms": 5000,
#   "required_fields": ["result"]
# }

# 2. Upload SLA → registry returns sha256. Buyer verifies the returned hash
#    matches their local hash before signing FundPayment.
SLA_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/sla" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq -r .sha256)

echo "Registered SLA: $SLA_HASH"

# 3. Buyer signs FundPayment on-chain (sla_hash + payment_uid + your oracle
#    authority). pr402.build-sla-escrow-payment-tx packages it.

# 4. Buyer's request lands on your service. Run the work.

# 5. Re-fetch the SLA from the registry to read payment_uid + buyer_nonce.
#    The registry is content-addressed and re-hashes on read, so this is
#    safe to trust.
SLA=$(curl -fsS "$ORACLE/v1/registry/$SLA_HASH")
PAYMENT_UID=$(echo "$SLA" | jq -r .payment_uid)
BUYER_NONCE=$(echo "$SLA" | jq -r '.buyer_nonce // empty')

# 6. Capture evidence; echo payment_uid (and buyer_nonce when present) verbatim.
cat > delivery.json <<EOF
{
  "status_code": 200,
  "latency_ms": 240,
  "response_body": {"result": "..."},
  "response_headers": {"content-type": "application/json"},
  "timestamp": $(date +%s),
  "payment_uid": "$PAYMENT_UID"$([ -n "$BUYER_NONCE" ] && echo ",
  \"buyer_nonce\": \"$BUYER_NONCE\"")
}
EOF

# 7. Upload the delivery → registry returns sha256.
DELIVERY_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/delivery" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @delivery.json | jq -r .sha256)

echo "Registered delivery: $DELIVERY_HASH"

# 8. Submit delivery on-chain (the only on-chain action you take).
sla-escrow submit-delivery \
    --seller /path/to/seller-keypair.json \
    --payment-uid "$PAYMENT_UID" \
    --delivery-hash "$DELIVERY_HASH"

# That's it. The oracle picks up the on-chain event, fetches both files
# from the registry, evaluates, and submits ConfirmOracle on-chain. If
# everything matches, the buyer's funds release to you on `release_payment`.
```

### 4.B. On-chain SPL transfer / swap

You **already broadcast** the transfer (that's the deliverable). The
evidence is the transaction signature.

```bash
ORACLE="https://oracle-transfer.example.com"
TOKEN="$SELLER_TOKEN"

# 1. Buyer authors and uploads the SLA bytes off-band (same flow as 4.A:
#    buyer obtains payment_uid from pr402, generates buyer_nonce, hands
#    sla.json to you). The buyer-authored SLA looks like:
# {
#   "version": 1,
#   "profile_id": "x402/oracles/onchain-transfer/v1",
#   "payment_uid": "<64-hex-payment_uid>",
#   "buyer_nonce": "<64-hex-32-byte-nonce-or-omitted>",
#   "cluster": "mainnet",
#   "expected_transfers": [{
#     "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
#     "recipient_owner": "BUYER_WALLET_PUBKEY",
#     "min_amount": "1000000",
#     "direction": "in"
#   }]
# }

SLA_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/sla" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq -r .sha256)

# 2. Re-fetch SLA to read the buyer's payment_uid and buyer_nonce.
SLA=$(curl -fsS "$ORACLE/v1/registry/$SLA_HASH")
PAYMENT_UID=$(echo "$SLA" | jq -r .payment_uid)
BUYER_NONCE=$(echo "$SLA" | jq -r '.buyer_nonce // empty')

# 3. Broadcast the actual transfer — solana-cli, your wallet, whatever.
TX_SIG="$(spl-token transfer ... --output json | jq -r .signature)"

# 4. Author the evidence pointing at the tx. Echo payment_uid (and
#    buyer_nonce if present) verbatim.
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

DELIVERY_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/delivery" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @delivery.json | jq -r .sha256)

# 4. Submit delivery on-chain.
sla-escrow submit-delivery \
    --seller /path/to/seller-keypair.json \
    --payment-uid "$PAYMENT_UID" \
    --delivery-hash "$DELIVERY_HASH"
```

The oracle calls `getTransaction(jsonParsed)` against `tx_signature`,
re-derives the pre/post token deltas, and approves only when the observed
delta meets `min_amount` for the right `(mint, recipient_owner)`. You
cannot fake this — the chain is the ground truth.

> **Token-2022 mints with a transfer fee.** If your mint is owned by
> Token-2022 *and* has a transfer-fee extension configured, the
> recipient receives the **post-fee net** amount, not what you debited.
> The buyer's `min_amount` MUST be the net the recipient will receive;
> the seller broadcasts a slightly higher gross. Mismatched expectations
> here are the most common false-reject cause for Token-2022 mints. See
> [`NORMATIVE §6.1`](../oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md#61-token-2022-transfer-fee-handling)
> for the worked example. Quick check before broadcasting:
>
> ```bash
> spl-token display "$MINT_ADDRESS"
> ```
>
> If the output shows a non-zero `Transfer fee` line, you're in this
> path; otherwise you're on plain SPL Token and `min_amount` equals
> what you debit.

#### Idempotency contract (crash recovery for service-side sellers)

If your seller logic runs inside a long-running service (not a one-shot
shell session) you need to guard against two specific crash points:

1. **Process crashes between the broadcast call returning and the evidence
   POST.** Your service has spent the gas and moved tokens, but the oracle
   has no idea. Naive restart logic re-broadcasts → double-send.
2. **Process crashes between the evidence POST and `submit_delivery`.**
   The registry knows about the delivery hash, but the chain doesn't, so
   the oracle never settles. Naive restart logic re-broadcasts → still
   double-send.

Both failure modes share one root cause: the seller has no durable record
of what `tx_signature` belongs to which `payment_uid`. The fix is one
write to a durable store (your own database — Postgres, SQLite, anything)
**after** the broadcast returns and **before** evidence upload.

The contract:

1. The seller MUST durably persist `(payment_uid, tx_signature, broadcast_at)`
   AFTER `send_and_confirm_transaction` returns AND BEFORE `POST /v1/registry/delivery`.
2. On restart, BEFORE invoking the broadcast logic for a given
   `payment_uid`, the seller MUST first look up the persisted row. If
   present, skip the broadcast and resume from evidence upload. If absent
   AND the buyer's `payment_uid` has any matching transfer on-chain
   (verifiable via `getSignaturesForAddress(seller_pubkey)`-then-`getTransaction`
   filtering by recipient + mint + min_amount), recover that signature
   and persist it; do not re-broadcast.
3. The on-chain `SubmitDelivery` instruction is idempotent at the program
   level — calling it twice with the same `(payment_uid, delivery_hash)`
   is rejected by the program with no state mutation. So a retry of just
   `submit_delivery` after success is safe.
4. **A retry of broadcast + submit_delivery as a sequence is NOT safe**
   without persisted `tx_signature`. Without the durable record, you
   cannot tell whether the previous attempt succeeded mid-flight.

Recommended persistence pattern (Postgres, SQL pseudocode):

```sql
-- Schema:
-- CREATE TABLE seller_payments (
--   payment_uid    BYTEA  PRIMARY KEY,
--   tx_signature   TEXT   NOT NULL,
--   broadcast_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
-- );

-- Insert path: returns a tuple of (current_signature, was_inserted).
-- If was_inserted=true the caller proceeds with broadcast then
-- back-fills tx_signature; if was_inserted=false the caller skips
-- broadcast and resumes from the persisted signature.
INSERT INTO seller_payments (payment_uid, tx_signature)
VALUES ($1, '') -- empty placeholder; caller fills in after broadcast
ON CONFLICT (payment_uid) DO NOTHING;

-- After broadcast succeeds:
UPDATE seller_payments SET tx_signature = $2 WHERE payment_uid = $1
  AND tx_signature = '';
```

Or if your service is single-threaded per `payment_uid` (typical for
order-processing daemons), a simpler pattern is to wrap the whole
broadcast-then-update in a single transaction with `SELECT ... FOR UPDATE`
on a per-uid row.

If your seller logic is shell-based (the recipes in §4 are like this),
you don't need any of this — each shell invocation is a single attempt
and a crash means the buyer eventually refunds via TTL. Idempotency
only matters when you're running a service that retries on its own.

### 4.C. Large file delivery

The file itself **is** the delivery — there's no JSON wrapper. The oracle
streams it from the registry and verifies the SHA-256 chunk-by-chunk.

```bash
ORACLE="https://oracle-file.example.com"
TOKEN="$SELLER_TOKEN"

# 1. Buyer authors and uploads the SLA bytes off-band (size bounds + MIME +
#    payment_uid + optional buyer_nonce). You receive sla.json from the buyer:
# {
#   "version": 1,
#   "profile_id": "x402/oracles/file-delivery/attestation/v1",
#   "payment_uid": "<64-hex-payment_uid>",
#   "buyer_nonce": "<64-hex-32-byte-nonce-or-omitted>",
#   "expected_size_bytes_min": 5242880,
#   "expected_size_bytes_max": 524288000,
#   "expected_mime": "video/mp4"
# }
#
# Note: file-delivery's evidence is the streamed *file itself*, not a JSON
# envelope, so payment_uid / buyer_nonce binding for this profile lives in
# the SLA only. The on-chain sla_hash already commits the SLA bytes
# (including the nonce, by hash), which is enough for cross-SLA replay
# defense at the SLA layer.

SLA_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/sla" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq -r .sha256)

# 2. Stream-upload the file. Up to ORACLE_REGISTRY_MAX_BLOB_BYTES (default 5 GiB).
DELIVERY_HASH=$(curl -fsS -X POST "$ORACLE/v1/registry/blob" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: video/mp4" \
    --data-binary @output.mp4 | jq -r .sha256)

# 3. Submit delivery on-chain.
sla-escrow submit-delivery \
    --seller /path/to/seller-keypair.json \
    --payment-uid "$PAYMENT_UID" \
    --delivery-hash "$DELIVERY_HASH"
```

## 5. What the oracle checks (so you can be sure to pass)

Each profile has a normative spec. Read the relevant one **once**, then
forget it — your SLA implicitly encodes the rules.

| Profile                       | Spec                                                                              |
| ----------------------------- | --------------------------------------------------------------------------------- |
| `x402/oracles/api-quality/v1`         | [`oracle-api-quality/spec/api-quality-v1/NORMATIVE.md`](../oracle-api-quality/spec/api-quality-v1/NORMATIVE.md) |
| `x402/oracles/onchain-transfer/v1`    | [`oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md`](../oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md) |
| `x402/oracles/file-delivery/attestation/v1` | [`oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md`](../oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md) |

The high-level rules:

- **api-quality** approves when status is in range, latency under cap,
  required fields present, schema validates, body length above min.
- **onchain-transfer** approves when the on-chain delta for `(mint,
  recipient_owner)` is at least `min_amount` and the tx is on `cluster`
  with `meta.err == null`.
- **file-delivery** approves when blob size is in `[min, max]` and (if
  set) the sniffed MIME matches `expected_mime`.

If you're rejected, the verdict has a numeric `resolution_reason`. The
per-family READMEs list the codes:

- [`oracle-api-quality/README.md` §codes](../oracle-api-quality/README.md#resolution-reason-codes)
- [`oracle-onchain-transfer/README.md` §codes](../oracle-onchain-transfer/README.md#resolution-reason-codes)
- [`oracle-file-delivery/README.md` §codes](../oracle-file-delivery/README.md#resolution-reason-codes)

## 6. Common pitfalls (and how to avoid them)

**The hash on-chain doesn't match what's in the registry.** This is the
single most common error and it fails closed (oracle never approves).
Cause: you hashed one byte sequence but uploaded a different one — usually
because of trailing newlines, BOM, or pretty-print vs minified JSON. Fix:
**always** use the SHA-256 the registry returned in the upload response as
your on-chain `delivery_hash`. Don't recompute from local bytes.

**`profile_id` mismatch.** Your SLA must declare `profile_id` matching the
oracle's profile (e.g. `x402/oracles/api-quality/v1`). Missing or wrong → instant
reject. Use the examples in §4 verbatim.

**Bearer token expired or revoked.** `401 Unauthorized` from any
`POST /v1/registry/...`. Re-run `seller-register.sh` to get a new token.

**Buyer's `oracle_authority` doesn't match the registry's
`operatorPubkey`.** `pr402` should reject this at advertise time, but if
you bypass pr402 it can happen. Symptom: oracle observes the delivery but
silently doesn't pick it up. Fix: confirm the buyer used the
`oracle_authority` you advertised.

**On-chain transfer evidence references a tx on the wrong cluster.** The
oracle binary serves exactly one cluster (`TRANSFER_CLUSTER` env var).
Your `sla.cluster` must match. Symptom: `Custom(258) ClusterMismatch`.

**File upload size > `ORACLE_REGISTRY_MAX_BLOB_BYTES`.** `413 Payload Too
Large`. Default cap is 5 GiB; check `GET /v1/registry/info` for the
deployment's actual cap.

## 7. Quick sanity check before you ship

Run this five-step checklist for your first integration. If any step
fails, re-read §4 for that scenario.

- [ ] `seller-register.sh` returned a bearer; saved as `$SELLER_TOKEN`.
- [ ] `POST /v1/registry/sla` returned 200 with a 64-hex-char `sha256`.
- [ ] `GET /v1/registry/<that-sha256>` returns your exact SLA bytes.
- [ ] `POST /v1/registry/delivery` (or `/blob`) returned 200 with a `sha256`.
- [ ] You used **that response sha256** as your on-chain `delivery_hash`,
      not a hash you computed yourself.

## 8. FAQ

**Do I need to run my own oracle?** No. Use whichever oracle the buyer's
HTTP-402 advertises. Running your own oracle is a different role
(operator). See [`DEPLOYMENT.md`](DEPLOYMENT.md) for that path.

**What if the oracle is down?** Settlement waits. The chain monitor
catches up via startup backfill when the oracle restarts; you don't lose
the deliverable. If you're worried about a single-oracle dependency,
publish multiple `oracleProfiles[]` in your HTTP-402 challenge — pr402
lets buyers pick.

**What gets stored on-chain?** Hashes only. `sla_hash` and `delivery_hash`
are SHA-256 fingerprints. The actual bytes live in the registry. The
final `resolution_hash` is also a fingerprint — counterparties can
recompute it to verify the verdict.

**Can the oracle see my customer's data?** It can fetch whatever you
upload to the registry. Don't upload secrets you wouldn't share with the
oracle operator. For sensitive workloads, treat the registry as
non-private storage; encrypt at the application layer if needed.

**Why does the registry need a bearer? Why not just upload anonymously?**
To prevent registry pollution and to bind uploads to the seller wallet
for audit. The bearer is one-time setup; you keep it indefinitely.

**Can I rotate my token without losing in-flight payments?** Yes. Old and
new tokens both work briefly during rotation; settle in-flight uploads
on either. The on-chain `payment_uid` flow is independent of which
bearer you used to upload.

**Where can I see my history?** Hit `GET /stats` and `GET /health` on the
oracle (no auth required). For verdict-level history, ask the operator
for a query against `oracle_jobs` and `oracle_verdicts` filtered by your
seller bearer id (operator policy).

---

## Appendix — minimal seller-register helper

A small `seller-register.sh` is shipped at
[`oracles/scripts/seller-register.sh`](../scripts/seller-register.sh).
Source code is roughly 30 lines and uses only `curl`, `jq`, and the
`solana` CLI for keypair signing. Read it once to understand the flow,
then use it forever.

If you prefer TypeScript / Python:

```ts
// minimal TypeScript sketch
const { challenge } = await fetch(
  `${oracle}/v1/registry/seller/challenge?wallet=${wallet.publicKey.toBase58()}`
).then(r => r.json());

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
