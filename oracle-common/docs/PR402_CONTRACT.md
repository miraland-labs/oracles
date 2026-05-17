# pr402 Discovery Contract — Specification for the pr402 follow-up spec

This document fixes the JSON shapes pr402 will adopt to advertise the
multi-category oracle layer. **Implementing them is out of scope for this spec
(`multi-category-oracle-architecture`)** — pr402 has its own separate spec — but
publishing the contract here lets pr402's implementation reference us without
further negotiation.

The contract is normative for both directions:

* **Seller advertisement** — what a seller may put under `accepts[].extra` for
  the `sla-escrow` scheme (Section 1).
* **pr402 capabilities** — what pr402 advertises under
  `GET /api/v1/facilitator/capabilities` (Section 2).

Cross-references:

* Architectural rationale — [`design.md` § Buyer ↔ Oracle Discovery Contract](../../../.kiro/specs/multi-category-oracle-architecture/design.md#buyer--oracle-discovery-contract).
* Acceptance criteria — `requirements.md` Requirements 28, 29 and Properties
  P-CAP-1, P-CAP-2.

---

## 1. Seller advertisement: `accepts[].extra` for `scheme: "sla-escrow"`

```json
{
  "feePayer": "...",
  "oracleAuthorities": ["Pubkey1", "Pubkey2", "Pubkey3"],
  "oracleProfiles": [
    {
      "profileId": "x402/oracle/api-quality/v1",
      "operatorPubkey": "Pubkey1",
      "normativeSpecUrl": "https://github.com/miraland-labs/x402/blob/main/oracles/oracle-api-quality/spec/api-quality-v1/NORMATIVE.md",
      "registryBaseUrl": "https://registry.example.com/v1/registry"
    },
    {
      "profileId": "x402/oracle/onchain-transfer/v1",
      "operatorPubkey": "Pubkey2",
      "normativeSpecUrl": "https://github.com/miraland-labs/x402/blob/main/oracles/oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md",
      "supportedClusters": ["mainnet-beta", "devnet"],
      "supportedMints": ["Es9vMFr...", "USDT..."]
    },
    {
      "profileId": "x402/oracle/file-delivery/attestation/v1",
      "operatorPubkey": "Pubkey3",
      "normativeSpecUrl": "https://github.com/miraland-labs/x402/blob/main/oracles/oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md",
      "registryBaseUrl": "https://registry.example.com/v1/registry",
      "maxBlobBytes": 5368709120
    }
  ],
  "escrowProgramId": "...",
  "bankAddress": "..."
}
```

### 1.1 Required and optional fields per `oracleProfiles[]` entry

| Field                | Type                | Required | Notes                                                                                                  |
| -------------------- | ------------------- | -------- | ------------------------------------------------------------------------------------------------------ |
| `profileId`          | `string`            | yes      | One of the registered profile ids (e.g. `x402/oracle/api-quality/v1`).                                        |
| `operatorPubkey`     | base58 `string`     | yes      | Solana pubkey that signs `ConfirmOracle` for this profile.                                             |
| `normativeSpecUrl`   | URL `string`        | yes      | Permanent link to the family's NORMATIVE.md.                                                           |
| `registryBaseUrl`    | URL `string`        | optional | Where to GET / POST artifacts. Buyers / oracles fall back to their own configured mirror list when unset. |
| `supportedClusters`  | `string[]`          | optional | (onchain-transfer) advisory list — `oracle-onchain-transfer` rejects with `Custom(261)` regardless.    |
| `supportedMints`     | `string[]`          | optional | Advisory whitelist for downstream UIs.                                                                  |
| `maxBlobBytes`       | `number`            | optional | (file-delivery) advisory upper bound; the oracle's `ORACLE_REGISTRY_MAX_BLOB_BYTES` is authoritative.   |

### 1.2 Invariants pr402 MUST enforce

These are enforced when pr402 proxies a seller's `accepts[].extra`:

1. **Every `operatorPubkey` in `oracleProfiles[]` MUST also appear in
   `oracleAuthorities[]`** (Property P-CAP-1, Requirement 5.3 / 28.1). If the
   invariant is violated, pr402 SHOULD reject the advertisement with a clear
   error rather than serving it to buyers.

2. **No `operatorPubkey` may appear in two `oracleProfiles[]` entries.** v1
   binds one authority to one profile so the buyer's choice of authority
   deterministically selects the family the SLA must satisfy (design.md C10).

3. **`profileId` strings are matched by exact equality.** No prefix matches,
   no aliases (design.md C7, Property P-DISP-1).

4. **`oracleAuthorities[]` remains the authoritative list** for backwards
   compatibility with builders that don't know about `oracleProfiles[]`.

### 1.3 Buyer-side selection algorithm

Buyers SHOULD use:

```rust
fn select_oracle(seller_extra: &SellerExtra, desired_profile_id: &str) -> Option<Pubkey> {
    seller_extra
        .oracle_profiles
        .iter()
        .find(|p| p.profile_id == desired_profile_id)
        .map(|p| p.operator_pubkey)
}
```

A buyer that does not match a profile id MUST NOT silently fall back to
`oracleAuthorities[0]` — there is no guarantee the chosen authority handles
the family the buyer wants (Requirement 8.3).

---

## 2. pr402 capabilities: `GET /api/v1/facilitator/capabilities`

```json
{
  "...": "existing fields",
  "slaEscrowOracleProfiles": [
    {
      "profileId": "x402/oracle/api-quality/v1",
      "normativeSpecUrl": "https://github.com/miraland-labs/x402/blob/main/oracles/oracle-api-quality/spec/api-quality-v1/NORMATIVE.md",
      "defaultOperatorPubkey": "Pubkey1",
      "repositoryPath": "oracles/oracle-api-quality"
    },
    {
      "profileId": "x402/oracle/onchain-transfer/v1",
      "normativeSpecUrl": "https://github.com/miraland-labs/x402/blob/main/oracles/oracle-onchain-transfer/spec/onchain-transfer-v1/NORMATIVE.md",
      "defaultOperatorPubkey": "Pubkey2",
      "repositoryPath": "oracles/oracle-onchain-transfer"
    },
    {
      "profileId": "x402/oracle/file-delivery/attestation/v1",
      "normativeSpecUrl": "https://github.com/miraland-labs/x402/blob/main/oracles/oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md",
      "defaultOperatorPubkey": "Pubkey3",
      "repositoryPath": "oracles/oracle-file-delivery"
    }
  ]
}
```

### 2.1 Required and optional fields

| Field                  | Type                | Required | Notes                                                                                  |
| ---------------------- | ------------------- | -------- | -------------------------------------------------------------------------------------- |
| `profileId`            | `string`            | yes      | Canonical id of the family this entry advertises.                                      |
| `normativeSpecUrl`     | URL `string`        | yes      | Permanent link to the family's NORMATIVE.md.                                           |
| `defaultOperatorPubkey`| base58 `string`     | yes      | The operator pr402 will recommend if a seller doesn't specify per-profile authorities. |
| `repositoryPath`       | `string`            | yes      | Path within the x402 repo to the binary's source.                                      |
| `supportedClusters`    | `string[]`          | optional | Advisory.                                                                               |
| `supportedMints`       | `string[]`          | optional | Advisory.                                                                               |
| `maxBlobBytes`         | `number`            | optional | Advisory.                                                                               |
| `registryBaseUrl`      | URL `string`        | optional | Default registry endpoint pr402 advertises to buyers / sellers.                        |

### 2.2 Invariant pr402 MUST enforce

**Every `defaultOperatorPubkey` advertised in
`/capabilities.slaEscrowOracleProfiles[]` MUST be listed in pr402's configured
`ORACLE_AUTHORITIES` env list** (Property P-CAP-2, Requirement 29.2).

This ensures pr402 cannot recommend an operator pubkey it isn't aware of from
its own runtime config — preventing stale or typo'd entries from misleading
buyers.

### 2.3 No reachability promise

pr402 does NOT guarantee that the advertised `defaultOperatorPubkey` is
currently online. Buyers and sellers SHOULD probe the oracle's
`GET /health` (or a registry probe equivalent) before relying on it for a
critical payment. The contract here is one of *configuration consistency*,
not *liveness*.

---

## 3. What this contract does NOT cover

* The HTTP shape pr402 uses to *build* the SLA-escrow `FundPayment`
  transaction (`POST /build-sla-escrow-payment-tx`). That belongs to pr402's
  own spec.
* Multi-oracle adjudication / quorum. v1 binds one oracle authority per
  payment on-chain; quorum, if needed, lives in pr402 or a future wrapper
  spec.
* `compute-result/v1` and other future families. New entries are additive —
  pr402 can append to `slaEscrowOracleProfiles[]` without breaking buyers
  that don't recognize the new `profileId`.

---

## 4. Versioning

* Adding a new optional field to either Section 1 or Section 2 is **not** a
  breaking change.
* Adding a new required field, or changing the meaning of an existing field,
  IS a breaking change. New profile ids are the preferred extension path
  (`x402/<family>/<profile>/v2`); the JSON shape itself should change as
  rarely as possible.
