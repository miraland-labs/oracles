# RWA Transfer Profile — Version 1 (Normative)

**Profile identifier:** `x402/oracles/rwa-transfer/v1`
**Document status:** Normative specification for the planned
`oracle-rwa-transfer` reference implementation.
**Scope:** Off-chain SLA documents and delivery evidence for **RWA primary
delivery** — Token-2022 SPL transfer adjudication on a single Solana cluster,
with Transfer Hook program awareness.

> For the cross-actor flow (buyer / seller / oracle / pr402), see
> `x402/sla-escrow-protocol/v1`.
> For generic SPL transfer semantics without RWA extensions, see
> `x402/oracles/onchain-transfer/v1` — **unchanged**; this profile is a
> **sibling**, not a revision of that document.

---

## Abstract

This profile adjudicates **issuer → investor** delivery of a **Token-2022**
RWA mint during a sla-escrow primary subscription. Payment into escrow is
typically **USDC on classic SPL**; that leg is **out of scope** for this
profile (handled by sla-escrow + seller settlement).

The oracle verifies that a confirmed Solana transaction satisfies the SLA's
`expected_transfers` constraints and that Token-2022 / Transfer Hook metadata
in the SLA matches on-chain mint configuration. **KYC eligibility itself**
is enforced by the Transfer Hook program at execution time; this profile
proves the transfer **succeeded on-chain** under those rules.

**Keywords:** Solana, Token-2022, RWA, transfer hook, escrow, oracle.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) /
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## 1. Introduction

RWA primary issuance separates three layers:

1. **Qualification** (off-chain KYC, optional pre-gates) — not evaluated here.
2. **Payment** (USDC sla-escrow) — on-chain program, not this profile.
3. **Delivery** (Token-2022 transfer to qualified wallet) — **this profile**.

This document binds:

* `sla_hash` to agreed transfer tuple(s), Token-2022 program id, and optional
  hook program id / offering id.
* `delivery_hash` to seller evidence JSON with `tx_signature`.
* `resolution_hash` to `x402/oracles/resolution-envelope/v1`.

Reference implementation target: `oracle-rwa-transfer` (sibling of
`oracle-onchain-transfer`, sharing `oracle-common` pipeline).

Informative seller binding: `x402/informative/bindings/buy-rwa-token/v1`.

---

## 2. Relationship to onchain-transfer/v1

| Topic | `onchain-transfer/v1` | `rwa-transfer/v1` (this doc) |
|---|---|---|
| Profile id | `x402/oracles/onchain-transfer/v1` | `x402/oracles/rwa-transfer/v1` |
| Oracle binary | `oracle-onchain-transfer` | `oracle-rwa-transfer` (planned) |
| Mint program | Classic SPL or Token-2022 | **Token-2022 required** |
| Transfer Hook metadata | Not required | **Required** when mint has hook extension |
| Reason codes | 256–319 | 448–479 |
| Ecosystem impact | Production reference | **Additive** — no changes to v1 |

Evaluators **SHOULD** reuse the same RPC delta algorithm as
`onchain-transfer/v1` §6 for `expected_transfers[]`, then apply §6.2
additional checks below.

---

## 3. Terminology

| Term | Definition |
|---|---|
| **RWA mint** | Token-2022 mint representing a real-world asset tranche or share class. |
| **Transfer Hook program** | On-chain program invoked by Token-2022 on transfer; enforces KYC PDA / allowlist rules. |
| **KYC PDA** | Program-derived account holding qualification state for an investor wallet; read by the hook — **not** written by this oracle. |
| **Offering id** | Issuer-defined string identifying a primary offering tranche; audit metadata only in v1. |

### 3.1 Trust model

This profile is **cryptographically strong on the "qualified transfer
happened" axis**, assuming the hook program is correct:

* Proves a **successful** transaction on the SLA cluster (`meta.err` is null).
* Proves recipient balance delta meets `min_amount` (post-fee net for
  fee-bearing mints — same rules as `onchain-transfer/v1` §6.1).
