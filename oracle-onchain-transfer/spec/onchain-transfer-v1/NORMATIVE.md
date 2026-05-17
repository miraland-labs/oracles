# On-Chain Transfer Profile — Version 1 (Normative)

**Profile identifier:** `x402/oracles/onchain-transfer/v1`
**Document status:** Normative specification for the `oracle-onchain-transfer`
reference implementation.
**Scope:** Off-chain SLA documents and delivery evidence for **SPL token
transfer / swap** adjudication on a single Solana cluster.

> For the cross-actor flow (buyer / seller / oracle / pr402) that surrounds
> this profile, see [`SLA_ESCROW_PROTOCOL.md`](../../../docs/SLA_ESCROW_PROTOCOL.md).
> This document is normative for the per-profile rules; the protocol doc is
> normative for the wire-level interaction.

---

## Abstract

This profile defines a finite rule set for binding a buyer's expectations and a
seller's fulfillment to cryptographic hashes (`sla_hash`, `delivery_hash`)
while permitting an oracle to verify settlement directly against the same
Solana cluster that the seller used to deliver. The oracle checks the Solana
transaction's pre / post token balances and approves iff the SLA's
`expected_transfers` are satisfied.

**Keywords:** Solana, SPL Token, escrow, oracle, transfer attestation.

---

## 1. Introduction

For payments where the seller's deliverable is itself an on-chain action — for
example, "deliver N units of token M to wallet B as part of a paid swap" — the
proof of delivery is the Solana transaction itself. This profile binds:

* `sla_hash` to the agreed `(mint, recipient_owner, min_amount, direction)`
  tuple(s) the buyer expects to see executed.
* `delivery_hash` to the seller's evidence JSON, which carries the
  `tx_signature` and the seller's claimed deltas.
* `resolution_hash` to the canonical
  `x402/oracles/resolution-envelope/v1` digest computed over the verdict.

The oracle verifies the SLA against the actual on-chain pre/post balances; the
seller's `claimed_delta` is informational only.

This document is **normative** for verdicts produced by
`oracle-onchain-transfer` at profile version `1`.

---

## 2. Terminology


| Term                     | Definition                                                                                                                                                |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **SLA document**         | UTF-8 JSON object describing the agreed transfer(s).                                                                                                      |
| **Delivery evidence**    | UTF-8 JSON object carrying the seller's transaction signature and asserted deltas.                                                                        |
| **Cluster**              | Solana cluster (`mainnet-beta`, `devnet`, `testnet`). The oracle's RPC MUST point at the same cluster the SLA names; mismatch is a hard reject.           |
| **Pre/post balance**     | The Solana RPC `getTransaction(jsonParsed)` response's `meta.preTokenBalances` / `meta.postTokenBalances` entries.                                        |
| **Direction**            | `"in"` (recipient gains tokens) or `"out"` (recipient loses tokens). Sign of `delta = post - pre` is checked against this.                                |
| **Profile**              | A versioned rule family (`x402/oracles/onchain-transfer/v1`).                                                                                                     |


### 2.1 Trust model

`x402/oracles/onchain-transfer/v1` is **cryptographically strong on the "transfer
happened" axis**. The oracle reads the Solana RPC directly and re-derives
balance deltas from the same `getTransaction` response any auditor can fetch.
Specifically, it proves:

* The transaction `tx_signature` was confirmed on the configured cluster.
* `meta.err` is `None` (the transaction did not fail on-chain).
* For each `expected_transfer`, a matching `(mint, recipient_owner)` row
  exists in `meta.postTokenBalances`, and the observed `delta = post − pre`
  satisfies `direction` and `min_amount`.

It does **not** prove:

* That the source of funds is the seller's wallet (the oracle only checks the
  destination side; swap routing through a third-party AMM is permissible).
* That the off-chain service the buyer paid for was actually rendered. The
  oracle takes the SLA at face value; it is the buyer's responsibility to put
  the right `recipient_owner` / `mint` / `min_amount` in the SLA before
  funding.
* That the oracle's RPC node is honest. Operators SHOULD configure multiple
  RPCs and require quorum (`ORACLE_RPC_QUORUM=N` is reserved for a future
  revision).

---

## 3. Cryptographic binding

Let `SHA256` denote the SHA-256 function on byte strings.

* `sla_hash = SHA256(B_sla)` where `B_sla` is the exact UTF-8 encoding of the
  SLA JSON text retrievable at the evidence registry path keyed by that hash.
* `delivery_hash = SHA256(B_del)` where `B_del` is the exact UTF-8 encoding
  of the delivery evidence JSON text similarly retrievable.

Hashing **serialized bytes** (not a re-parse through an arbitrary serializer)
ensures the seller, buyer, and oracle agree on the committed artifact.

---

## 4. SLA document

### 4.1 Schema

The SLA document MUST validate against
[`schema/sla-document.schema.json`](schema/sla-document.schema.json).

### 4.2 Field semantics


