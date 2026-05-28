# pr402 Discovery and Build Contract — Version 1 (Normative)

**Specification identifier:** `x402/pr402-discovery/v1`
**Document status:** Normative wire-level specification for pr402 facilitator
discovery fields and the SLA-escrow payment transaction builder.
**Scope:** Seller `accepts[].extra` advertisement, facilitator
capabilities, and `POST /build-sla-escrow-payment-tx`.

> For HTTP 402 purchase intent and commit variants, see
> `x402/delegated-authoring/v1`.
> For on-chain FundPayment layout, see
> `x402/sla-escrow-onchain-abi/v1`.

---

## Abstract

pr402 is the optional HTTP facilitator that builds unsigned Solana
transactions, verifies payment proofs, and settles on-chain. This document
specifies the JSON shapes sellers MUST advertise and buyers MUST parse for
scheme `sla-escrow`, plus the build endpoint request body.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) /
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## 1. x402 v2 PaymentRequired envelope

An unpaid resource response uses **HTTP 402** with a JSON body (and MAY
mirror it in a `Payment-Required` header, base64-encoded):

```json
{
  "x402Version": 2,
  "error": "PAYMENT-SIGNATURE header is required (x402 v2 payment proof)",
  "resource": {
    "url": "https://seller.example/<resource-path>?…",
    "description": "Human-readable resource description",
    "mimeType": "application/json"
  },
  "accepts": [ { "...": "one payment line — see §2" } ],
  "extensions": {}
}
```

Buyers **MUST** select the `accepts[]` line matching their intended
`scheme` (`sla-escrow` for this spec).

---

## 2. `accepts[]` line for scheme `sla-escrow`

### 2.1 Top-level fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `scheme` | string | yes | MUST be `"sla-escrow"`. |
| `network` | string | yes | pr402 network id (e.g. `solana-devnet`, `solana-mainnet`). |
| `asset` | string | yes | Base58 mint pubkey of the escrow funding token (typically USDC). |
| `amount` | string | yes | Escrow principal in raw mint units (integer decimal string). |
| `payTo` | string | yes | Base58 **Escrow PDA** for `(program_id, asset mint, bank)`, NOT the seller wallet. |
| `maxTimeoutSeconds` | integer | yes | Payment proof validity hint for pr402 verify. |
| `extra` | object | yes | §2.2 |

### 2.2 `extra` — pr402-required fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `feePayer` | base58 pubkey | yes | Transaction fee payer pr402 expects on the built tx. |
| `oracleAuthorities` | pubkey[] | yes | Allow-list; `FundPayment.oracle_authority` MUST be one of these for verify success. |
| `escrowProgramId` | base58 pubkey | yes | Deployed sla-escrow program id for the cluster. |
| `bankAddress` | base58 pubkey | yes | Bank PDA. |
| `configAddress` | base58 pubkey | yes | Config PDA. |
| `feeBps` | string | yes | Protocol fee basis points (decimal string). |
| `oracleFeeBps` | string | yes | Oracle tip basis points at release/refund. |
| `ttlSeconds` | string | yes | Default `FundPayment.ttl_seconds` suggestion. |
| `maxComputeUnitLimit` | string | yes | CU limit bound for built transactions. |
| `recommendedComputeUnitPrice` | string | yes | Micro-lamports per CU hint. |
| `merchantWallet` or `beneficiary` | base58 pubkey | RECOMMENDED | Pubkey receiving funds on `ReleasePayment` (`payment.seller`). pr402 build prefers `beneficiary` over `merchantWallet`. |

Optional:

| Field | Type | Notes |
|---|---|---|
| `slaFundTxNetworkFeePayer` | `"buyer"` \| `"facilitator"` | Cost expectation only. |
| `oracleProfiles` | object[] | §3 |
| Delegated-authoring fields | various | `delegated-authoring/v1` §3 (`intentContractUrl`, `commitVariant`, `serializationRecipeId`, `commitMaterial`, or `seller-precommit`: `slaHash`, `slaUrl`, `paymentUidHex`) |

Sellers **SHOULD** obtain baseline pr402 fields from the facilitator
capabilities/`/supported` response for the target network and overlay
seller-specific values.

When a facilitator **elevates** a seller's Lite challenge to full sla-escrow
metadata (pr402 `POST /api/v1/facilitator/payment-required/enrich`, or
equivalent), it **MUST** merge institutional
fields into the existing `accepts[].extra` object rather than replacing it,
so delegated-authoring keys (`commitMaterial`, `intentContractUrl`,
`oracleProfiles`, …) survive elevation.

Facilitators **SHOULD** cross-check `bankAddress` and `configAddress` against
their derived PDAs on build and verify (same rule as `escrowProgramId`).

---

## 3. `oracleProfiles[]` (multi-family discovery)

Each entry advertises one oracle profile reachable via a distinct
`operatorPubkey`:

