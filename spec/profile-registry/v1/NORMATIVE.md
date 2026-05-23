# Oracle Profile Registry and Discovery — Version 1 (Normative)

**Specification identifier:** `x402/profile-registry/v1`
**Document status:** Normative rules for publishing and discovering oracle profiles.
**Scope:** Operator registration, seller advertisement, buyer selection.

---

## Abstract

A profile exists in the ecosystem only when it is discoverable through a
consistent set of surfaces. This document ties together oracle operator
obligations and seller/facilitator advertisement fields.

---

## 1. Operator surfaces (authoritative)

Each oracle binary **MUST** publish:

| Surface | Field | Value |
|---|---|---|
| `GET /v1/registry/info` | `registered_profile_id` | Canonical `profile_id` |
| `GET /v1/registry/info` | `oracle_pubkey` | Same key as `FundPayment.oracle_authority` |
| `GET /v1/policy` | `registeredProfiles` | `[profile_id]` (exactly one in v1) |
| `GET /v1/policy` | `operatorPubkey` | Same as `oracle_pubkey` |
| Optional | `normativeSpecUrl` | HTTPS link to profile NORMATIVE.md |

Cluster-pinned profiles **MUST** also publish `cluster` on `/info`.

---

## 2. Facilitator surfaces (defaults)

pr402 **MAY** publish defaults on:

- `GET /api/v1/facilitator/capabilities` → `slaEscrowOracleProfiles[]`
- `GET /api/v1/facilitator/supported` → baseline sla-escrow `extra` (not profile-specific)

Each capabilities row **SHOULD** include:

| Field | Purpose |
|---|---|
| `profileId` | Canonical id |
| `defaultOperatorPubkey` | Suggested oracle for buyers |
| `normativeSpecUrl` | Profile spec link |
| `repositoryPath` | Informative source path |

Facilitator defaults **MUST NOT** override per-resource seller advertisement.

---

## 3. Seller advertisement (per resource)

For each sla-escrow `accepts[]` line, sellers **SHOULD** include:

```json
"extra": {
  "oracleAuthorities": ["<pubkey>", "..."],
  "oracleProfiles": [{
    "profileId": "x402/oracles/onchain-transfer/v1",
    "operatorPubkey": "<same-as-oracleAuthorities-entry>",
    "normativeSpecUrl": "https://…",
    "registryBaseUrl": "https://oracle.example"
  }]
}
```

Invariants (`x402/pr402-discovery/v1` §3):

1. Every `oracleProfiles[].operatorPubkey` **MUST** appear in `oracleAuthorities[]`.
2. No duplicate `operatorPubkey` across entries.
3. Buyers **MUST** select authority by matching `profileId`, not array index.

Delegated authoring fields (`commitVariant`, `commitMaterial`, …) are orthogonal;
see `x402/delegated-authoring/v1`.

---

## 4. Buyer selection algorithm

```text
1. Choose desired profile_id (from intent contract or product docs).
2. Find oracleProfiles[] entry where profileId == desired.
3. Set oracleAuthority = entry.operatorPubkey.
4. Assert oracleAuthority ∈ accepts.extra.oracleAuthorities.
5. Optional: GET {registryBaseUrl}/v1/policy and confirm registeredProfiles.
6. Optional: GET {registryBaseUrl}/v1/policy for tip-floor preflight.
```

**MUST NOT** default to `oracleAuthorities[0]` without profile matching.

---

## 5. New profile onboarding

To register a new public profile:

1. Author profile NORMATIVE per `x402/profile-authoring/v1`.
2. Reserve reason codes per `x402/reason-codes/v1`.
3. Deploy oracle binary; verify `/info` + `/policy`.
4. Request facilitator operator add row to `slaEscrowOracleProfiles` (if using pr402).
5. Document seller `oracleProfiles[]` snippet for integrators.
6. Add informative binding under `spec/informative/bindings/` if shipping reference seller.

---

## 6. References

| Reference | Purpose |
|---|---|
| `x402/pr402-discovery/v1` | Wire fields |
| `x402/registry-http-api/v1` | Registry HTTP |
| `x402/oracle-policy-http-api/v1` | Policy HTTP |

---

**Document version:** v1.0