| Field                                        | Type                | Required | Notes                                                                                                                       |
| -------------------------------------------- | ------------------- | -------- | --------------------------------------------------------------------------------------------------------------------------- |
| `version`                                    | `u32`               | yes      | MUST be `1`.                                                                                                                |
| `profile_id`                                 | `string`            | yes      | MUST be `x402/oracles/onchain-transfer/v1`.                                                                                         |
| `cluster`                                    | enum                | yes      | One of `mainnet-beta`, `devnet`, `testnet`. Mismatch with the binary's configured cluster is a hard reject (`Custom(261)`). |
| `expected_transfers`                         | `array`             | yes      | Non-empty; each element specifies one `(mint, recipient_owner, min_amount, direction)` constraint.                          |
| `expected_transfers[].mint`                  | base58 `string`     | yes      | SPL mint pubkey.                                                                                                            |
| `expected_transfers[].recipient_owner`       | base58 `string`     | yes      | Owner pubkey of the destination ATA (NOT the ATA itself).                                                                   |
| `expected_transfers[].min_amount`            | decimal `string`    | yes      | Minimum **raw** token amount; compared to `|post − pre|`.                                                                   |
| `expected_transfers[].direction`             | `"in"` \| `"out"`   | yes      | Direction relative to `recipient_owner`.                                                                                    |
| `swap_router`                                | base58 `string`     | optional | Recorded but not enforced in v1.                                                                                            |
| `slippage_bps`                               | `u16`               | optional | Recorded but not enforced in v1.                                                                                            |
| `deadline_unix`                              | `i64`               | optional | If set and `meta.block_time > deadline_unix`, reject (`Custom(260)`).                                                       |


---

## 5. Delivery evidence

### 5.1 Schema

The delivery evidence MUST validate against
[`schema/delivery-evidence.schema.json`](schema/delivery-evidence.schema.json).

### 5.2 Field semantics


| Field                                        | Type             | Required | Notes                                                                                              |
| -------------------------------------------- | ---------------- | -------- | -------------------------------------------------------------------------------------------------- |
| `version`                                    | `u32`            | yes      | MUST be `1`.                                                                                       |
| `profile_id`                                 | `string`         | yes      | MUST be `x402/oracles/onchain-transfer/v1`.                                                                |
| `tx_signature`                               | base58 `string`  | yes      | Solana transaction signature of the transfer/swap that fulfilled the SLA.                          |
| `asserted_transfers`                         | `array`          | yes      | Seller's claim of `(mint, recipient_owner, claimed_delta)`. Informational only — oracle re-derives. |
| `submitted_at`                               | `i64`            | yes      | Unix epoch seconds when evidence was recorded (audit metadata).                                    |


---

## 6. Evaluation semantics

Given validated SLA `S` and evidence `E`, the oracle:

1. Asserts `S.cluster == config.cluster`. Mismatch → reject `Custom(261)` (`TransferClusterMismatch`).
2. Calls `getTransaction(E.tx_signature, jsonParsed)` against the configured
   RPC. Missing → reject `Custom(256)` (`TransferTxNotFound`). Failed
   (`meta.err.is_some()`) → reject `Custom(257)` (`TransferTxFailed`).
3. If `S.deadline_unix` is set, asserts `meta.block_time <= S.deadline_unix`.
   Late → reject `Custom(260)` (`TransferDeadlineExceeded`).
4. For each `S.expected_transfers[i]`:
   - Find `(mint, recipient_owner)` in `meta.postTokenBalances`. Missing
     → reject `Custom(259)` (`TransferMintMismatch`).
   - Find the matching pre-token-balance row (treat absent pre as zero —
     the standard semantic for newly-created ATAs).
   - Compute `delta = post.amount − pre.amount` as a signed `i128`.
   - Check the sign: `direction="in"` requires `delta > 0`,
     `direction="out"` requires `delta < 0`. Mismatch
     → reject `Custom(263)` (`TransferDirectionMismatch`).
   - Check magnitude: `|delta| >= min_amount`. Insufficient
     → reject `Custom(258)` (`TransferAmountInsufficient`).

If every `expected_transfers[]` entry passes, the verdict is **approved**
with reason `0` (`ResolutionReason::None`).

The first failing check in the above order determines the rejection reason
(P-VER-2). The `expected_transfers` array is iterated in declaration order;
the first failing entry's reason wins.

---

## 7. Resolution-hash details

The `details` slot of the canonical `x402/oracles/resolution-envelope/v1` envelope
carries the verifier's observation:

```json
{
  "txSignature": "<base58>",
  "cluster": "mainnet-beta",
  "verifiedTransfers": [
    {
      "mint": "Es9vMFr...",
      "recipientOwner": "BuyerPubkey...",
      "delta": "1000000",
      "satisfied": true
    }
  ],
  "blockTime": 1770000000,
  "slot": 287654321
}
```

Builds for this profile MAY omit `slot` when the RPC does not surface it; the
deterministic `compute_resolution_hash` recipe normalizes by always including
the field as `null` in that case.

---

## 8. Versioning and extensibility

* Documentation fixes do not change the profile id.
* Breaking changes (new required keys, changed evaluation semantics) require a
  new profile path (e.g. `…/v2`) and a bumped `version` field.

---

## 9. References

* Multi-category oracle architecture:
  [`design.md`](../../../../.kiro/specs/multi-category-oracle-architecture/design.md).
* Implementation: [`oracle-onchain-transfer/src/evaluator.rs`](../../src/evaluator.rs)
  — see `verify_observed_transfer` for the pure check battery.
