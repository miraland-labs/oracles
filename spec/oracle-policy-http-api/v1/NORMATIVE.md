# Oracle Policy HTTP API — Version 1 (Normative)

**Specification identifier:** `x402/oracle-policy-http-api/v1`
**Document status:** Normative wire-level specification for the oracle
operator policy snapshot exposed at `GET /v1/policy`.
**Mount point:** `/v1/policy` on each oracle binary's HTTP server (sibling to
`/v1/registry/...`).

> Registry storage metadata lives in
> `x402/registry-http-api/v1` (`GET
> /v1/registry/info`). Policy and info serve different discovery needs;
> see §1.

---

## Abstract

Buyers and sellers query `/v1/policy` before funding to learn oracle tip
floors, guardian timing, and registered profiles. This endpoint is public
(no authentication).

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) /
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## 1. Relationship to `/v1/registry/info`

| Endpoint | Purpose |
|---|---|
| `GET /v1/registry/info` | Registry storage: backends, size caps, single `registered_profile_id`, `oracle_pubkey`. |
| `GET /v1/policy` | Operator economics and evaluation policy: tip floors, guardian margins, `registeredProfiles[]`. |

Clients **MAY** call both. For tip-floor preflight, `/v1/policy` is
authoritative.

---

## 2. `GET /v1/policy`

### 2.1 Request

No parameters. No authentication.

### 2.2 Response — `200 OK`

```json
{
  "operatorPubkey": "<base58>",
  "programId": "<sla-escrow-program-id-base58>",
  "tipFloorEnabled": true,
  "minVerdictTipDefaultRaw": 1000,
  "minVerdictTipByMintRaw": {
    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v": 5000
  },
  "guardianRejectSafetyMarginSec": 30,
  "guardianMaxRetryAttempts": 3,
  "registeredProfiles": ["x402/oracles/onchain-transfer/v1"]
}
```

| Field | Type | Notes |
|---|---|---|
| `operatorPubkey` | string | MUST equal the pubkey signing `ConfirmOracle`. |
| `programId` | string | sla-escrow program the oracle monitors. |
| `tipFloorEnabled` | boolean | If `false`, tip floor fields are `null` and any eligible job is evaluated. |
| `minVerdictTipDefaultRaw` | integer \| null | Minimum oracle tip in raw escrow mint units when no per-mint entry matches. |
| `minVerdictTipByMintRaw` | object \| null | Map mint base58 → minimum raw tip. |
| `guardianRejectSafetyMarginSec` | integer | Active Guardian: reject with timeout if less than this many seconds remain before `expires_at`. |
| `guardianMaxRetryAttempts` | integer | Max pipeline retries before dead-letter / timeout verdict. |
| `registeredProfiles` | string[] | Profile ids this binary evaluates. v1: exactly one element per binary. |

### 2.3 Tip floor evaluation

When `tipFloorEnabled` is `true`, the oracle **MAY** reject with reason
code `200` (`TIP_BELOW_OPERATOR_FLOOR`) if:

```text
expected_tip_raw = floor(payment.amount * payment.oracle_fee_bps / 10000)
required_tip_raw = minVerdictTipByMintRaw[payment.mint]
                   ?? minVerdictTipDefaultRaw
```

Reject when `expected_tip_raw < required_tip_raw`.

Buyers **SHOULD** evaluate this inequality using `accepts[].extra.oracleFeeBps`
and `amount` before signing `FundPayment`.

### 2.4 Effective floors when enabled

When `tipFloorEnabled` is `true`:

- If `minVerdictTipDefaultRaw` is JSON `null` and `minVerdictTipByMintRaw`
  has no entry for the payment mint, implementations **MAY** apply a
  documented default for known stablecoins (reference: USDC `5000` raw =
  `$0.005` at 6 decimals).
- `/v1/policy` **SHOULD** echo the **effective** default used by the worker
  in `minVerdictTipDefaultRaw` when a implicit default applies, so buyers
  need not read implementation source.

When `tipFloorEnabled` is `false`, floor fields **MUST** be JSON `null`.

---

## 3. Conformance

- Response field names use **camelCase** as shown.
- `registeredProfiles` **MUST** match the binary's compiled profile registry.
- When `tipFloorEnabled` is `false`, `minVerdictTipDefaultRaw` and
  `minVerdictTipByMintRaw` **MUST** be JSON `null`.

---

## 4. Versioning

Spec id: `x402/oracle-policy-http-api/v1`. Backward-compatible optional
fields may be added in v1 errata.

---

## 5. References

| Reference | Purpose |
|---|---|
| `x402/registry-http-api/v1` | Registry `/info` |
| `x402/sla-escrow-onchain-abi/v1` | Reason code 200, fee math |
| `x402/pr402-discovery/v1` | Buyer preflight |
| `oracle-common/src/server.rs` | Reference handler |

---

**Document version:** v1.0
**Last verified against reference implementation:** 2026-05-23
