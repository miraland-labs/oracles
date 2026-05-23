# Buy SPL Token — Reference Binding (Informative)

**Document status:** Informative — **not normative**.
**Binding identifier:** `x402/informative/bindings/buy-spl-token/v1`
**Implements:** Layer 1 `delegated-authoring/v1` + Layer 2
`x402/oracles/onchain-transfer/v1`.

> Normative rules: `x402/delegated-authoring/v1`.
> This document is a concrete binding for an open-source reference seller.
> Production deployments conform to Layer 0–1; they **MAY** differ in wire
> names if their intent contract declares them.

---

## 1. Purpose

Demonstrates a complete sla-escrow HTTP seller flow:

- Buyer purchases a catalogued SPL token for USDC escrow principal.
- Deliverable verified by `onchain-transfer/v1` oracle.
- Commit variant: **`buyer-commit`**.

Target audience: seller authors, buyer agents, oracle integrators learning
the rail without reading closed-source code.

---

## 2. Endpoint

```http
GET /api/v1/buy-spl-token
```

Unpaid → HTTP 402. Paid → `PAYMENT-SIGNATURE` header (x402 v2).

---

## 3. Intent contract summary

| Declaration | Value |
|---|---|
| `profileId` | `x402/oracles/onchain-transfer/v1` |
| `commitVariant` | `buyer-commit` |
| `serializationRecipeId` | `x402/canonical-json/v1` |
| **Escrow terms** | `accepts[].asset` = USDC mint; `accepts[].amount` = catalog price in USDC raw units (6 decimals) |
| **Deliverable** | Transfer `tokenPriceUnits` raw units of `tokenMint` to `recipient_owner` on `cluster` |

### 3.1 Intent parameters (buyer)

| Name | Location | Type | Required | Semantics |
|---|---|---|---|---|
| `token` | query | string | yes | Catalog product id or mint pubkey |
| `recipient_owner` | query | pubkey-base58 | yes | Destination wallet (ATA owner) for delivered SPL tokens |
| `buyer_nonce` | query | hex-64 | yes | 32-byte entropy; SLA uniqueness |

### 3.2 Seller context (commit material)

Returned under `accepts[].extra.commitMaterial` (recommended layout):

| Key | Type | Maps to SLA |
|---|---|---|
| `tokenMint` | base58 | `expected_transfers[].mint` |
| `tokenDecimals` | integer | `expected_transfers[].decimals` |
| `tokenPriceUnits` | decimal string | `expected_transfers[].min_amount` |
| `recipientOwner` | base58 | `expected_transfers[].recipient_owner` |
| `buyerNonce` | hex-64 | `buyer_nonce` |
| `sellerPubkey` | base58 | `expected_transfers[].sender_owner` |
| `cluster` | string | `cluster` |
| `profileId` | string | `profile_id` |
| `version` | integer | `version` |

Buyer supplies at commit time: `payment_uid` (hex-64) → SLA `payment_uid`.

---

## 4. SLA shape (after serialization)

Logical content (profile schema is authoritative):

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
  "payment_uid": "<hex-64>",
  "profile_id": "x402/oracles/onchain-transfer/v1",
  "version": 1
}
```

Bytes = `x402/canonical-json/v1` over this value tree.

---

## 5. Flow

1. Unpaid GET with intent params → 402 + commit material + pr402 extras.
2. Buyer builds SLA, `sla_hash`, calls pr402 `build-sla-escrow-payment-tx`.
3. Buyer signs `FundPayment`, retries GET with `PAYMENT-SIGNATURE`.
4. Seller verifies hash, uploads SLA, transfers SPL, uploads delivery evidence,
   `SubmitDelivery`.

---

## 6. Worked example (devnet)

```http
GET /api/v1/buy-spl-token?token=merry-xmas&recipient_owner=4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU&buyer_nonce=abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789
```

---

## 7. Migration note

Existing deployments may expose commit fields at `accepts[].extra` top level
instead of `commitMaterial`. New open-source reference code **SHOULD** use
`commitMaterial` per Layer 1 spec.

---

## 8. References

| Reference | Purpose |
|---|---|
| `x402/delegated-authoring/v1` | Abstract pattern |
| `x402/oracles/onchain-transfer/v1` | Verdict rules |
| `x402/pr402-discovery/v1` | Build + 402 wire |

---

**Document version:** v1.0