* Proves mint owner is Token-2022 when `token_program` is declared.
* Proves declared `transfer_hook_program` matches mint hook configuration
  when the mint has the Transfer Hook extension.

It does **not** prove:

* That off-chain KYC was legally sufficient (regulatory question).
* That the KYC PDA was written by a specific auditor (only that transfer
  succeeded under hook rules).
* Escrow funding correctness (sla-escrow + seller verify that separately).
* Confidential Transfer balance privacy (out of scope v1 — see §9).

---

## 4. Cryptographic binding

Same as `onchain-transfer/v1` §3 and `x402/sla-document/v1` §3:

* `sla_hash = SHA256(B_sla)` over exact UTF-8 SLA bytes.
* `delivery_hash = SHA256(B_del)` over exact UTF-8 delivery bytes.
* No re-canonicalization between hash and registry upload.

---

## 5. SLA document

### 5.1 Envelope

MUST satisfy `x402/sla-document/v1` envelope fields:

| Field | Value |
|---|---|
| `version` | `1` |
| `profile_id` | `x402/oracles/rwa-transfer/v1` |

### 5.2 Field semantics

All fields from `onchain-transfer/v1` §4.2 apply with the same meanings,
**plus**:

| Field | Type | Required | Notes |
|---|---|---|---|
| `token_program` | base58 `string` | yes | MUST be Token-2022 program id `TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb` on clusters where that program is deployed. Mismatch → reject `Custom(448)`. |
| `transfer_hook_program` | base58 `string` | conditional | **Required** when the RWA mint has the Transfer Hook extension. MUST equal the hook program id returned by mint inspection. Mismatch → reject `Custom(449)`. Omit only when mint has no hook extension (discouraged for RWA). |
| `offering_id` | `string` | optional | Issuer offering / tranche id for audit logs. Not enforced against chain state in v1. |
| `payment_uid` | hex-64 | yes | 32-byte payment uid bound at `FundPayment`. Same replay rules as onchain-transfer. |
| `buyer_nonce` | hex-64 | yes | Buyer entropy; echoed in delivery evidence. |
| `cluster` | enum | yes | `mainnet-beta` \| `devnet` \| `testnet` |
| `expected_transfers` | array | yes | Non-empty. Each element as onchain-transfer §4.2. |
| `expected_transfers[].sender_owner` | base58 | recommended | **SHOULD** be set to issuer treasury wallet for primary issuance. |

JSON Schema and examples **SHOULD** ship with `oracle-rwa-transfer` crate at
`spec/rwa-transfer-v1/schema/` (mirroring onchain-transfer layout).

---

## 6. Delivery evidence

Same shape as `onchain-transfer/v1` §5, with these substitutions:

| Field | Required value |
|---|---|
| `profile_id` | `x402/oracles/rwa-transfer/v1` |
| `payment_uid` | Echo from SLA |
| `tx_signature` | Successful delivery transaction |
| `buyer_nonce` | Echo from SLA when present |

---

## 7. Evaluation semantics

### 7.1 Baseline transfer checks

Execute `onchain-transfer/v1` §6 steps 1–4 (cluster match, tx fetch,
deadline, per-transfer delta checks) using reason codes **448–479** mapped
from the same failure classes where applicable (see §8). Implementations
**MAY** internally delegate to shared `verify_observed_transfer` logic.

Mapping from onchain-transfer codes (informative):

| onchain-transfer | rwa-transfer v1 |
|---|---|
| 256 TxNotFound | 450 |
| 257 TxFailed | 451 |
| 258 AmountInsufficient | 452 |
| 259 MintMismatch | 453 |
| 260 DeadlineExceeded | 454 |
| 261 ClusterMismatch | 455 |
| 263 DirectionMismatch | 456 |
| 269 SenderMismatch | 457 |

### 7.2 RWA-specific checks (after baseline passes)

