# Resolution Envelope — Version 1 (Normative)

**Specification identifier:** `x402/oracles/resolution-envelope/v1`
**Document status:** Normative envelope hashed into on-chain `resolution_hash`.
**Serialization recipe:** `x402/envelope-json/v1` per
`x402/serialization-recipes/v1`.

---

## Abstract

Oracles **MUST** set `resolution_hash = SHA-256(B_env)` where `B_env` is the
UTF-8 output of recipe `x402/envelope-json/v1` over the object below.
Per-family data lives in `details` only — **never** the raw SLA document.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in RFC 2119 / RFC 8174.

---

## 1. Envelope object (fixed key order)

| Key | Type |
|---|---|
| `profile` | string — MUST be `"x402/oracles/resolution-envelope/v1"` |
| `evaluatorProfile` | string — see §2 |
| `paymentUid` | string — 64 lowercase hex chars |
| `paymentPubkey` | string — base58 Payment PDA |
| `slaHash` | string — 64 lowercase hex chars |
| `deliveryHash` | string — 64 lowercase hex chars |
| `approved` | boolean |
| `resolutionReason` | integer — u16 reason code |
| `details` | object — per Layer 2 profile §7 |

No other top-level keys in v1. Serialize with `x402/envelope-json/v1` (fixed
key order above; compact JSON).

---

## 2. `evaluatorProfile` values

| Value | When |
|---|---|
| Profile id (e.g. `x402/oracles/onchain-transfer/v1`) | Normal family evaluation |
| `x402/oracles/guardian/v1` | Guardian timeout / protective reject |
| `x402/oracles/economic-refusal/v1` | Tip below operator floor |

Non-profile evaluator ids **MUST NOT** reuse a verdict profile's id.

---

## 3. `details` object

- **MUST** conform to the active profile's §7 schema in its Layer 2 NORMATIVE.
- **MUST NOT** embed the full SLA or delivery documents.
- **MUST** use camelCase keys as declared by the profile spec.
- Nested objects **SHOULD** use stable key order documented by the profile;
  when unspecified, lexicographic sort at each object boundary is RECOMMENDED.

Indexers recomputing `resolution_hash` **MUST** use the profile §7 schema,
not implementation-internal structs.

---

## 4. Conformance

- Oracle: `approved` and `resolutionReason` **MUST** match `ConfirmOracle`
  body and on-chain `resolution_state`.
- Indexer: recompute hash from published envelope bytes.
- Profile author: publish §7 schema in Layer 2 NORMATIVE before advertising profile.

---

## 5. Versioning

New envelope schema → new `profile` string and new spec version.

---

## 6. References

| Reference | Purpose |
|---|---|
| `x402/serialization-recipes/v1` | `x402/envelope-json/v1` |
| `x402/profile-authoring/v1` | §7 requirements |
| `x402/reason-codes/v1` | `resolutionReason` |
| `x402/sla-escrow-onchain-abi/v1` | `ConfirmOracle` |

---

**Document version:** v2.1
