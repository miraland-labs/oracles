# Buyer Integration Guide

You're a buyer. A seller is asking you to pay for something via
`sla-escrow`, and the seller's HTTP-402 challenge mentions an oracle.
This guide tells you how to pick the right oracle and fund the escrow.

> **Normative reference.** Buyer obligations are specified in
> [`spec/sla-escrow-protocol/v1`](../spec/sla-escrow-protocol/v1/NORMATIVE.md)
> §3. SLA envelope rules in
> [`spec/sla-document/v1`](../spec/sla-document/v1/NORMATIVE.md). Wire-level
> registry calls in
> [`spec/registry-http-api/v1`](../spec/registry-http-api/v1/NORMATIVE.md).
> The on-chain `FundPayment` (and optional `RefundPayment` /
> `ReleasePayment`) instruction bytes are specified in
> [`spec/sla-escrow-onchain-abi/v1`](../spec/sla-escrow-onchain-abi/v1/NORMATIVE.md) —
> useful when integrating from a non-Rust language. This guide gives
> recipes that follow those specs.

## 30-second summary

When a seller advertises `scheme: "v2:solana:sla-escrow"`, they list
oracles in `accepts[].extra.oracleProfiles[]`. You pick one (or accept
the seller's default) and pass its `operatorPubkey` as
`oracle_authority` when funding. The chosen oracle decides the verdict.

## Two authoring patterns

You'll do one of these (per spec §3.1):

**Direct authoring.** You construct the SLA JSON locally, hash it,
transmit the bytes to the seller. Use this when your tooling can build
profile-conforming SLAs.

**Delegated authoring (HTTP 402).** You send intent-bearing parameters
to the seller, receive a 402 with `accepts[].extra.slaHash` already
computed by the seller, sign `FundPayment` with that hash. Use this for
x402-paid HTTP services where your client doesn't assemble per-profile
JSON.

Both patterns are equally safe on the funds dimension. Correctness comes
from the on-chain commit binding funds to a specific hash, regardless
of who computed it (spec §3.5).

## 1. The 402 challenge

```json
{
  "x402Version": 2,
  "accepts": [{
    "scheme": "v2:solana:sla-escrow",
    "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "maxAmountRequired": "1000000",
    "payTo": "<escrow-pda>",
    "extra": {
      "escrowProgramId": "SEsc...",
      "bankAddress": "Bank...",
      "oracleAuthorities": ["OracLe1...", "OracLe2..."],
      "oracleProfiles": [
        { "profileId": "x402/oracles/api-quality/v1",
          "operatorPubkey": "OracLe1...",
          "registry": "https://oracle.example.com/v1/registry" }
      ],
      "slaHash": "<64-hex>",
      "paymentUidHex": "<64-hex>"
    }
  }]
}
```

`extra.slaHash` and `extra.paymentUidHex` are present in the delegated
flow only.

## 2. Pick the oracle

Rules of thumb:

- **Profile must match the delivery category.** JSON HTTP response →
  `api-quality/v1`. SPL transfer → `onchain-transfer/v1`. Large file →
  `file-delivery/attestation/v1`.
- **One oracle per payment.** The on-chain `Payment` binds one
  `oracle_authority`; not changeable later.
- **If multiple oracles match the profile**, prefer one you've used,
  one with lower fetch latency to the seller, or the first listed
  (seller's stated preference).

Before funding, check the oracle's `/v1/policy` and `/health`:

```bash
curl -s "$ORACLE/v1/policy" | jq '.tipFloorEnabled, .registeredProfiles, .minVerdictTipDefaultRaw'
curl -s "$ORACLE/health"
```

`registeredProfiles` is a single-element array for any one binary.

## 3. Fund the escrow

### 3.1 Direct authoring path

```bash
PR402="https://ipay.sh"
BUYER_PUBKEY="<your-wallet>"
ACCEPTED='<the JSON object from accepts[0]>'
RESOURCE='<the JSON value of "resource">'

# 0. Prepare buyer-controlled fields.
PAYMENT_UID="$(openssl rand -hex 32)"
BUYER_NONCE="$(openssl rand -hex 32)"

# 1. Author the SLA. Shape per profile (see SELLER_GUIDE or per-family NORMATIVE.md).
cat > sla.json <<EOF
{
  "version": 1,
  "profile_id": "x402/oracles/api-quality/v1",
  "payment_uid": "$PAYMENT_UID",
  "buyer_nonce": "$BUYER_NONCE",
  "endpoint": "https://seller.example.com/v1/inference",
  "method": "POST",
  "min_status_code": 200,
  "max_status_code": 299,
  "max_latency_ms": 5000,
  "required_fields": ["result"]
}
EOF

# 2. Hash locally over the exact bytes you will send to the seller.
SLA_HASH="$(shasum -a 256 sla.json | awk '{print $1}')"

# 3. Hand sla.json bytes to the seller. Seller uploads to the oracle's
#    registry with their bearer; the registry returns the SHA-256.

# 4. SHOULD: confirm the registry actually has the bytes (spec §3.2 #1).
curl -fsSI "$ORACLE/v1/registry/$SLA_HASH" -o /dev/null \
  || { echo "registry HEAD failed; abort"; exit 1; }

ORACLE_AUTHORITY="$(echo "$ACCEPTED" | jq -r .extra.oracleProfiles[0].operatorPubkey)"

# 5. Build, sign, and submit FundPayment.
BUILD_BODY=$(jq -n \
  --arg payer "$BUYER_PUBKEY" \
  --argjson accepted "$ACCEPTED" \
  --argjson resource "$RESOURCE" \
  --arg slaHash "$SLA_HASH" \
  --arg oracleAuthority "$ORACLE_AUTHORITY" \
  --arg paymentUidHex "$PAYMENT_UID" \
  '{payer:$payer, accepted:$accepted, resource:$resource,
    slaHash:$slaHash, oracleAuthority:$oracleAuthority,
    paymentUidHex:$paymentUidHex}')

UNSIGNED=$(curl -fsS -X POST "$PR402/api/v1/facilitator/build-sla-escrow-payment-tx" \
    -H "Content-Type: application/json" -d "$BUILD_BODY")

echo "$UNSIGNED" | jq -r .transactionBase64 | base64 -d > /tmp/tx.unsigned
solana sign-and-submit /tmp/tx.unsigned --keypair /path/to/buyer-keypair.json \
    --url mainnet-beta
```

### 3.2 Delegated authoring path (HTTP 402)

The seller hands you `slaHash` and `paymentUidHex` in the 402 response.
Your `BUILD_BODY` then contains the seller-supplied values verbatim:

```bash
SLA_HASH="$(echo "$ACCEPTED" | jq -r .extra.slaHash)"
PAYMENT_UID="$(echo "$ACCEPTED" | jq -r .extra.paymentUidHex)"
ORACLE_AUTHORITY="$(echo "$ACCEPTED" | jq -r .extra.oracleProfiles[0].operatorPubkey)"
# ...same BUILD_BODY + sign + submit as above.
```

**Verify intent before signing** (spec §3.2 #2). Three options in
increasing strength:

1. Trust the seller's published template.
2. Recompute locally from the same parameters and the seller's
   documented template (recommended for high-value).
3. Fetch the SLA bytes from the registry post-funding and compare
   against your intent — if it diverges, the buyer can self-refund
   pre-delivery (subject to cooldown).

## 4. After payment

You don't do anything else unless settlement needs your action. The
flow continues server-side: seller delivers, oracle adjudicates, anyone
can call `ReleasePayment` post-approval (spec §7).

If the oracle rejects (`Payment.resolution_state == 2`), refund is
**permissionless** post-rejection — any signer can trigger it,
including you. The cooldown is waived once a rejection is recorded.

If the payment expires without a verdict, refund is also permissionless
(after `expires_at`). Any signer can trigger.

```bash
sla-escrow refund-payment --payment-uid "$PAYMENT_UID" \
  --keypair /path/to/buyer-keypair.json --url mainnet-beta
```

**Pre-outcome refund** (you change your mind before the oracle has
ruled) requires you to wait for `Config.refund_cooldown_seconds` to
elapse since funding. Read the cooldown live from chain — don't
hard-code. The current pr402 deployment runs at 24h.

## 5. Verifying the verdict

Optional but recommended for high-value flows. The on-chain
`Payment.resolution_hash` is a deterministic SHA-256 over a canonical
envelope; anyone holding SLA + delivery bytes can recompute and
confirm. See `oracle-common::settler` for the implementation.

If they don't match, the oracle has tampered. Recourse is off-chain
operator reputation; the protocol does not currently support on-chain
dispute.

## 6. When `oracleProfiles[]` is missing

Some sellers list only the legacy `oracleAuthorities[]`. Defaults to
assume:

- If the resource looks like an HTTP API, assume `api-quality/v1`.
- Hit `GET <pr402>/api/v1/facilitator/capabilities` for the
  deployment's advertised SLA-escrow profile list.

If you can't determine the profile confidently, **don't fund** — ask
the seller or pick a different one.

## 7. Common pitfalls

**Wrong `oracleAuthority`.** Most common error. The pubkey must be in
`accepted.extra.oracleAuthorities[]`. pr402 returns `400` on mismatch.

**`slaHash` mismatch.** Direct authoring: hash over the exact bytes
the seller will upload, no re-serialization. Delegated authoring:
verify intent (spec §3.2 #2).

**Profile mismatch.** Seller advertises `api-quality/v1`, buyer picks
an `onchain-transfer` operator. `FundPayment` succeeds but the oracle
silently ignores it (no matching profile). Escrow stays stuck until
expiry.

**Wrong cluster.** `onchain-transfer` binaries are cluster-pinned. Tx
on the wrong cluster gets `TRANSFER_CLUSTER_MISMATCH` (code 261).
pr402 doesn't catch this — confirm with the seller.

**Ignoring `/health`.** If `chain_connected` or `websocket_connected`
is false, the oracle isn't processing. Defer or pick another operator.

## 8. Quick sanity check before funding

- [ ] `accepted.scheme == "v2:solana:sla-escrow"`.
- [ ] `accepted.extra.oracleAuthorities[]` is non-empty and includes
      your chosen `oracleAuthority`.
- [ ] `oracleProfiles[]` entry's `profileId` matches the resource.
- [ ] Oracle's `/health` returns 200 with `chain_connected=true` and
      `websocket_connected=true`.
- [ ] Oracle's `/v1/policy` is acceptable (tip floor, registered
      profile).
- [ ] (Direct authoring) `HEAD /v1/registry/<slaHash>` returns 200.
- [ ] (Delegated authoring) `slaHash` reflects your intent (recompute
      or post-fund verify).

## 9. FAQ

**Why does pr402 only enforce `oracleAuthorities[]` and not the
profile?** Profiles are off-chain metadata; the on-chain
`FundPayment.oracle_authority` is what matters for `ConfirmOracle`.
pr402 enforces the on-chain identity match; profile enforcement is the
buyer's responsibility.

**Oracle goes down between payment and verdict.** `Payment.expires_at`
protects you. If the deadline passes without a verdict, refund is
permissionless. Choose oracles with stable operators.

**Can I switch oracles after funding?** No. `oracle_authority` is
permanently bound at funding. Wait for expiry and refund.

**Does the oracle see my customer data?** Whatever the seller uploads.
If the deliverable is sensitive, talk to the seller about end-to-end
encryption (application-layer; not part of v1).

**Why three different oracles instead of one?** Each oracle binary
registers exactly one profile (one keypair per profile, blast-radius
isolation). You don't need to care — pick by profile.

**Where do I see verdict history?** On-chain via the `Payment` PDA's
`resolution_state` / `resolution_hash`. Oracle's `GET /stats` shows
aggregate counters only.