1. **Token program pin:** Fetch mint account owner. MUST equal
   `S.token_program`. Else → reject `Custom(448)` (`RwaTokenProgramMismatch`).

2. **Transfer Hook pin:** If mint has Transfer Hook extension, parsed hook
   program id MUST equal `S.transfer_hook_program`. Else → reject
   `Custom(449)` (`RwaTransferHookMismatch`). If extension absent but
   `transfer_hook_program` is set → reject `Custom(449)`.

3. **Hook failure is tx failure:** If `meta.err` is set (including hook
   Custom errors), reject `Custom(451)` (`RwaTransferTxFailed`). No separate
   "KYC not passed" code is required — failed transfers never reach approval.

4. **Evidence freshness:** Apply onchain-transfer replay / uid / nonce rules
   using codes 458–461 (§8).

If all checks pass → **approved**, reason `0`.

### 7.3 Token-2022 transfer fees

Identical authoring rules to `onchain-transfer/v1` §6.1: `min_amount` is
**post-fee net** received by `recipient_owner`.

---

## 8. Custom reason codes (448–479)

Registered under `x402/reason-codes/v1` ecosystem window for
`x402/oracles/rwa-transfer/*`.

| Code | Constant | Meaning |
|---|---|---|
| `448` | `RwaTokenProgramMismatch` | Mint owner ≠ `token_program` |
| `449` | `RwaTransferHookMismatch` | Hook extension vs SLA mismatch |
| `450` | `RwaTransferTxNotFound` | RPC missing `tx_signature` |
| `451` | `RwaTransferTxFailed` | `meta.err` set (includes hook revert) |
| `452` | `RwaTransferAmountInsufficient` | Delta below `min_amount` |
| `453` | `RwaTransferMintMismatch` | No matching balance row |
| `454` | `RwaTransferDeadlineExceeded` | Past `deadline_unix` |
| `455` | `RwaTransferClusterMismatch` | Cluster ≠ oracle config |
| `456` | `RwaTransferDirectionMismatch` | Wrong delta sign |
| `457` | `RwaTransferSenderMismatch` | `sender_owner` pin failed |
| `458` | `RwaTransferEvidencePredatesPayment` | `block_time < created_at` |
| `459` | `RwaTransferTxSignatureReused` | Replay across payments |
| `460` | `RwaTransferPaymentUidMismatch` | UID binding failure |
| `461` | `RwaTransferBuyerNonceMismatch` | Nonce echo failure |
| `462–479` | — | Reserved for rwa-transfer extensions |

---

## 9. Out of scope (v1)

* **Confidential Transfer** mints — defer to future profile revision.
* **Permanent Delegate** enforcement — issuer/TA ops, not subscription oracle.
* **Multi-oracle quorum** inside one Payment — use sequential separate payments.
* **Legal / document attestation** — use `file-delivery/attestation/v1` in a
  separate sla-escrow payment if needed.

---

## 10. Resolution envelope details

Same structure as `onchain-transfer/v1` §7, with optional additions:

```json
{
  "txSignature": "<base58>",
  "cluster": "devnet",
  "tokenProgram": "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
  "transferHookProgram": "<base58-or-null>",
  "offeringId": "<string-or-null>",
  "verifiedTransfers": [ … ]
}
```

---

## 11. Versioning

* Documentation fixes do not change the profile id.
* Breaking SLA or evaluation changes require `x402/oracles/rwa-transfer/v2`
  and a new oracle binary registration.
* **MUST NOT** alter `x402/oracles/onchain-transfer/v1` when extending RWA.

---

## 12. References

| Reference | Purpose |
|---|---|
| `x402/oracles/onchain-transfer/v1` | Baseline transfer delta semantics |
| `x402/sla-document/v1` | Envelope + byte commitment |
| `x402/reason-codes/v1` | Code range policy |
| `x402/informative/bindings/buy-rwa-token/v1` | Reference HTTP seller binding |
| `x402/sla-escrow-protocol/v1` | Four-actor flow |

---

**Document version:** v1.0
