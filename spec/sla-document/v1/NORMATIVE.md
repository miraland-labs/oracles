# SLA Document Envelope — Version 1 (Normative)

**Specification identifier:** `x402/sla-document/v1`
**Document status:** Normative cross-family specification for the JSON
envelope shared by all SLA documents under the x402 oracle ecosystem.
**Scope:** Required envelope fields, type rules, profile-id sniffing,
and the relationship between uploaded bytes and on-chain `sla_hash`.

> Family-specific SLA fields are normative under their respective
> per-family `<profile>/NORMATIVE.md`. This document defines the
> minimum cross-family envelope the registry and oracle dispatcher
> rely on.

---

## Abstract

This document specifies the JSON envelope that any conformant SLA
document **MUST** carry, the bytes-to-hash relationship that defines
`sla_hash`, and the rules for profile-id-based dispatch. It is the
minimum cross-family contract; per-family normatives extend it with
additional required fields.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) /
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## 1. Introduction

An SLA document declares, in JSON, what a buyer expects from a seller
in exchange for an escrow-funded payment. Three concerns must hold
across all profiles:

1. **The on-chain `sla_hash` binds to specific bytes.** The bytes the
   buyer hashes locally must be byte-identical to what the registry
   serves to the oracle. Any mid-flight transformation breaks the
   binding.

2. **The registry must dispatch uploads by profile.** The catalog row
   stores `profile_id` for routing, indexing, and access control.
   The registry parses `profile_id` from the SLA JSON without parsing
   the rest.

3. **The oracle must dispatch verdict logic by profile.** A single
   oracle binary serves one profile; multi-binary oracle deployments
   route incoming `DeliverySubmittedEvent`s by the SLA's `profile_id`
   to the correct binary.

This spec defines the envelope that satisfies all three concerns
without imposing canonicalization rules that would create a re-parse
attack surface.

---

## 2. Terminology

| Term | Definition |
|---|---|
| **SLA document** | UTF-8 JSON object describing the agreed deliverable for one payment. |
| **Envelope fields** | The cross-family fields specified in this document. |
| **Profile-specific fields** | Additional fields defined per-family in `<profile>/NORMATIVE.md`. |
| **Raw commitment** | The exact octet sequence (UTF-8 JSON text) hashed into `sla_hash`. No re-canonicalization is performed. |
| **`sla_hash`** | `SHA-256` of the raw commitment. 32 bytes. Hex when transmitted as text. |

---

## 3. Bytes-to-hash binding

This is the foundational rule. All other rules derive from it.

### 3.1 Rule

Let `B_sla` be the exact byte sequence the buyer commits to.

- The buyer **MUST** compute `sla_hash = SHA-256(B_sla)`.
- The buyer **MUST** transmit `B_sla` to the seller verbatim. The
  seller **MUST** upload `B_sla` verbatim to the registry.
- The registry **MUST** store and serve `B_sla` byte-identically.
  Re-serialization on the storage path is forbidden (see
  `registry-http-api/v1/NORMATIVE.md` §12.4).
- The oracle **MUST** fetch the bytes by `sla_hash`, recompute
  `SHA-256` over the received bytes, and refuse to evaluate if the
  computed digest does not equal the on-chain `sla_hash`.

### 3.2 Why no canonicalization

This spec **DOES NOT** specify canonicalization rules (sorted keys,
whitespace removal, RFC 8785 JCS, etc.). The on-chain commitment is
to bytes, not to a JSON value. Rationale:

- **Re-canonicalization is a re-parse attack surface.** A canonicalizer
  is a parser; any parser bug becomes a security bug. The buyer's
  hash is over what the buyer has, period.
- **Sellers and oracles already have the exact bytes** via the registry
  HTTP API. There is no scenario where someone has a different
  representation that needs to be normalized.
- **Per-family normatives can rely on direct byte comparison** for
  `sla_hash` checks. This is what `oracle-onchain-transfer/v1`,
  `oracle-api-quality/v1`, and `oracle-file-delivery/attestation/v1`
  already do.

The cost is that the buyer **MUST** be careful: re-encoding the SLA
through a different JSON serializer between local hash computation and
seller transmission will produce a different byte sequence and a
different hash. The buyer's local copy and the bytes sent to the seller
must be the same byte sequence.

### 3.3 Conformance

Conformant implementations:

- **MUST NOT** apply JCS or any other canonicalization between the
  buyer's hash computation and the registry upload.
- **MUST** treat `sla_hash` as a commitment to specific bytes, not to
  a structural JSON value.
- **SHOULD** retain the original `B_sla` bytes locally until at least
  the payment reaches a terminal state (Released or Refunded).

---

## 4. Encoding

### 4.1 Required encoding

`B_sla` **MUST** be:

