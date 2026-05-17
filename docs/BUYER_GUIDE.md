# Buyer Integration Guide

You're a buyer. A seller is asking you to pay for something via
`sla-escrow`, and the seller's HTTP-402 challenge mentions an **oracle**.
This guide tells you, in plain language, how to pick the right oracle and
fund the escrow.

> **30-second summary.** When a seller advertises `scheme:
> "v2:solana:sla-escrow"`, they list one or more oracles in
> `accepts[].extra.oracleProfiles[]`. You **pick** one of those oracles
> (or accept the seller's default), pass its `operatorPubkey` as
> `oracle_authority` when funding the escrow, and the chosen oracle
> decides the verdict. That's it.

If you've used `pr402` for the `exact` rail, you already know the
mechanics. SLA-escrow adds **two extra fields** to the build call:
`slaHash` and `oracleAuthority`.

---

## 1. The 402 challenge — what to look for

When you fetch a paywalled resource and get HTTP 402, the body looks
like:

```json
{
  "x402Version": 2,
  "accepts": [
    {
      "scheme": "v2:solana:sla-escrow",
      "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
      "maxAmountRequired": "1000000",
      "payTo": "<seller-or-escrow-pda>",
      "resource": "https://seller.example.com/api/inference",
      "extra": {
        "escrowProgramId": "Escr...",
        "bankAddress": "Bank...",
        "oracleAuthorities": ["OracLe1...", "OracLe2..."],
        "oracleProfiles": [
          {
            "profileId": "x402/oracle/api-quality/v1",
            "operatorPubkey": "OracLe1...",
            "registry": "https://oracle-api.example.com/v1/registry"
          },
          {
            "profileId": "x402/oracle/api-quality/v1",
            "operatorPubkey": "OracLe2...",
            "registry": "https://oracle-api.alt.example.com/v1/registry"
          }
        ]
      }
    }
  ],
  "resource": { "...": "..." }
}
```

Three fields matter for oracle selection:

| Field                             | Meaning                                                         |
| --------------------------------- | --------------------------------------------------------------- |
| `extra.oracleAuthorities[]`       | The pubkeys the seller authorizes (mirror of `oracleProfiles`). |
| `extra.oracleProfiles[]`          | What each oracle does, where its registry lives.                |
| `extra.oracleProfiles[].profileId`| Which delivery category (api-quality, onchain-transfer, file-delivery). |

If `oracleProfiles[]` is missing or empty, the seller hasn't
formalized the oracle choice — see [§6](#6-when-the-seller-doesnt-advertise-oracleprofiles).

## 2. Pick the right oracle

Use these rules of thumb:

1. **Profile must match the delivery category**. If the resource
   returns a JSON response, you want `x402/oracle/api-quality/v1`. If the
   seller is delivering an SPL transfer, `x402/oracle/onchain-transfer/v1`. If
   it's a large file, `x402/oracle/file-delivery/attestation/v1`. The seller
   normally only advertises profiles compatible with what they sell.
2. **One oracle per payment**. The on-chain `Payment` binds **one**
   `oracle_authority`. Pick before funding; you cannot change later.
3. **If multiple oracles are advertised for the same profile**, prefer:
   - One you have prior good experience with.
   - One whose registry and operator you trust more.
   - One closer to the seller's region (lower fetch latency → faster
     settlement).
   - In a tie, the **first** listed entry — that's the seller's
     stated preference.
4. **Default fall-through**: if you need a tiebreaker and don't know
   the operators, hit `GET https://<your-pr402>/api/v1/facilitator/capabilities`
   and see if it advertises a `slaEscrowDefaultOracle` for the profile
   — that's the deployment's recommended default.

For most simple buyers: pick `oracleProfiles[0].operatorPubkey` and
move on.

## 3. Fund the escrow via pr402

This is the only on-chain action you take. Two HTTP calls plus one
signature.

```bash
PR402="https://ipay.sh"   # or your trusted facilitator
BUYER_PUBKEY="<your-wallet>"
ACCEPTED='<the JSON object from accepts[0]>'        # paste verbatim
RESOURCE='<the JSON value of "resource">'           # paste verbatim
SLA_HASH="$(shasum -a 256 sla.json | awk '{print $1}')"
ORACLE_AUTHORITY="$(echo "$ACCEPTED" | jq -r .extra.oracleProfiles[0].operatorPubkey)"

# 1. Ask pr402 to build the unsigned FundPayment transaction.
BUILD_BODY=$(jq -n \
  --arg payer "$BUYER_PUBKEY" \
  --argjson accepted "$ACCEPTED" \
  --argjson resource "$RESOURCE" \
  --arg slaHash "$SLA_HASH" \
  --arg oracleAuthority "$ORACLE_AUTHORITY" \
  '{payer:$payer, accepted:$accepted, resource:$resource, slaHash:$slaHash, oracleAuthority:$oracleAuthority}')

UNSIGNED=$(curl -fsS -X POST "$PR402/api/v1/facilitator/build-sla-escrow-payment-tx" \
    -H "Content-Type: application/json" \
    -d "$BUILD_BODY")

# 2. Sign the returned base64 transaction with your wallet (CLI example).
echo "$UNSIGNED" | jq -r .transactionBase64 \
    | base64 -d > /tmp/tx.unsigned
solana sign-and-submit /tmp/tx.unsigned \
    --keypair /path/to/buyer-keypair.json \
    --url mainnet-beta

# 3. Settle via pr402 (fills verifyBodyTemplate, then /verify + /settle).
#    The build response carries `verifyBodyTemplate` — fill its
#    `paymentPayload.payload.transaction` field with your signed base64
#    and POST the whole template to /verify and /settle.
```

If you use `pr402-buy` (the buyer-starter CLI), the four steps above
collapse into a single command. See
[`x402-buyer-starter`](../../x402-buyer-starter/) for the wrapper.

> **What pr402 enforces for you.** Before building the TX, pr402 checks
> that your `oracleAuthority` is in `accepted.extra.oracleAuthorities[]`.
> If it isn't, the build call returns `400` and you don't waste a
> blockhash. This is your first line of defense against typos.

## 4. After payment — what happens next

You don't do anything else. The flow continues server-side:

1. The seller produces the deliverable.
2. The seller uploads the SLA + delivery (or blob) to the oracle's
   registry, getting back hashes.
3. The seller calls `submit_delivery` on-chain with the
   `delivery_hash`.
4. The oracle observes the on-chain event, fetches the SLA + delivery
   from its registry, evaluates, and submits `confirm_oracle` with
   approve / reject.
5. If approved, the funds release to the seller (via `release_payment`,
   which any party can call).
6. If rejected, you can call `refund_payment` once the cooldown
   elapses.

You can monitor the payment's state via the on-chain `Payment` PDA or
by hitting the oracle's `GET /health` and `GET /stats` (no auth) for a
high-level "is the oracle alive and processing".

## 5. Verifying the verdict (optional but recommended for high-value)

The on-chain `Payment.resolution_hash` is a deterministic SHA-256
fingerprint of the verdict. Anyone holding the SLA + delivery bytes can
recompute it and confirm the oracle didn't lie. The recipe lives in
[`design.md` §Resolution Hash](../../.kiro/specs/multi-category-oracle-architecture/design.md)
and the property test
[`oracle-common/tests/cross_family_properties.rs`](../oracle-common/tests/cross_family_properties.rs)
proves the determinism.

For most buyers this is unnecessary. For high-value or auditable flows:

```bash
# Pseudo-recipe; full implementation in oracle-common::settler.
# Re-fetch the SLA + delivery you saved.
# Recompute compute_resolution_hash(...) per the canonical envelope.
# Compare against the on-chain Payment.resolution_hash.
```

If they don't match, the oracle has tampered. File a dispute via the
operator's contact channel; on-chain refund is governed by the
`sla-escrow` program's expiry rules.

## 6. When the seller doesn't advertise `oracleProfiles[]`

Some sellers list only the legacy `oracleAuthorities[]` array without
the richer `oracleProfiles[]`. That's fine for a single canonical
profile — you have to know the profile out-of-band. Defaults to assume:

- If the seller advertises `oracleAuthorities[]` and the resource looks
  like an HTTP API, assume `x402/oracle/api-quality/v1`.
- Hit `GET https://<your-pr402>/api/v1/facilitator/capabilities` →
  `slaEscrowOracleProfiles[]` is the deployment's advertised profile list
  for SLA-escrow.

If you can't determine the profile confidently, **don't fund the
escrow** — ask the seller or pick a different one.

## 7. Common pitfalls

**Wrong `oracleAuthority`.** The most common buyer error.
`oracleAuthority` you pass to `build-sla-escrow-payment-tx` **must** be
in `accepted.extra.oracleAuthorities[]`. pr402 catches typos at build
time with a clear `400` error.

**`slaHash` mismatch.** You computed the hash, the seller computed a
different hash. Always use the SHA-256 of **the exact bytes the seller
will upload** (or that the seller's reference SLA file shows). Don't
re-serialize.

**Profile mismatch.** Seller advertises `x402/oracle/api-quality/v1`, but
buyer picked an `oracle-onchain-transfer` operator pubkey. The on-chain
`FundPayment` succeeds but the chosen oracle silently ignores it (it
won't dispatch a non-matching profile). The escrow stays stuck until
expiry. Fix: only pick an `operatorPubkey` whose `profileId` matches the
seller's resource.

**Funding for the wrong cluster.** `oracle-onchain-transfer` binaries
are pinned to one cluster (`mainnet` / `devnet` / `testnet`). If
your tx is on the wrong cluster vs the oracle's, you'll get
`Custom(258) ClusterMismatch`. pr402 doesn't catch this — confirm with
the seller.

**Ignoring the oracle's `/health`.** Before paying, hit
`GET https://<oracle-host>/health`. If `chain_connected: false` or
`websocket_connected: false`, the oracle isn't processing right now —
defer or pick a different operator.

## 8. Quick sanity check before you fund

- [ ] `accepted.scheme` == `"v2:solana:sla-escrow"`.
- [ ] `accepted.extra.oracleAuthorities[]` is non-empty.
- [ ] Your chosen `oracleAuthority` is in that array.
- [ ] (If `oracleProfiles[]` is present) the matching entry's
      `profileId` is what you expect.
- [ ] The chosen oracle's `/health` returns `200` with
      `chain_connected=true` and `websocket_connected=true`.
- [ ] The seller's `slaHash` matches what you'd compute over the bytes
      they handed you.

## 9. FAQ

**Why does pr402 only enforce `oracleAuthorities[]` and not the
profile?** Profiles are off-chain metadata; the on-chain
`FundPayment.oracle_authority` is what matters for `confirm_oracle`.
pr402 enforces the on-chain identity match; profile enforcement is the
buyer's responsibility (helped by the docs above).

**What if the oracle goes down between payment and verdict?** The on-chain
`Payment.expires_at` protects you. If the deadline passes without a
verdict, you can refund. Choose oracles with stable operators and
publish-known SLAs around uptime if it matters.

**Can I switch oracles after funding?** No. The on-chain
`oracle_authority` is committed at FundPayment. If the chosen oracle
goes silent, your only path is to wait for expiry and refund.

**Does the oracle see my customer data?** Whatever the seller uploads
to the oracle's registry. The oracle's operator can read those bytes.
If the deliverable is sensitive, talk to the seller about
end-to-end encryption (which is application-layer, not part of v1).

**Why are there three different oracles instead of one?** Different
delivery shapes (JSON / on-chain tx / large file) need different
verification logic. Each oracle binary registers exactly one profile;
the architecture is one keypair per oracle for blast-radius isolation.
You don't need to care — just pick by profile.

**Where do I see verdict history for my payments?** On-chain via the
`Payment` PDA's `resolution_state` and `resolution_hash`. The oracle's
public `GET /stats` shows aggregate counters but not per-payment
detail (operator policy gates that).

---

## Appendix — What pr402 does for you under the hood

When you POST to `/build-sla-escrow-payment-tx`, pr402:

1. Validates `slaHash` is 64 hex chars.
2. Parses `oracleAuthority` as a Solana pubkey.
3. Reads `accepted.extra.oracleAuthorities[]`, asserts the parsed
   `oracleAuthority` is one of them. **400** otherwise.
4. Resolves the seller wallet from `accepted.payTo` / `extra`.
5. Resolves the escrow + bank PDAs from the program's seeds.
6. Builds an unsigned `FundPayment` instruction wrapped in a versioned
   transaction with the right compute-budget config.
7. Returns the unsigned TX + `verifyBodyTemplate` you fill with your
   signed bytes.

You sign and submit. `verifyBodyTemplate` carries the same payload
shape pr402's `/verify` and `/settle` accept, so the next two calls
are mechanical.
