# Delegated SLA Authoring over HTTP 402 — Version 1 (Normative)

**Specification identifier:** `x402/delegated-authoring/v1`
**Document status:** Normative pattern for mapping buyer **purchase intent**
to an SLA document bound by on-chain `sla_hash`, using HTTP 402 and x402 v2.
**Scope:** Intent contracts, commit variants, commit material, verification.
**Out of scope:** Domain-specific parameters (Layer 2 profiles, Layer 3
bindings).

> Layer index: `spec/README.md` (layer index, this repo). See also
> `x402/sla-escrow-protocol/v1`,
> `x402/sla-document/v1`,
> `x402/serialization-recipes/v1`,
> `x402/pr402-discovery/v1`.

---

## Abstract

**Delegated authoring** applies when a buyer pays for an HTTP resource via
sla-escrow without hand-authoring profile-level SLA JSON. The buyer sends
**purchase intent**; the seller publishes how intent maps to SLA bytes; both
parties converge on `sla_hash` before or at `FundPayment`.

This spec is domain-agnostic. Concrete parameter names (e.g. product skus,
wallet fields) belong in the seller's **intent contract** or a Layer 3
**binding** — not here.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) /
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## 1. Terminology

| Term | Definition |
|---|---|
| **Purchase intent** | Buyer input to the unpaid request describing the expected deliverable. |
| **Intent contract** | Machine-readable spec: parameters, types, SLA mapping, recipe, commit variant. |
| **Commit material** | Seller-supplied values (402 response and/or contract) needed to build or verify `B_sla`. |
| **Commit variant** | `buyer-commit` or `seller-precommit` (§4). |
| **Escrow terms** | x402 `accepts[]` principal (`asset`, `amount`, `payTo`) — payment into escrow, distinct from deliverable semantics. |
| **Deliverable terms** | What the SLA encodes (profile-specific); defined in intent contract. |

---

## 2. Intent contract

Every delegated endpoint **MUST** have a published intent contract reachable
before payment (`intentContractUrl` on 402, OpenAPI, or equivalent).

### 2.1 Required declarations

| Field | Required | Purpose |
|---|---|---|
| `endpoint` | yes | HTTP method + path template. |
| `profileId` | yes | Oracle profile id for the SLA. |
| `commitVariant` | yes | `buyer-commit` \| `seller-precommit`. |
| `serializationRecipeId` | yes | From `serialization-recipes/v1`. |
| `intentParameters[]` | yes | Buyer fields: name, location, type, required, description, `mapsToSlaField`. |
| `sellerContextFields[]` | yes | Seller-derived SLA inputs: name, source, `mapsToSlaField`. |
| `escrowTerms` | yes | Meaning of `accepts[].asset` and `accepts[].amount`. |
| `deliverableSummary` | yes | Human-readable statement of what the buyer receives (not escrow principal). |

Optional: JSON Schema for intent, JSON Schema for commit material object,
worked examples.

### 2.2 Escrow vs deliverable

Intent contracts **MUST** distinguish:

- **Escrow terms** — what the buyer locks in sla-escrow (typically stablecoin
  principal on the `accepts[]` line).
- **Deliverable terms** — what the oracle evaluates (encoded in SLA JSON).

Documenting only `asset` + `amount` without deliverable semantics is
insufficient for buyer agents.

---

## 3. HTTP 402 response (sla-escrow)

On unpaid requests, sellers **MUST** return HTTP 402 with an x402 v2 body
containing an `accepts[]` line for scheme `sla-escrow` per
`pr402-discovery/v1`.

### 3.1 Required `accepts[].extra` (all variants)

| Field | Type | Purpose |
|---|---|---|
| `intentContractUrl` | URL | Stable link to intent contract (unless contract is inline in `intentSchema`). |
| `commitVariant` | string | `buyer-commit` \| `seller-precommit`. |
| `serializationRecipeId` | string | Recipe id. |
| `profileId` | string | Target oracle profile. |

Plus all pr402-required sla-escrow fields (`oracleAuthorities`, `escrowProgramId`, …).

### 3.2 Commit material

Seller-specific commit fields **MUST** be grouped under
`accepts[].extra.commitMaterial` (object). Key names and types are defined
only in the intent contract — the core spec does not enumerate them.

Alternative: inline the same keys at `extra` top level if the intent contract
declares that layout; agents resolve via `intentContractUrl`.

---

## 4. Commit variants

Both variants are peer options; choice **MUST** be declared in the intent
contract.

### 4.1 `buyer-commit`

| Phase | Responsibility |
|---|---|
| Unpaid 402 | Seller returns commit material; does **not** assert final `slaHash`. |
| Before chain | Buyer chooses `payment_uid`, builds `B_sla` via recipe, sets `sla_hash = SHA-256(B_sla)`, signs `FundPayment`. |
| Paid request | Seller rebuilds `B_sla`, verifies hash matches on-chain `payment.sla_hash`, uploads bytes to registry, performs work. |

**Seller MUST NOT** treat any `slaHash` on the 402 response as authoritative
in this variant.

**Buyer MUST** verify deliverable terms locally before signing.

### 4.2 `seller-precommit`

| Phase | Responsibility |
|---|---|
| Unpaid 402 | Seller builds `B_sla`, uploads to registry, returns authoritative `slaHash`, `slaUrl`, and `paymentUidHex` (or seller-chosen uid). |
| Before chain | Buyer verifies `B_sla` matches intent, then signs `FundPayment` with returned hash. |
| Paid request | Seller performs work; SLA already in registry. |

Use when the SLA does not require buyer-chosen `payment_uid` before first
hash, or when the seller allocates the uid.

---

## 5. Buyer verification (before `FundPayment`)

Buyers **SHOULD**:

1. Fetch and parse the intent contract.
2. Validate all required intent parameters.
3. Select `oracle_authority` per `pr402-discovery/v1`.
4. Reconstruct or fetch `B_sla`; compute or confirm `sla_hash`.
5. Confirm deliverable terms match purchase intent (not merely escrow terms).
6. Query oracle `GET /v1/policy` for tip-floor compatibility.

For autonomous agents, steps 4–5 are **MUST** when using `buyer-commit`.

---

## 6. Seller obligations (summary)

| | `buyer-commit` | `seller-precommit` |
|---|---|---|
| Publish intent contract | MUST | MUST |
| Return `commitMaterial` on 402 | MUST | MAY (hash may suffice) |
| Authoritative `slaHash` on 402 | MUST NOT | MUST |
| Deterministic recipe | MUST | MUST |
| Hash mismatch → reject paid request | MUST | MUST |
| Registry upload bytes = committed hash | MUST | MUST |

---

## 7. Versioning

Spec id: `x402/delegated-authoring/v1`. New commit variants require v2 or
a new variant string registered in an errata.

---

## 8. References

| Reference | Purpose |
|---|---|
| `x402/sla-escrow-protocol/v1` | Actor obligations |
| `x402/serialization-recipes/v1` | Recipe ids |
| `x402/pr402-discovery/v1` | x402 wire format |
| `x402/informative/bindings/` | Non-normative product bindings |

---

**Document version:** v2.0 (layered refactor; supersedes implementation-specific v1.0 text)