- **Valid JSON** per RFC 8259.
- **UTF-8 encoded** (no BOM).
- A **JSON object** at the top level (not an array, scalar, or
  null).

The registry's `POST /v1/registry/sla` endpoint enforces JSON validity
and object-at-top-level (per `registry-http-api/v1` §9.4). Non-JSON or
non-object SLA uploads are rejected with `400 Bad Request`.

### 4.2 Permitted encoding choices

Within the constraints above, the buyer **MAY** choose:

- Any field ordering. Object key ordering is not normative.
- Any whitespace and formatting. Pretty-printed and minified
  representations are equally valid.
- Any escape style for strings (`"\u0041"` vs `"A"` are different
  bytes producing different hashes — pick one and be consistent).

The buyer's choice fixes `B_sla` and therefore `sla_hash`. Subsequent
parties (seller, oracle) work with `B_sla` as bytes, not as a
re-formatted reproduction.

### 4.3 Discouraged choices

Buyers **SHOULD NOT**:

- Embed comments. Standard JSON does not permit comments; some lax
  parsers accept them, others reject. Avoiding comments avoids portability
  bugs.
- Use trailing commas. Same rationale.
- Use NaN, Infinity, or `undefined`. Standard JSON does not represent
  these.

---

## 5. Envelope fields

Every SLA document **MUST** include these top-level fields. Profile-specific
normatives MAY require additional fields; they MUST NOT remove or rename
envelope fields.

| Field | Type | Required | Notes |
|---|---|---|---|
| `version` | integer | yes | The major version of the profile schema. **MUST** match the `v<n>` in `profile_id`. Enforced by per-family parsers (see §5.1). |
| `profile_id` | string | yes | The full profile identifier. **MUST** match `^x402/oracles/[a-z][a-z0-9-]*(/[a-z][a-z0-9-]*)*/v[0-9]+$` for oracle-evaluated profiles. The cross-family envelope dispatcher reads only this field; per-family parsers enforce its exact value match. |

### 5.1 Required field semantics

#### `version`

- **Type**: positive integer (`1`, `2`, ...).
- **Purpose**: lets the per-family parser do a fast equality check
  without re-parsing `profile_id`.
- **Constraint**: **MUST** equal the version embedded in `profile_id`.
  For example, if `profile_id` is `x402/oracles/onchain-transfer/v1`,
  then `version` is `1`.
- **Enforcement layer**: per-family parsers (not the cross-family
  envelope dispatcher). The dispatcher reads only `profile_id`; the
  per-family runner is responsible for refusing mismatched `version`.
- **Rationale**: the duplication is intentional. `profile_id` routes
  binaries; `version` lets the binary do a fast equality check before
  parsing further. A future v2 would have `profile_id` `…/v2` AND
  `version: 2`.

#### `profile_id`

- **Type**: non-empty UTF-8 string.
- **Purpose**: identifies the verdict family AND the schema version.
- **Format**: `x402/oracles/<family>/v<n>` for oracle-evaluated
  profiles. The `<family>` segment is lowercase ASCII, possibly with
  hyphens or slashes (e.g., `file-delivery/attestation`). The version
  is `v` followed by a positive integer.
- **Empty / whitespace-only values** are rejected at upload time
  (registry HTTP API §9.3).
- **Mismatch with the oracle binary's `registered_profile_id`** is a
  hard reject by per-family normatives (typically the first check
  performed).

### 5.2 Optional envelope fields

The following are **OPTIONAL** at the envelope layer. Profile-specific
normatives MAY require them.

| Field | Type | Notes |
|---|---|---|
| `payment_uid` | string (32-byte hex) | The intended `payment_uid` for the on-chain `Payment` PDA. Including it lets oracles cross-check that the SLA document was authored for the specific payment they are evaluating. |
| `buyer_nonce` | string (base64 or hex) | A buyer-chosen nonce that ensures `sla_hash` is unique even when the SLA fields are otherwise identical across payments. RECOMMENDED for buyers issuing many SLAs with similar shapes. |
| `cluster` | string | Solana cluster (`mainnet-beta`, `devnet`, `testnet`). REQUIRED by some per-family normatives (e.g. `onchain-transfer/v1`). |

#### `payment_uid` rationale

A buyer authoring two payments with identical SLA fields would otherwise
produce identical `sla_hash` values, causing PDA collision on
`FundPayment` (the program rejects with `Account already in use`).
Including a unique `payment_uid` per SLA prevents the collision.
Alternatively, `buyer_nonce` serves the same purpose without binding
to a specific payment uid.

#### `buyer_nonce` rationale

Some buyers prefer to mint `payment_uid` after authoring the SLA
(e.g., derived from `sla_hash`). For these, `buyer_nonce` provides
the uniqueness without a chicken-and-egg dependency.

---

## 6. Profile-specific extension

