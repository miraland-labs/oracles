# Resolution Reason Codes — Version 1 (Normative)

**Specification identifier:** `x402/reason-codes/v1`
**Document status:** Normative registry of on-chain `resolution_reason` values
for `ConfirmOracle`.
**Scope:** Standard codes (0–255), partitioned custom ranges (≥256), allocation rules.

> On-chain field: `Payment.resolution_state` + `resolution_reason` per
> `x402/sla-escrow-onchain-abi/v1`.

---

## Abstract

Every oracle verdict carries a `resolution_reason` (u16). Codes **0–255** are
**standard** (shared semantics). Codes **≥256** are **custom**, partitioned by
profile family so independent implementers do not collide.

---

## 1. Standard codes (0–255)

| Code | Name | Meaning |
|---|---|---|
| `0` | `None` | Approved; no rejection |
| `1`–`99` | Protocol / guardian | Reserved by core protocol (timeout, guardian abort) |
| `100`–`199` | Operator economics | Tip floor, policy refusal |
| `200` | `TipBelowOperatorFloor` | Projected oracle tip below operator floor |
| `201`–`219` | Operator economics | Reserved extensions |
| `220`–`255` | Reserved | Future standard codes |

Exact guardian codes **100–102** are defined in operator runbooks; profiles
**MUST NOT** reuse 0–255 for family-specific semantics.

---

## 2. Custom code ranges (≥256)

| Range | Family prefix | Assignee |
|---|---|---|
| `256–319` | `x402/oracles/onchain-transfer/*` | On-chain SPL transfer profiles |
| `320–383` | `x402/oracles/file-delivery/*` | File / blob attestation profiles |
| `384–447` | `x402/oracles/compute-result/*` | Reserved for compute attestation |
| `448–479` | `x402/oracles/rwa-transfer/*` | RWA Token-2022 primary delivery profiles |
| `480–511` | Ecosystem | New families — register via errata before use |
| `512+` | Per-deployment | Private extensions; **MUST NOT** appear in public profile normatives |

New public profiles **MUST** request a range in the `448–511` window (or extend
an existing family prefix) before shipping.

---

## 3. Onchain-transfer v1 codes (256–269)

| Code | Constant | Meaning |
|---|---|---|
| `256` | `TransferTxNotFound` | RPC has no transaction for `tx_signature` |
| `257` | `TransferTxFailed` | Transaction landed but `meta.err` set |
| `258` | `TransferAmountInsufficient` | Observed delta below `min_amount` |
| `259` | `TransferMintMismatch` | No matching post balance row |
| `260` | `TransferDeadlineExceeded` | `block_time > deadline_unix` |
| `261` | `TransferClusterMismatch` | SLA cluster ≠ oracle cluster |
| `262` | `TransferRecipientNotResolvable` | Reserved / rare ATA path |
| `263` | `TransferDirectionMismatch` | Delta sign ≠ declared direction |
| `264` | `TransferEvidencePredatesPayment` | `block_time < Payment.created_at` |
| `265` | `TransferTxSignatureReused` | Same tx settled for another payment |
| `266` | `TransferPaymentUidMismatch` | SLA/evidence uid ≠ on-chain uid |
| `267` | `TransferBuyerNonceMismatch` | Evidence missing/wrong `buyer_nonce` |
| `268` | `TransferBlockTimeMissing` | Freshness check requires `block_time` |
| `269` | `TransferSenderMismatch` | Optional `sender_owner` pin failed |
| `270–319` | — | Reserved for onchain-transfer extensions |

---

## 4. File-delivery attestation v1 codes (320–327)

| Code | Constant | Meaning |
|---|---|---|
| `320` | `BlobSizeOutOfRange` | Blob size outside SLA bounds |
| `321` | `BlobMimeMismatch` | Sniffed MIME ≠ `expected_mime` |
| `322` | `BlobAttestorSignatureInvalid` | Seller signature over blob hash invalid |
| `323` | `BlobUploadIncomplete` | Streaming fetch incomplete |
| `324` | `BlobPredatesPayment` | Blob timestamp before funding |
| `325` | `BlobDeliveryHashReused` | Same blob hash, different payment |
| `326` | `BlobPaymentUidMismatch` | Companion evidence uid mismatch |
| `327` | `BlobBuyerNonceMismatch` | Missing/wrong buyer nonce |
| `328–383` | — | Reserved for file-delivery extensions |

---

## 5. RWA transfer v1 codes (448–479)

Normative detail: `x402/oracles/rwa-transfer/v1` §8.

| Code | Constant | Meaning |
|---|---|---|
| `448` | `RwaTokenProgramMismatch` | Mint owner ≠ SLA `token_program` |
| `449` | `RwaTransferHookMismatch` | Hook extension vs SLA mismatch |
| `450` | `RwaTransferTxNotFound` | RPC missing `tx_signature` |
| `451` | `RwaTransferTxFailed` | `meta.err` set (includes hook revert) |
| `452` | `RwaTransferAmountInsufficient` | Delta below `min_amount` |
| `453` | `RwaTransferMintMismatch` | No matching balance row |
| `454` | `RwaTransferDeadlineExceeded` | Past `deadline_unix` |
| `455` | `RwaTransferClusterMismatch` | Cluster ≠ oracle config |
| `456` | `RwaTransferDirectionMismatch` | Wrong delta sign |
| `457` | `RwaTransferSenderMismatch` | `sender_owner` pin failed |
| `458` | `RwaTransferEvidencePredatesPayment` | Evidence before funding |
| `459` | `RwaTransferTxSignatureReused` | Replay across payments |
| `460` | `RwaTransferPaymentUidMismatch` | UID binding failure |
| `461` | `RwaTransferBuyerNonceMismatch` | Nonce echo failure |
| `462–479` | — | Reserved for rwa-transfer extensions |

---

## 6. Allocation rules

1. First failing check in the documented evaluation order **MUST** determine
   the emitted code (deterministic verdicts).
2. Profiles **MUST NOT** emit codes outside their allocated range except
   standard codes §1.
3. Adding a code within an existing family range **MAY** be done via profile
   errata if the range has spare slots.
4. Exhausting a range requires a new profile version (`…/v2`) with a new range
   registration.

---

## 7. References

| Reference | Purpose |
|---|---|
| `x402/profile-authoring/v1` | Profile checklist |
| `x402/sla-escrow-onchain-abi/v1` | On-chain encoding |
| `oracle-common/src/resolution_codes.rs` | Reference constants |

---

**Document version:** v1.0
