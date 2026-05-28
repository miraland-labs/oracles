# x402/oracles/logistics-attestation/v1 — Normative spec (draft)

**Profile id:** `x402/oracles/logistics-attestation/v1`

## Purpose

Adjudicate physical-goods shipment for sla-escrow payments. The on-chain program stores opaque hashes only; all semantics live off-chain in this profile.

## SLA document

Required fields:

| Field | Type | Description |
|-------|------|-------------|
| `profileId` | string | Must equal `x402/oracles/logistics-attestation/v1` |
| `orderReference` | string | pr402-link id or merchant order ref |
| `merchantWallet` | string | Base58 merchant pubkey |
| `destinationCountry` | string | ISO 3166-1 alpha-2 |
| `maxTransitDays` | integer | Max calendar days from ship to delivery |
| `deliveryDeadlineUnix` | integer | Unix seconds; must be ≤ fund time + 30d |
| `allowedCarriers` | string[] | Lowercase carrier ids |
| `requireSignature` | boolean | Optional; default false |

## Evidence document

Required fields:

| Field | Type | Description |
|-------|------|-------------|
| `profileId` | string | Same as SLA |
| `trackingNumber` | string | Carrier tracking id |
| `carrier` | string | Must be in SLA allowlist |
| `events` | array | `{ status, at, location? }` |
| `attestationSource` | string | `merchant_registry` \| `carrier_api` |
| `fetchedAt` | string | RFC3339 |

## Evaluation (v1)

**Approve (release):** Last event `status` is `delivered` (case-insensitive), timestamp ≤ `deliveryDeadlineUnix`, carrier allowed, `orderReference` matches SLA.

**Reject:** No delivery by deadline, carrier not allowed, tracking mismatch.

Custom resolution codes: `512` = delivery_timeout, `513` = invalid_carrier, `514` = tracking_mismatch.

## Operator

Binary registers under profile id at boot. Merchants set `oracleAuthority` to operator pubkey in `POST /api/v1/links`.

## References

- [pr402-link EVOLUTION_RFC.md](../../pr402-link/docs/EVOLUTION_RFC.md)
- [Multi-category oracle architecture](../../.kiro/specs/multi-category-oracle-architecture/design.md)