| Field | Type | Required | Notes |
|---|---|---|---|
| `profileId` | string | yes | e.g. `x402/oracles/onchain-transfer/v1` |
| `operatorPubkey` | base58 | yes | Signs `ConfirmOracle`; MUST also appear in `oracleAuthorities[]`. |
| `normativeSpecUrl` | URL | yes | Permanent link to profile NORMATIVE.md |
| `registryBaseUrl` | URL | optional | Registry origin for HEAD/GET/upload |
| `supportedClusters` | string[] | optional | Advisory (onchain-transfer enforces cluster in SLA) |
| `supportedMints` | string[] | optional | Advisory UI hint |
| `maxBlobBytes` | number | optional | Advisory (file-delivery) |

**Invariants:**

1. Every `operatorPubkey` in `oracleProfiles[]` **MUST** appear in
   `oracleAuthorities[]`.
2. No `operatorPubkey` may appear in two `oracleProfiles[]` entries.
3. `profileId` matching is exact string equality.

**Buyer selection algorithm:**

```text
oracle_authority = entry.operatorPubkey
  where entry.profileId == desired_profile_id
```

Buyers **MUST NOT** silently default to `oracleAuthorities[0]`.

---

## 4. `POST /build-sla-escrow-payment-tx`

**Path:** `/api/v1/facilitator/build-sla-escrow-payment-tx` (pr402 deployment).

### 4.1 Request

```json
{
  "payer": "<buyer-base58-pubkey>",
  "accepted": { "...": "one accepts[] line from 402, verbatim" },
  "resource": { "...": "402 resource object" },
  "slaHash": "<64-lowercase-hex>",
  "oracleAuthority": "<base58-pubkey>",
  "paymentUidHex": "<64-lowercase-hex>",
  "skipSourceBalanceCheck": false,
  "facilitatorPaysTransactionFees": false
}
```

| Field | Required | Notes |
|---|---|---|
| `slaHash` | yes | 32-byte SLA digest as 64 lowercase hex chars. |
| `oracleAuthority` | yes | MUST ∈ `accepted.extra.oracleAuthorities`. |
| `paymentUidHex` | optional | 32 raw bytes as 64 lowercase hex; verbatim PDA seed. |
| `paymentUid` | optional | Legacy string normalization (ABI §2.5). **MUST NOT** send both. |

### 4.2 Response

pr402 returns a standard `BuildPaymentTxResponse`:

- Base64-encoded unsigned transaction containing `FundPayment`.
- `paymentUid` / `paymentUidHex` echo.
- `verifyBodyTemplate` for subsequent `/verify` and `/settle`.

Buyer **MUST** sign the transaction locally; pr402 **MUST NOT** hold buyer keys.

### 4.3 Cross-checks pr402 enforces

- `escrowProgramId`, `bankAddress`, `configAddress` match facilitator config.
- `oracleAuthority` ∈ `oracleAuthorities`.
- Optional strict mode: `profileId` in SLA matches an `oracleProfiles[]` entry.

---

## 5. Facilitator capabilities

`GET /api/v1/facilitator/capabilities` MAY include:

```json
{
  "slaEscrowOracleProfiles": [
    {
      "profileId": "x402/oracles/onchain-transfer/v1",
      "normativeSpecUrl": "https://…",
      "defaultOperatorPubkey": "<base58>",
      "repositoryPath": "oracles/oracle-onchain-transfer"
    }
  ]
}
```

Every `defaultOperatorPubkey` **MUST** be configured in the facilitator's
oracle authority allow-list. Capabilities advertise defaults; seller
`accepts[]` lines are authoritative per resource.

---

## 6. Fee expectations

On-chain oracle tip at settlement (when `oracle_fee_bps > 0` and a verdict
was rendered):

```text
oracle_tip_raw = floor(payment.amount * payment.oracle_fee_bps / 10000)
```

Oracle operators MAY publish minimum tips via `GET /v1/policy`
(`oracle-policy-http-api/v1`). Buyers **SHOULD** confirm
`payment.amount * oracleFeeBps / 10000 ≥ minVerdictTip*` before funding.

---

## 7. Versioning

Spec id: `x402/pr402-discovery/v1`. Optional fields may be added without a
version bump. New required `extra` fields require v2 or a new scheme string.

---

## 10. References

| Reference | Purpose |
|---|---|
| `x402/serialization-recipes/v1` | Recipe registry |
| `x402/delegated-authoring/v1` | Purchase intent |
| `x402/informative/bindings/` | Example bindings |
| `x402/sla-escrow-onchain-abi/v1` | FundPayment wire format |
| `x402/oracle-policy-http-api/v1` | Oracle tip floors |
| pr402 `public/openapi.json` | Full facilitator surface |
| pr402 `src/sla_escrow_payment_build.rs` | Reference build implementation |

---

**Document version:** v1.1
