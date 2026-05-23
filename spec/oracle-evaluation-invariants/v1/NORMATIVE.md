# Oracle Evaluation Invariants — Version 1 (Normative)

**Specification identifier:** `x402/oracle-evaluation-invariants/v1`
**Document status:** Normative cross-family rules for oracle evaluation pipelines.
**Scope:** Byte binding, identity binding, freshness, replay protection.

> Per-family check batteries live in Layer 2 profile normatives. This document
> defines invariants **every production profile SHOULD implement** unless it
> documents an explicit waiver.

---

## Abstract

These invariants protect buyers and sellers from byte drift, cross-payment
replay, and stale evidence — independent of whether the deliverable is an SPL
transfer, HTTP snapshot, or file blob.

---

## 1. Byte binding (MUST)

| Step | Rule |
|---|---|
| SLA fetch | `SHA-256(fetched_bytes) == payment.sla_hash` else refuse |
| Delivery fetch | `SHA-256(fetched_bytes) == payment.delivery_hash` else refuse |
| Re-serialization | **MUST NOT** re-canonicalize before hash verify (`x402/sla-document/v1` §3) |

---

## 2. Profile dispatch (MUST)

- Parse SLA `profile_id` from verified bytes.
- **MUST** equal this binary's registered profile exactly.
- Unknown profile → refuse before family evaluation.

---

## 3. Payment identity binding (SHOULD → MUST for escrow-bound profiles)

When the SLA includes `payment_uid` (hex-64):

- Recomputed uid **MUST** equal on-chain `Payment.payment_uid`.
- Delivery evidence **SHOULD** echo the same uid when the profile uses JSON evidence.

When the SLA includes `buyer_nonce`:

- Evidence **SHOULD** echo the same nonce.

Failure → custom code in family range (see `x402/reason-codes/v1`).

---

## 4. Freshness lower bound (SHOULD)

When `Payment.created_at > 0` is available to the evaluator:

- Evidence timestamp or on-chain `block_time` **SHOULD** be ≥ `created_at`.
- Pre-funding replays **MUST** be rejected when observable.

Profiles using RPC observations use `block_time`; blob profiles use registry
`stored_at` or evidence `submitted_at`.

---

## 5. Cross-payment replay (SHOULD when ledger enabled)

When the operator configures a durable ledger:

- Evaluators **SHOULD** register evidence keys (e.g. `tx_signature`, `delivery_hash`)
  after successful approve.
- Reuse of the same key for a **different** `payment_uid` **MUST** be rejected.

Without a ledger, this invariant is best-effort only — operators **SHOULD**
document `DATABASE_URL` as required for production.

---

## 6. Expiry guard (MUST)

- Evaluator **MUST NOT** sign `ConfirmOracle` when `now > payment.expires_at`.
- Guardian **SHOULD** fail-closed reject before expiry when evaluation cannot complete.

---

## 7. Tip floor (MAY)

When operator enables tip floors (`x402/oracle-policy-http-api/v1`):

- Projected tip `floor(amount * oracle_fee_bps / 10000)` **MAY** be compared to published floors.
- Sub-floor jobs **MAY** be skipped with reason `200`.

Policy JSON **MUST** reflect effective floors including documented defaults
(see oracle-policy §2.4 errata).

---

## 8. Waivers

A profile **MAY** waive an invariant only by documenting:

- Which invariant is waived
- Why (e.g. trusted seller attestation-only model)
- Residual risk to buyers

---

## 9. References

| Reference | Purpose |
|---|---|
| `x402/sla-escrow-protocol/v1` §5 | Oracle obligations |
| `x402/profile-authoring/v1` | Profile checklist |
| `x402/reason-codes/v1` | Codes for invariant failures |

---

**Document version:** v1.0
