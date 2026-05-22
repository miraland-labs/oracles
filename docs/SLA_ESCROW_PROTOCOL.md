# SLA-Escrow Protocol

This is the navigation hub for SLA-Escrow protocol documentation. The
normative content lives in versioned spec documents. This page tells you
which spec or guide answers your question.

## Spec layer (normative)

These four documents define the public contract every conformant
implementation must follow.

| Spec | Use when you need |
|---|---|
| [`spec/sla-escrow-protocol/v1`](../spec/sla-escrow-protocol/v1/NORMATIVE.md) | Per-actor obligations (buyer / seller / oracle / pr402) and the settlement matrix |
| [`spec/sla-escrow-onchain-abi/v1`](../spec/sla-escrow-onchain-abi/v1/NORMATIVE.md) | On-chain instruction set, account layouts, PDA seeds, event formats — for any-language integrators |
| [`spec/registry-http-api/v1`](../spec/registry-http-api/v1/NORMATIVE.md) | Wire-level HTTP contract for the oracle registry endpoints |
| [`spec/sla-document/v1`](../spec/sla-document/v1/NORMATIVE.md) | Cross-family JSON envelope and the bytes-to-hash binding |

Per-family verdict semantics (what makes a delivery pass or fail) live
under each oracle's `spec/<profile>/NORMATIVE.md`.

## Guide layer (implementer-facing)

These documents are how-to recipes for one role at a time. They cite the
spec layer for normative requirements and add tactical content
(commands, troubleshooting, FAQ).

| Guide | Audience |
|---|---|
| [`BUYER_GUIDE.md`](./BUYER_GUIDE.md) | Buyer agents (direct authoring + delegated/HTTP-402) |
| [`SELLER_GUIDE.md`](./SELLER_GUIDE.md) | Sellers running paid HTTP services |
| [`ORACLE_DEVELOPER_GUIDE.md`](./ORACLE_DEVELOPER_GUIDE.md) | Operators implementing a new profile |
| [`DEPLOYMENT.md`](./DEPLOYMENT.md) | Bringing up an oracle binary |
| [`OPERATIONS.md`](./OPERATIONS.md) | Day-2 oracle operations |
| [`ARCHITECTURE.md`](./ARCHITECTURE.md) | System architecture and component map |

## Quick answers

**What does each actor do?** → `sla-escrow-protocol/v1` §3–§6.

**Who can settle and when?** → `sla-escrow-protocol/v1` §7 (settlement matrix).

**What does the SLA JSON look like?** → `sla-document/v1` §5 + the
relevant per-family `NORMATIVE.md`.

**What HTTP calls do I make to the registry?** → `registry-http-api/v1`.

**What instruction bytes do I build for FundPayment / SubmitDelivery /
ConfirmOracle?** → `sla-escrow-onchain-abi/v1`. Also covers PDA seeds,
account layouts, event formats. Use this when integrating from a
non-Rust language, or when building a multi-cluster Rust binary that
can't pin to a single `sla-escrow-api` crate version.

**How is `sla_hash` computed?** → `sla-document/v1` §3.

**What resolution reason codes are defined?** → see the per-family
`NORMATIVE.md`. Standard codes 0–7 and 100–102 are interoperable;
custom ranges are partitioned per family (256–319 onchain-transfer,
320–383 file-delivery, 384+ reserved).

**Direct vs delegated SLA authoring?** → `sla-escrow-protocol/v1` §3.1.

## Implementations

The reference oracle binaries live under `oracles/oracle-*/` in this
repo and serve as the canonical example implementations of each
profile. The reference seller is `spl-token-balance-serverless`, which
implements delegated authoring for the `onchain-transfer/v1` profile.

## Changelog

- **2026-05-22**: Protocol doc refactored to a navigation hub.
  Normative content moved to `spec/sla-escrow-protocol/v1`,
  `spec/registry-http-api/v1`, and `spec/sla-document/v1`. Per-actor
  obligations, settlement matrix, registry HTTP contract, and SLA
  envelope rules now live in those three specs. Tracks on-chain
  program v0.4.0 (mainnet) / v0.2.11 (devnet) which made post-outcome
  settlement permissionless.
- **1.1** (2026-05-20): Protocol-aligned updates from devnet E2E
  validation. `paymentUidHex` canonical hex form for `payment_uid`;
  delegated authoring (seller uploads SLA on paid path);
  Active Guardian protective REJECT for unavailable artifacts;
  oracle's 10-minute reject safety margin vs program's 5-minute
  `delivery_cutoff_seconds`; `cluster` field required on
  `onchain-transfer` SLAs; escrow PDA as `payTo` for the sla-escrow
  scheme. These behaviors are now incorporated into the spec layer.
- **1.0**: Initial publication. Buyer-authored SLA with mandatory
  `payment_uid` and optional `buyer_nonce`; cross-payment replay
  protection; evidence-freshness lower bound; pr402 optional oracle
  health gate.
