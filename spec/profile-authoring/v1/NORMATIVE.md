# Oracle Profile Authoring — Version 1 (Normative)

**Specification identifier:** `x402/profile-authoring/v1`
**Document status:** Normative meta-spec for Layer 2 verdict profiles.
**Scope:** Required structure, identifiers, and conformance checklist for
new domain-specific arbitration oracles.

> Wire surfaces shared by every oracle binary: Layer 0 in
> `spec/README.md` (layer index, this repo). This document defines what each **profile**
> MUST publish beyond Layer 0.

---

## Abstract

A **profile** is a versioned rule family (`x402/oracles/<family>/v<n>`) that
defines how an oracle maps `(sla_hash, delivery_hash)` bytes to an approval
decision and a `resolution_hash`. This meta-spec is the checklist partners
follow when authoring a new profile — independent of implementation language.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in RFC 2119 / RFC 8174.

---

## 1. Deployment model

| Rule | Requirement |
|---|---|
| Profiles per binary | Exactly **one** `profile_id` per oracle process |
| Authority key | One Solana keypair signs `ConfirmOracle` for that profile |
| Cluster | Cluster-pinned profiles (e.g. onchain-transfer) run **one binary per cluster** |
| Discovery | Operator publishes profile via `x402/profile-registry/v1` |

Multi-profile binaries are **non-conformant** in v1.

---

## 2. Profile identifier

Format:

```text
x402/oracles/<family>/<version>
```

Examples:

- `x402/oracles/onchain-transfer/v1`
- `x402/oracles/file-delivery/attestation/v1`
- `x402/oracles/api-quality/v1`

Rules:

- **MUST** match exactly in SLA `profile_id`, `/v1/policy.registeredProfiles[]`,
  and seller `oracleProfiles[].profileId`.
- **MUST NOT** use aliases, prefix matching, or case variants.
- Breaking evaluation semantics **MUST** bump version (`…/v2`), not silently change `v1`.

---

## 3. Required profile document sections

Each profile **MUST** ship a normative document at:

```text
oracles/oracle-<family>/spec/<profile>/NORMATIVE.md
```

with these sections:

| § | Section | Content |
|---|---|---|
| 1 | Introduction | What deliverable class this profile adjudicates |
| 2 | Terminology + trust model | What the oracle **proves** vs **does not prove** |
| 3 | Cryptographic binding | What bytes `sla_hash` and `delivery_hash` commit to |
| 4 | SLA document | JSON schema path + field semantics table |
| 5 | Delivery evidence | JSON schema **or** raw blob rules (see §4) |
| 6 | Evaluation semantics | Ordered check battery; first failure wins |
| 7 | Resolution `details` | JSON schema embedded in `x402/oracles/resolution-envelope/v1` |
| 8 | Reason codes | Custom codes allocated from `x402/reason-codes/v1` |
| 9 | Versioning | When to bump profile id vs errata |

Optional but RECOMMENDED: machine-readable JSON Schema under `schema/`.

---

## 4. Delivery commitment models (choose one)

Profiles **MUST** declare which model they use:

| Model | `delivery_hash` binds to | Evidence fetch | Example profile |
|---|---|---|---|
| **JSON evidence** | UTF-8 JSON document bytes | Registry GET → parse JSON | onchain-transfer, api-quality |
| **Raw blob** | Raw binary blob bytes | Streaming hash verify | file-delivery attestation |

Mixing models within one profile is **non-conformant**.

---

## 5. Evidence fetcher class

| Class | When | Requirements |
|---|---|---|
| `RegistryJsonFetcher` | JSON evidence model | Re-verify SHA-256 before parse |
| `RegistryStreamingFetcher` | Raw blob model | Incremental SHA-256; fail closed on mismatch |

The profile NORMATIVE **MUST** name its fetcher class and max size constraints.

---

## 6. Cross-family invariants

Profiles **SHOULD** implement applicable rules from
`x402/oracle-evaluation-invariants/v1`:

- Byte-identical SLA/delivery fetch
- `payment_uid` binding when SLA includes `payment_uid`
- Freshness lower bound when `Payment.created_at` is known
- Cross-payment replay keys when operator runs a ledger

Profiles **MAY** document explicit waivers with rationale.

---

## 7. Resolution envelope obligations

Every profile **MUST**:

1. Set top-level envelope per `x402/oracles/resolution-envelope/v1`.
2. Set `evaluatorProfile` to this profile's `profile_id` for normal verdicts.
3. Populate `details` per profile §7 schema — **not** the raw SLA document.
4. Allocate `resolutionReason` from `x402/reason-codes/v1`.

Non-profile paths (`guardian`, `economic-refusal`) use evaluator ids defined in
resolution-envelope §2.

---

## 8. Operator HTTP surfaces (inherited from Layer 0)

Implementations **MUST** expose without profile-specific variation:

- Full `x402/registry-http-api/v1`
- `x402/oracle-policy-http-api/v1`
- Chain monitor → worker pipeline per `x402/sla-escrow-protocol/v1` §5

---

## 9. Conformance checklist (new partner)

Before advertising a profile:

- [ ] Profile id registered in `/v1/policy` and `/v1/registry/info`
- [ ] SLA + delivery JSON Schemas published (or blob rules documented)
- [ ] Reason code range reserved in reason-codes registry
- [ ] Resolution `details` schema documented and hash-stable
- [ ] Evaluation order documented (first failure → reason code)
- [ ] Trust model section explicit about adversarial sellers
- [ ] Seller registration path documented (registry bearer flow)
- [ ] Listed in facilitator/seller discovery per profile-registry

---

## 10. References

| Reference | Purpose |
|---|---|
| `x402/oracle-evaluation-invariants/v1` | Cross-family rules |
| `x402/reason-codes/v1` | Code allocation |
| `x402/profile-registry/v1` | Discovery |
| `x402/oracles/resolution-envelope/v1` | Hash recipe |
| `x402/sla-document/v1` | SLA envelope |

---

**Document version:** v1.0