Per-family normatives extend this envelope. Conformance rules:

- A per-family normative **MUST** declare its `profile_id`.
- A per-family normative **MUST** specify additional required fields,
  each with a type and validation rule.
- A per-family normative **MUST** NOT redefine the meaning of envelope
  fields.
- A per-family normative **MAY** specify a JSON Schema for full
  document validation.

### 6.1 Reference profiles

| `profile_id` | Per-family normative |
|---|---|
| `x402/oracles/onchain-transfer/v1` | `oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md` |
| `x402/oracles/api-quality/v1` | `oracle-api-quality/spec/api-quality-v1/NORMATIVE.md` |
| `x402/oracles/file-delivery/attestation/v1` | `oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md` |

---

## 7. Profile dispatch

### 7.1 At the registry

When the registry receives `POST /v1/registry/sla`, it parses the body
to extract `profile_id`. The catalog row stores
`profile_id` for indexing and operator visibility. The registry
**MUST NOT** reject SLA uploads on `profile_id` value (a registry may
host SLA bytes for profiles served by other oracle binaries — useful
in test deployments).

### 7.2 At the oracle

When an oracle receives a `DeliverySubmittedEvent` with
`oracle_authority == self.pubkey`, it:

1. Fetches the SLA bytes by `sla_hash` from the registry.
2. Re-verifies `SHA-256(bytes) == sla_hash`.
3. Parses the cross-family envelope to read `profile_id`.
4. Dispatches to the per-family parser registered for that
   `profile_id`. If no parser is registered, **MUST** reject with
   `UnknownProfile`.
5. The per-family parser **MUST** reject if `profile_id` does not
   match the binary's `registered_profile_id`.
6. The per-family parser **MUST** reject if `version` does not match
   the embedded version in `profile_id`.

Layering: steps 1–4 are cross-family concerns and live in the envelope
dispatcher; steps 5–6 are per-family responsibilities. Both checks
happen before any per-family validation logic. They protect
against:

- A buyer routing an SLA to the wrong oracle (a mis-bind on
  `oracle_authority`).
- A buyer using a malformed `profile_id` that the oracle would otherwise
  silently misinterpret.

### 7.3 Multi-binary oracle deployments

A deployment with multiple oracle binaries (one per profile) **MUST**
operate each binary on a distinct `oracle_authority` Solana keypair.
The buyer's `FundPayment.oracle_authority` selection determines which
binary will see the event. Routing by `profile_id` happens at the
binary level after fetch; cross-binary message-passing is out of scope
for this spec.

---

## 8. Conformance

A conformant **buyer**:

- Authors an SLA document satisfying §4 (encoding) and §5 (envelope
  fields).
- Computes `sla_hash` over the exact bytes.
- Transmits those bytes to the seller verbatim.
- MUST NOT apply canonicalization between hash computation and
  transmission.

A conformant **registry**:

- Stores SLA bytes verbatim.
- Parses only `profile_id` for catalog tagging.
- Re-verifies SHA-256 on read (per registry HTTP API spec).

A conformant **oracle**:

- Re-verifies SHA-256 on receipt.
- Validates `profile_id` and `version` envelope fields before per-family
  validation.
- Rejects mismatches with the documented per-family error code.

A conformant **per-family `<profile>/NORMATIVE.md`**:

- Cites this envelope spec.
- Specifies its `profile_id`.
- Adds profile-specific required fields without redefining envelope
  semantics.
- Specifies its delivery evidence format separately (this spec covers
  only the SLA document side).

---

## 9. Versioning

This spec is `x402/sla-document/v1`. The envelope is intentionally
minimal so v1 should remain stable across many profile generations.

- **Minor additions** (new optional envelope fields with documented
  defaults) MAY be issued as v1 errata.
- **Substantive changes** (new required fields, type changes) require
  v2.

A future v2 envelope **MUST** be backward-readable by v1 oracles only
to the extent that `profile_id` and `version` parsing remains valid;
profile-id-based dispatch protects against silent misinterpretation.

---

## 10. References

| Reference | Purpose |
|---|---|
| `sla-escrow-protocol/v1/NORMATIVE.md` | Cross-actor protocol that uses this envelope |
| `registry-http-api/v1/NORMATIVE.md` | Wire-level registry contract for upload/fetch |
| `oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md` | Reference per-family extension |
| `oracle-api-quality/spec/api-quality-v1/NORMATIVE.md` | Reference per-family extension |
| `oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md` | Reference per-family extension (binary delivery, JSON SLA) |
| RFC 2119 / RFC 8174 | Keyword interpretation |
| RFC 8259 | JSON syntax |
| SHA-256 (NIST FIPS 180-4) | Hash function |

---

**Document version:** v1.0
**Last verified against per-family references:** 2026-05-22
