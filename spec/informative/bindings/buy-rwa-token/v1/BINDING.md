# Buy RWA Token — Reference Binding (Informative)

**Document status:** Informative — **not normative**.
**Binding identifier:** `x402/informative/bindings/buy-rwa-token/v1`
**Implements:** Layer 1 `delegated-authoring/v1` + Layer 2
`x402/oracles/rwa-transfer/v1`.

> Normative rules: `x402/delegated-authoring/v1`, `x402/oracles/rwa-transfer/v1`.
> This document is a concrete binding for a planned open-source RWA primary
> reference seller (`x402-buy-rwa-token`). It does **not** modify
> `x402/informative/bindings/buy-spl-token/v1` or
> `x402/oracles/onchain-transfer/v1`.
> Production deployments conform to Layer 0–1; they **MAY** differ in wire
> names if their intent contract declares them.

---

## 1. Purpose

Demonstrates **RWA primary subscription** on the sla-escrow rail:

- Investor locks **USDC** (classic SPL) in sla-escrow — payment leg.
- Issuer delivers **RWA Token-2022** tokens to the investor — deliverable leg.
- **KYC / qualification** is enforced **off-chain** (centralized audit) and
  **on-chain** (Transfer Hook + KYC PDA) — **outside** sla-escrow.
- Deliverable verified by `rwa-transfer/v1` oracle (planned:
  `oracle-rwa-transfer` binary).

Commit variant: **`buyer-commit`**.

Target audience: RWA issuers, transfer agents, buyer agents, oracle integrators.

**Not a securities opinion.** This binding describes technical integration only.

---

## 2. Architectural separation (design intent)

| Concern | Where it lives |
|---|---|
| Investor KYC / AML / offering docs | Off-chain issuer portal + auditors |
| On-chain hold/transfer eligibility | Token-2022 **Transfer Hook** + KYC PDA program |
| Subscription payment (USDC) | **sla-escrow** (`FundPayment` / `ReleasePayment` / `RefundPayment`) |
| Delivery proof | **`rwa-transfer/v1`** oracle (`ConfirmOracle`) |

sla-escrow **MUST NOT** be extended for Token-2022 compliance. The escrow
mint remains USDC (or another stable settlement asset on classic SPL).

---

## 3. Endpoint

```http
GET /api/v1/buy-rwa-token
```

Unpaid → HTTP 402. Paid → `PAYMENT-SIGNATURE` header (x402 v2).

---

## 4. Phased investor journey (informative)

These phases are **separate concerns**. Only Phase 5 uses this binding's
endpoint. Earlier phases **MAY** use other x402 sellers (`exact` or
`sla-escrow`) or off-chain flows.

| Phase | Activity | Typical x402 component |
|---|---|---|
| 0 | Optional wallet risk screen | `exact` — e.g. solrisk `GET /api/v1/wallet-risk` |
| 1 | KYC / AML / offering qualification | Off-chain issuer portal |
| 2 | Write KYC result on-chain | KYC hook program → investor KYC PDA |
| 3 | Optional multi-party attestation badge | Off-chain airdrop + optional `exact` balance check |
| 4 | Optional document attestation | `sla-escrow` + `file-delivery/attestation/v1` (separate payment) |
| 5 | **Primary subscription (this binding)** | `GET /api/v1/buy-rwa-token` + `rwa-transfer/v1` |

Phases 0–4 **SHOULD** complete before Phase 5. The Token-2022 Transfer Hook
runs at delivery time; an unqualified `recipient_owner` causes transfer
failure → oracle reject → USDC refund path.

---

## 5. Intent contract summary

| Declaration | Value |
|---|---|
| `profileId` | `x402/oracles/rwa-transfer/v1` |
| `commitVariant` | `buyer-commit` |
| `serializationRecipeId` | `x402/canonical-json/v1` |
| **Escrow terms** | `accepts[].asset` = USDC mint; `accepts[].amount` = session total in USDC raw units (6 decimals) |
| **Deliverable** | Transfer `tokenPriceUnits` raw units of Token-2022 `tokenMint` to `recipient_owner` on `cluster`, satisfying hook/KYC rules on-chain |

### 5.1 Intent parameters (buyer / investor)

| Name | Location | Type | Required | Semantics |
|---|---|---|---|---|
| `offering` | query | string | yes | Catalog offering id or RWA mint pubkey |
| `recipient_owner` | query | pubkey-base58 | yes | Destination wallet (ATA owner). **MUST** be the KYC-qualified wallet |
| `quantity` | query | decimal string | yes | Offering units to subscribe (seller maps to raw token amount) |
| `buyer_nonce` | query | hex-64 | yes | 32-byte entropy; SLA uniqueness |

### 5.2 Seller context (commit material)

Returned under `accepts[].extra.commitMaterial` (recommended layout):

