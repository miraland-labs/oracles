# Serialization Recipes — Version 1 (Normative)

**Specification identifier:** `x402/serialization-recipes/v1`
**Document status:** Normative registry of byte-serialization algorithms
used when computing `sla_hash`, `delivery_hash`, or `resolution_hash`.
**Scope:** Recipe identifiers, determinism requirements, registration rules.

> Byte commitment semantics: `x402/sla-document/v1` §3.
> Resolution envelope uses recipe `x402/envelope-json/v1` per
> `x402/oracles/resolution-envelope/v1`.

---

## Abstract

On-chain hashes bind to **octets**, not JSON values. This document names
deterministic serialization algorithms so sellers, buyers, and oracles can
agree on `B_*` without ad-hoc serializers.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) /
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## 1. Rules

1. Every delegated flow **MUST** declare a `serializationRecipeId` in the
   seller's intent contract (`delegated-authoring/v1`).
2. Direct authoring **MUST** use `x402/raw-bytes/v1` (buyer supplies
   final octets; no transform).
3. Implementations **MUST NOT** apply an undeclared serializer between
   hash computation and registry upload.
4. A recipe **MUST** be pure: identical input value → identical output
   bytes across processes and languages.

---

## 2. Registered recipes (v1)

### `x402/raw-bytes/v1`

**Input:** Byte string `B` already in final form.
**Output:** `B` unchanged.
**Use:** Direct SLA authoring.

### `x402/canonical-json/v1`

**Input:** JSON value (typically an object).
**Output:** UTF-8 JSON text with:

- Object keys sorted lexicographically at every object boundary.
- Array element order preserved.
- Compact separators (no space after `:` or `,`).
- Strings and numbers per RFC 8259; string escaping as JSON standard
  (implementation MAY use a well-tested JSON library's string encoder).

**Use:** Delegated authoring when seller and buyer build SLA JSON from
structured fields.

### `x402/envelope-json/v1`

Same algorithm as `x402/canonical-json/v1`, but the top-level key order
**MUST** be fixed by the envelope spec that references this recipe (see
`resolution-envelope/v1` §1).

**Use:** `resolution_hash` only, unless another spec explicitly cites it.

---

## 3. Registration of new recipes

New recipe ids **MUST** use prefix `x402/` and a monotonic version suffix
(`/v1`, `/v2`, …). Each registration **MUST** document:

- Algorithm sufficient for an independent implementation.
- Input domain and output encoding.
- Whether it is suitable for SLA, delivery, resolution, or all.

Until registered here, custom recipe ids are **private** and MUST NOT
appear in public intent contracts.

---

## 4. References

| Reference | Purpose |
|---|---|
| `x402/sla-document/v1` | SLA byte commitment |
| `x402/delegated-authoring/v1` | Intent contract cites recipe id |
| `x402/oracles/resolution-envelope/v1` | Envelope key order |

---

**Document version:** v1.0
