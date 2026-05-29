# x402 Oracle Specs — Layered Architecture

**Authority model:** Normative specifications in this tree are the public
contract for the pr402/x402 sla-escrow ecosystem. Implementations conform
to specs; specs are not reverse-engineered from any single deployment.

**Stability rule:** Layer 0 documents change only for security fixes,
clarifications, or backward-compatible errata. New domains extend Layer 2
profiles or Layer 3 bindings — they do not redefine Layer 0.

---

## Layer map

```
Layer 0 — Core (small, stable, domain-agnostic)
├── sla-document/v1              SLA envelope + byte commitment
├── serialization-recipes/v1     Named deterministic serializers
├── resolution-envelope/v1       Oracle attestation digest
├── sla-escrow-onchain-abi/v1    Solana program wire format
├── sla-escrow-protocol/v1       Buyer / seller / oracle obligations
├── reason-codes/v1              resolution_reason registry
└── oracle-evaluation-invariants/v1  Cross-family evaluation rules

Layer 1 — Interaction patterns (HTTP & discovery, still domain-agnostic)
├── delegated-authoring/v1       Purchase intent + HTTP 402 commit flow
├── registry-http-api/v1         Content-addressed artifact registry
├── oracle-policy-http-api/v1    Operator policy (tip floors, guardian)
├── pr402-discovery/v1           Facilitator + seller advertisement
├── profile-authoring/v1         How to write a Layer 2 profile (meta)
└── profile-registry/v1          Operator + seller discovery surfaces

Layer 2 — Verdict profiles (domain-specific; sibling crates in this repo)
└── e.g. `x402/oracles/onchain-transfer/v1` — see Layer 2 index below

Layer 3 — Informative (non-normative reference bindings & examples)
└── informative/bindings/*/BINDING.md
```

---

## Layer 2 profile index

| Profile id | Delivery model | Reason range | Reference maturity |
|---|---|---|---|
| `x402/oracles/onchain-transfer/v1` | JSON evidence + RPC verify | 256–319 | **Reference** (production) |
| `x402/oracles/rwa-transfer/v1` | Token-2022 RWA delivery + hook metadata | 448–479 | **Draft** (spec + planned `oracle-rwa-transfer`) |
| `x402/oracles/api-quality/v1` | JSON evidence (seller-attested) | TBD (480+) | Experimental |
| `x402/oracles/file-delivery/attestation/v1` | Raw blob streaming | 320–383 | Draft (streaming WIP) |

New partners: start with `x402/profile-authoring/v1`.

---

## Reading guide by role

| Role | Start with | Then |
|---|---|---|
| **Buyer / agent** | L0 `sla-escrow-protocol`, L1 `delegated-authoring` | L1 `pr402-discovery`, profile NORMATIVE for deliverable shape |
| **Seller** | L0 `sla-escrow-protocol`, L1 `delegated-authoring`, L1 `registry-http-api` | Publish a Layer 3 binding or intent contract for your product |
| **Oracle partner** | L1 `profile-authoring`, L0 `resolution-envelope`, L1 `registry-http-api` | Copy Layer 2 reference profile; register via `profile-registry` |
| **Facilitator (pr402)** | L1 `pr402-discovery`, L0 `sla-escrow-onchain-abi` | OpenAPI for extended endpoints |

---

## Conformance

A conformant implementation **MUST** satisfy all Layer 0 and Layer 1
normatives for the features it advertises. Layer 2 applies per registered
profile. Layer 3 documents are **informative** — useful references, not
requirements.

Conformance tests (future) will live outside this tree.

---

## Document types

| Suffix / path | Status |
|---|---|
| `*/NORMATIVE.md` | Normative (RFC 2119 keywords) |
| `informative/**/*.md` | Informative only |
| `informative/**/BINDING.md` | Reference binding of Layer 0–1 to one product |

---

**Maintainers:** Ecosystem specs are versioned by identifier
(`x402/<name>/v1`). Substantive breaking changes require `v2`.