| Key | Type | Maps to SLA |
|---|---|---|
| `offeringId` | string | `offering_id` |
| `tokenMint` | base58 | `expected_transfers[].mint` |
| `tokenDecimals` | integer | `expected_transfers[].decimals` |
| `tokenProgram` | base58 | `token_program` |
| `transferHookProgram` | base58 | `transfer_hook_program` |
| `tokenPriceUnits` | decimal string | `expected_transfers[].min_amount` |
| `recipientOwner` | base58 | `expected_transfers[].recipient_owner` |
| `buyerNonce` | hex-64 | `buyer_nonce` |
| `sellerPubkey` | base58 | `expected_transfers[].sender_owner` |
| `cluster` | string | `cluster` |
| `profileId` | string | `profile_id` |
| `version` | integer | `version` |
| `kycPrerequisites` | object | Informative only — not hashed into SLA |

`kycPrerequisites` **MAY** include human-readable pointers (portal URL,
required badge mint, jurisdiction). It is **not** part of `B_sla`.

Buyer supplies at commit time: `payment_uid` (hex-64) → SLA `payment_uid`.

### 5.3 Oracle advertisement

Sellers **SHOULD** advertise a dedicated RWA oracle authority:

```json
"extra": {
  "oracleAuthorities": ["<rwa-oracle-pubkey>"],
  "oracleProfiles": [{
    "profileId": "x402/oracles/rwa-transfer/v1",
    "operatorPubkey": "<rwa-oracle-pubkey>",
    "normativeSpecUrl": "https://…/rwa-transfer/v1/NORMATIVE.md",
    "registryBaseUrl": "https://oracle-rwa.example"
  }]
}
```

Do **not** reuse `onchain-transfer/v1` profile rows for RWA offerings unless
the operator explicitly serves both profiles from distinct binaries.

---

## 6. SLA shape (after serialization)

Logical content (`rwa-transfer/v1` schema is authoritative):

```json
{
  "buyer_nonce": "<hex-64>",
  "cluster": "devnet",
  "expected_transfers": [{
    "decimals": 6,
    "direction": "in",
    "min_amount": "<tokenPriceUnits>",
    "mint": "<tokenMint>",
    "recipient_owner": "<recipientOwner>",
    "sender_owner": "<sellerPubkey>"
  }],
  "offering_id": "<offeringId>",
  "payment_uid": "<hex-64>",
  "profile_id": "x402/oracles/rwa-transfer/v1",
  "token_program": "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
  "transfer_hook_program": "<transferHookProgram>",
  "version": 1
}
```

Bytes = `x402/canonical-json/v1` over this value tree.

---

## 7. Primary subscription flow (Phase 5)

1. Unpaid GET with intent params → 402 + commit material + pr402 extras.
2. Buyer builds SLA, `sla_hash`, calls pr402 `build-sla-escrow-payment-tx`.
3. Buyer signs `FundPayment` (USDC → escrow), retries GET with
   `PAYMENT-SIGNATURE`.
4. Seller verifies hash, uploads SLA, builds Token-2022 `TransferChecked`
   (with hook extra accounts), broadcasts transfer.
5. **Transfer Hook** CPI validates investor KYC PDA. Failure → tx fails →
   seller **MUST NOT** `SubmitDelivery`.
6. On success, seller uploads delivery evidence, `SubmitDelivery`.
7. `oracle-rwa-transfer` evaluates → `ConfirmOracle`.
8. Settlement keeper / `ReleasePayment` → USDC to issuer merchant wallet.

**Refund path:** hook failure, oracle reject, or TTL expiry →
`RefundPayment` to investor (`payment.buyer`).

---

## 8. Invariants

1. `recipient_owner` **MUST** equal the wallet tagged by the KYC hook program.
2. Escrow asset (USDC) and deliverable asset (RWA Token-2022) **MUST** use
   different mints and may use different token programs.
3. Sellers **MUST NOT** call `SubmitDelivery` until the delivery transaction
   is confirmed successful on-chain.
4. This binding **MUST NOT** be served from the `buy-spl-token` endpoint;
   use a distinct path and `profile_id` so reference SPL demos stay stable.

---

## 9. Worked example (devnet, illustrative)

```http
GET /api/v1/buy-rwa-token?offering=series-a&recipient_owner=4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU&quantity=100&buyer_nonce=abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789
```

---

## 10. References

| Reference | Purpose |
|---|---|
| `x402/delegated-authoring/v1` | Abstract HTTP 402 pattern |
| `x402/oracles/rwa-transfer/v1` | RWA delivery verdict rules |
| `x402/oracles/onchain-transfer/v1` | Baseline transfer semantics (superset reference) |
| `x402/informative/bindings/buy-spl-token/v1` | Generic SPL reference (unchanged sibling) |
| `x402/pr402-discovery/v1` | Build + 402 wire |

---

**Document version:** v1.0
