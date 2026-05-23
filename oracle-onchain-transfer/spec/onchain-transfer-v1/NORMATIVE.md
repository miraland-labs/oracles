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
| `expected_transfers[].sender_owner`          | base58 `string`     | optional | When set, the oracle verifies the same `(mint, sender_owner)` pair appears in `pre_token_balances` and the sender's signed delta is negative with magnitude ≥ `min_amount`. Defense-in-depth on top of cross-payment replay protection. |
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
   - **(Optional)** If `S.expected_transfers[i].sender_owner` is set:
     - Find `(mint, sender_owner)` in `meta.preTokenBalances`. Missing
       → reject `Custom(269)` (`TransferSenderMismatch`).
     - Compute `sender_delta = sender_post.amount − sender_pre.amount`,
       treating an absent post-row as `0` (the sender drained their
       balance).
     - Require `sender_delta < 0` AND `|sender_delta| >= min_amount`.
       Failure of either condition → reject `Custom(269)`. The
       diagnostic detail string distinguishes the no-row, wrong-
       direction, and insufficient-magnitude cases.
     - Note: the sender's `|delta|` MAY exceed the recipient's `|delta|`
       on Token-2022 mints with a transfer-fee extension. The check uses
       `min_amount` as the floor for both sides independently, so a
       small fee gap does not cause false rejects as long as both
       sides clear the floor. See §6.1.

If every `expected_transfers[]` entry passes, the verdict is **approved**
with reason `0` (`ResolutionReason::None`).

The first failing check in the above order determines the rejection reason
(P-VER-2). The `expected_transfers` array is iterated in declaration order;
the first failing entry's reason wins.

### 6.1 Token-2022 transfer-fee handling

Mints owned by the **plain SPL Token** program
(`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`) have no transfer-fee
extension, so the recipient receives exactly what the sender sent. For
those mints the buyer's `min_amount` and the sender's debit are
equal, and there is nothing more to think about.

Mints owned by **Token-2022**
(`TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`) MAY carry a
transfer-fee extension that withholds a basis-points share of every
transfer. The fee is computed at the program level and credited to the
mint's withheld-fee account; the recipient's balance gains only the
**post-fee net** amount.

Two facts the buyer and seller MUST internalize:

1. The oracle reads `meta.postTokenBalances` from
   `getTransaction(jsonParsed)`. Solana's RPC reports balances *after*
   any Token-2022 fee was withheld, so the **delta the oracle sees on
   the recipient row is already net**. The check
   `|recipient_delta| >= min_amount` therefore compares the buyer's
   declared minimum to what the recipient actually receives, not to
   what the sender debited.

2. When `expected_transfers[i].sender_owner` is set, the oracle's
   sender-side check (§6, step 4, optional) requires
   `|sender_delta| >= min_amount`. The sender's gross debit is at
   least the recipient's net credit (the fee comes out of what the
   sender sent), so a sender-pinned check passes whenever the
   recipient-pinned check passes — provided the buyer's `min_amount`
   reflects post-fee expectations as described below.

**Authoring rule for the buyer.** When the mint has a transfer-fee
extension, set `min_amount` to the **post-fee amount the recipient
will receive**, NOT the gross amount the seller debits. Otherwise
honest deliveries will be rejected with `Custom(258)`
(`TransferAmountInsufficient`).

**Worked example.** A Token-2022 mint with a 150-bps (1.5%) transfer
fee. The buyer wants the recipient to net 1,000,000 raw units. The
seller will need to send a gross amount such that `gross × (1 -
0.015) ≥ 1_000_000`, i.e. `gross ≥ 1_015_229` (rounded up to the
nearest raw integer). The buyer's SLA SHOULD declare:

```json
"min_amount": "1000000"
```

A delivery that sends 1,015,229 raw → recipient nets 1,000,001 →
recipient_delta = 1,000,001 ≥ min_amount = 1,000,000 → **approve**.

A buyer who instead declared `"min_amount": "1015229"` (the gross
figure) would see the same honest delivery **reject** with
`TransferAmountInsufficient` because the recipient's net delta of
1,000,001 falls below the declared 1,015,229.

**Authoring rule for the seller.** Before broadcasting, query the
mint to discover whether a fee extension is present:

```bash
spl-token display <MINT_ADDRESS>
```

If the output shows a `Transfer fee` line with a non-zero rate, you
are in the Token-2022 fee path. Compute the gross amount that, after
fee withholding, lands at least `min_amount` at the recipient. Going
under the floor → reject. Going just over → approve and your
recipient receives slightly more than the buyer's minimum.

**Why the oracle does not auto-adjust.** The fee rate is a
mint-configurable parameter that can change over time
(`SetTransferFee`). Hard-coding "the oracle adds 1.5% headroom"
would be wrong as soon as the rate moved. Pushing the math to the
buyer keeps the oracle's check rule simple, deterministic, and
reproducible by any third-party auditor recomputing
`resolution_hash`.

The sender-side check (`sender_owner`, §6 step 4 optional) uses the
same `min_amount` floor on `|sender_delta|`. On a fee-bearing mint
the sender's debit is **strictly greater** than the recipient's
credit, so any `min_amount` that satisfies the recipient also
satisfies the sender by construction. There is no special
authoring rule for the sender side beyond what's already covered.

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

Evaluators **MUST** populate this object (not the full SLA document) on
`EvaluationResult.resolution_details` before the shared pipeline computes
`resolution_hash`. Reject paths **SHOULD** still include `txSignature`,
`cluster`, and per-transfer rows with `satisfied: false` when observation
data is available.

---

## 8. Versioning and extensibility

* Documentation fixes do not change the profile id.
* Breaking changes (new required keys, changed evaluation semantics) require a
  new profile path (e.g. `…/v2`) and a bumped `version` field.

---

## 9. References

* Cross-actor protocol: [`SLA_ESCROW_PROTOCOL.md`](../../../docs/SLA_ESCROW_PROTOCOL.md).
* Reason codes: `x402/reason-codes/v1` §3.
* Implementation: [`oracle-onchain-transfer/src/evaluator.rs`](../../src/evaluator.rs)
  — see `verify_observed_transfer` for the pure check battery.
