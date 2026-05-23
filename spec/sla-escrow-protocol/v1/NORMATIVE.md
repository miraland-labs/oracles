# SLA-Escrow Protocol — Roles and Operations, Version 1 (Normative)

**Specification identifier:** `x402/sla-escrow-protocol/v1`
**Document status:** Normative specification for the four-role interaction
that surrounds the on-chain `sla-escrow` program at program version
`0.4.0` and later.
**Deployed program addresses:**

- Solana mainnet-beta: `SEscZ6n23pVak34xipBKoGCikHUj3w6XPNyty4rHprJ`
- Solana devnet: `s5zkKiy8FD9nFdAhQZoHHV3G8s4QCPzE4cR9U4Hr4ZH`

**Scope:** Required and optional operations per actor (buyer, seller, oracle,
pr402 facilitator) plus settlement ownership rules.

> For per-family verdict semantics, see the profile-specific normatives
> under `Layer 2 profile normatives (see `spec/sla-document/v1/NORMATIVE.md` §5 profile index)`. For HTTP 402
> purchase intent and delegated authoring, see
> `x402/delegated-authoring/v1`. For
> pr402 wire formats, see
> `x402/pr402-discovery/v1`. The on-chain
> program is the authoritative source for validation logic; this document
> is normative for actor obligations and settlement triggers off-chain
> and at the public instruction surface.

---

## Abstract

This document specifies the off-chain and on-chain obligations of each actor
participating in an SLA-escrow payment lifecycle. Conformance to this spec
enables interoperable buyers, sellers, oracles, and facilitators that
implement the protocol independently against the same on-chain program.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) /
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## 1. Introduction

An SLA-escrow payment is a four-party interaction:

| Actor | Role |
|---|---|
| **Buyer** | Funds the payment, authors the SLA, holds settlement preference |
| **Seller** | Delivers the service, submits delivery evidence on-chain |
| **Oracle** | Verifies delivery against SLA, signs `ConfirmOracle` verdict |
| **pr402 facilitator** | OPTIONAL HTTP intermediary that builds funding transactions and (OPTIONALLY) drives settlement |

The on-chain program enforces invariants (state machine, fund destinations,
authorization gates). Everything else — SLA authoring, delivery upload,
oracle selection, settlement triggering — lives in actor responsibilities
specified here.

This spec covers what each actor MUST, SHOULD, and MAY do at and around the
public instruction surface. Program-internal validation logic is enforced
by the deployed program; this spec restates the externally observable
guarantees in §8.

---

## 2. Terminology

| Term | Definition |
|---|---|
| **SLA document** | UTF-8 JSON specifying the agreed deliverable. Hashed (SHA-256 over canonical bytes) into `sla_hash` on `FundPayment`. |
| **Delivery evidence** | UTF-8 JSON the seller produces after performing the work. Hashed into `delivery_hash` on `SubmitDelivery`. |
| **Resolution envelope** | Per-profile JSON the oracle hashes into `resolution_hash` on `ConfirmOracle`. |
| **`payment_uid`** | 32-byte unique identifier chosen at funding time. Seeds the `Payment` PDA. |
| **Profile** | A versioned per-family rule set (e.g. `x402/oracles/onchain-transfer/v1`). |
| **Registry** | Oracle-hosted HTTP service (`/v1/registry/...`) where SLA and delivery JSON are uploaded and content-addressed by SHA-256. |
| **Post-outcome** | Payment state where `resolution_state ∈ {1, 2}` (oracle has rendered a verdict) OR `now > expires_at` (TTL elapsed). |
| **Pre-outcome** | Payment state where `resolution_state == 0` AND `now ≤ expires_at`. |

### 2.1 Cryptographic primitives

- **Hashing**: SHA-256 (32 bytes) for `sla_hash`, `delivery_hash`,
  `resolution_hash`. Hex encoding when transmitted as text.
- **Signing**: Ed25519 over Solana's standard pubkey/signature scheme.
- **Encoding**: SLA and delivery artifacts commit to **specific UTF-8
  bytes** per `sla-document/v1` §3. Direct authoring uses buyer-supplied
  bytes (`raw-bytes`). Delegated authoring uses a named serialization
  recipe declared in the seller's intent contract (see
  `delegated-authoring/v1` §4).

---

## 3. Buyer obligations

### 3.1 Authoring patterns

The protocol supports two SLA authoring patterns. A buyer **MUST**
follow exactly one for any given payment.

#### 3.1.1 Direct authoring

The buyer constructs the SLA JSON locally, hashes it, and transmits the
bytes (and only those bytes) to the seller. Used when the buyer's
tooling is sophisticated enough to construct a profile-conforming SLA
without seller assistance.

A buyer **MUST**:

1. Author an SLA document conforming to the profile's per-family
   `NORMATIVE.md` (e.g., `onchain-transfer-v1` for SPL transfers).
2. Compute `sla_hash` over the exact UTF-8 bytes the buyer intends to
   commit to (per `sla-document/v1` §3).
3. Transmit those exact bytes to the seller for upload to the registry.

#### 3.1.2 Delegated authoring (HTTP 402 flow)

Used when the buyer expresses **purchase intent** on an unpaid HTTP request
instead of supplying final SLA bytes. Rules: `x402/delegated-authoring/v1`.

A buyer **MUST**:

1. Obtain the seller's **intent contract** and send all required intent
   parameters on the unpaid request.
2. Receive HTTP 402 with scheme `sla-escrow` per `pr402-discovery/v1`.
3. Follow the declared **commit variant** (`buyer-commit` or
   `seller-precommit`) to determine who computes `sla_hash` before
   `FundPayment`.
4. Verify **deliverable terms** (not only escrow principal) match intent
   before signing.
5. Select `oracle_authority` from the 402 advertisement.

Example bindings (informative, non-normative): `x402/informative/bindings/`.

#### 3.1.3 Common to both patterns

Regardless of authoring pattern, a buyer **MUST**:

1. Choose `oracle_authority` from a set the buyer trusts. Once bound
   by `FundPayment`, the oracle authority is permanently associated
   with this payment (the program does not support rotation). In
   delegated authoring, the buyer selects from the
   `oracleAuthorities` array in the 402 response.
2. Choose a `payment_uid` that is unique within the buyer's namespace.
   Prefer 32 raw bytes encoded as 64 lowercase hex (`paymentUidHex` in
   pr402 build). The on-chain program does not enforce cross-buyer
   uniqueness; PDA collision is the buyer's responsibility. In
   **`seller-precommit`** delegated authoring, the seller MAY propose
   `paymentUid` in the 402 response; in **`buyer-commit`**, the buyer
   MUST generate `payment_uid` before building the SLA.
3. Construct and sign `FundPayment`, transferring tokens into escrow
   with the `(payment_uid, sla_hash, oracle_authority, seller, mint,
   amount, ttl_seconds)` arguments. The on-chain payment is bound to
   the buyer's signed `sla_hash` regardless of how that hash was
   produced or what the seller relays afterward.

### 3.2 SHOULD

A buyer **SHOULD**:

1. Before signing `FundPayment`, issue `HEAD /v1/registry/{sla_hash_hex}`
   against the oracle and confirm `200 OK`. A `404` indicates the seller
   did not actually upload (or uploaded different bytes); the buyer
   SHOULD abort funding to avoid paying gas for a flow the oracle will
   reject as `EvidenceUnavailable`. This check is liveness-only —
   correctness is already provided by §3.1.3, which commits the
   on-chain payment to the buyer's signed hash.
2. **For delegated authoring (§3.1.2)**: verify the seller's
   `slaHash` reflects the buyer's intent before signing. The
   recommended approaches, in increasing strength:
   - Trust a published seller template (the seller documents their SLA
     template; the buyer trusts the seller to populate it correctly).
     Fastest; weakest.
   - Recompute locally from the published template (the buyer
     reconstructs the SLA bytes from the same parameters and the
     seller's documented template, hashes, compares). Strongest.
   - Fetch the SLA bytes from the registry post-funding via
     `GET /v1/registry/{sla_hash_hex}` and confirm the JSON matches
     intent. Catches divergence after-the-fact, when the buyer can
     still self-refund pre-delivery (subject to cooldown).
3. Choose `ttl_seconds` long enough that the seller has reasonable time to
   deliver AND the oracle has at least `delivery_cutoff_seconds` (default
   300s) of evaluation window. Recommended floor: 600s for AI workloads,
   substantially longer for human-mediated work.
4. Inspect the oracle's `GET /v1/policy` response before funding to
   confirm:
   - the oracle accepts the buyer's chosen `oracle_fee_bps`;
   - `tipFloorEnabled` policy is acceptable;
   - the profile the buyer intends to use appears in
     `registeredProfiles`. Each oracle binary registers exactly one
     profile, so this list contains a single entry; multi-profile
     deployments run multiple binaries with distinct
     `oracle_authority` keypairs.
5. Cache the local SLA bytes (direct authoring) or the
   parameters-plus-template needed to reproduce them (delegated
   authoring) until at least one terminal state (`Released` or
   `Refunded`) has been observed. Without these, the buyer cannot
   prove what they paid for if the registry copy becomes unavailable.

### 3.3 MAY

A buyer **MAY**:

1. Use a pr402 facilitator's `POST /build-sla-escrow-payment-tx` endpoint
   to obtain a pre-built `FundPayment` transaction shell. The facilitator's
   output is a convenience; the buyer remains responsible for signing.
2. Trigger `ReleasePayment` themselves once the oracle has approved (the
   protocol permits any signer post-approval).
3. Trigger `RefundPayment` themselves once the oracle has rejected, or
   after `refund_cooldown_seconds` has elapsed pre-outcome.
4. Extend the payment TTL via `ExtendPaymentTTL` while the payment remains
   funded and not expired. **MUST NOT** be used after delivery to
   circumvent the oracle window — the program rejects extensions that push
   total TTL beyond `MAX_TTL_SECONDS`.

### 3.4 MUST NOT

A buyer **MUST NOT**:

1. Reuse a `payment_uid` for a different SLA. Distinct payments require
   distinct uids.
2. Modify the SLA document after `FundPayment` has been confirmed. The
   `sla_hash` is bound; mutation invalidates the binding.

### 3.5 Rationale

- **Why two authoring patterns**: direct authoring is the protocol's
  trust-minimal default — the buyer holds and hashes the bytes they
  commit to. Delegated authoring exists because real x402 HTTP
  services need a buyer-friendly UX where the buyer sends
  intent-bearing parameters and gets a 402 back, without the buyer's
  client assembling per-profile JSON. Both patterns are equally safe
  on the funds dimension because correctness comes from the on-chain
  commit, not from who authored the bytes.
- **Why the buyer holds `oracle_authority` selection**: trust must be
  explicit. A protocol that lets sellers choose the oracle (rather
  than offer a list the buyer chooses from) would allow collusion.
  The program enforces this by checking the oracle's signature on
  `ConfirmOracle` against the value bound at `FundPayment`.
- **Why `payment_uid` uniqueness is the buyer's problem**: PDAs are
  derived from `(seed, payment_uid, bank_pda)`. The program returns
  `Account already in use` on collision. Centralized uniqueness would
  require a sequencer; uid choice in buyer space (UUID, hash-based)
  is simpler and equally safe. In delegated authoring the seller
  proposes; the buyer accepts by signing.
- **Why the on-chain commit alone provides correctness**: `FundPayment`
  binds `payment.sla_hash` to whatever value the buyer signs.
  Regardless of authoring pattern, a seller who substitutes different
  bytes after the buyer signs cannot redirect funds (destination is
  `payment.seller`, hardcoded), and produces a flow whose oracle
  outcome will fetch the buyer's signed hash and find either the
  intended bytes (success path) or different bytes / 404 (rejection
  path leading to refund). The buyer's funds are never bound to
  seller-controlled content.
- **Why delegated authoring needs intent verification (§3.2 #2)**:
  the on-chain commit protects funds, not intent. A dishonest seller
  can produce an SLA whose hash the buyer signs but whose terms
  diverge from what the buyer expressed — e.g., a smaller `min_amount`
  than parameters implied. The oracle will then approve a delivery
  the buyer would consider non-conforming. Intent verification
  (recompute or registry-fetch) closes this gap. The MUST/SHOULD
  asymmetry is deliberate: this risk applies only to delegated
  authoring, and a buyer who chose delegated authoring presumably
  has reduced tooling — making intent-verification SHOULD rather
  than MUST keeps the protocol usable.
- **Why the registry HEAD is SHOULD, not MUST**: skipping it does not
  compromise security. The worst case is a wasted gas fee on a doomed
  flow that will eventually refund. A compliant implementation gains
  nothing by skipping the HEAD; the cost is one HTTP round-trip and
  the failure mode is real (network glitches, registry quotas,
  seller-side bugs).

---

## 4. Seller obligations

### 4.1 Authoring patterns (matching §3.1)

A seller **MUST** implement at least one of the two SLA authoring
patterns. Implementing both is permitted; in that case the seller
MUST advertise which pattern is in use per request (typically via
the request shape — direct authoring receives SLA bytes in the
request body; delegated authoring receives intent-bearing parameters
in the request URL or body).

### 4.2 MUST

A seller **MUST**:

1. Register with each oracle they intend to use, obtaining a bearer token
   via the `seller/challenge` + `seller/register` handshake (see
   `registry-http-api/v1`).
2. Obtain the SLA bytes by exactly one of:
   - **Direct authoring path**: receive the buyer's SLA bytes through
     the seller's HTTP service. Bytes are canonical per
     `sla-document/v1`; the seller MUST NOT re-canonicalize.
   - **Delegated authoring path**: produce the SLA bytes from the
     buyer's intent-bearing parameters using the seller's
     deterministic SLA template (§4.6).
3. Upload the SLA bytes to the chosen oracle's registry via
   `POST /v1/registry/sla` with the seller's bearer token. The registry
   returns the SHA-256 of the uploaded bytes. The seller relays this
   hash back to the buyer:
   - in direct authoring: along with any service-specific response;
   - in delegated authoring **`seller-precommit`**: relay `slaHash` in
     `accepts[].extra` on the 402 response;
   - in delegated authoring **`buyer-commit`**: upload after payment
     verification on the paid path (hash already on-chain from buyer's
     `FundPayment`).
4. Perform the work described in the SLA document.
5. After completion, produce delivery evidence per the profile's
   per-family normative (e.g., `tx_signature` for `onchain-transfer/v1`).
6. Upload the delivery evidence to the same oracle's registry via
   `POST /v1/registry/delivery`, obtaining the `delivery_hash`.
7. Sign and submit `SubmitDelivery` on-chain with the `delivery_hash`,
   no later than `delivery_cutoff_seconds` before `expires_at`. Submitting
   later results in `DeliveryTooLateForOracle`.

### 4.3 SHOULD

A seller **SHOULD**:

1. Reject SLA documents whose deliverable is outside the seller's
   capability before accepting funding. The program does not enforce SLA
   feasibility; an infeasible SLA leads to oracle rejection and the seller
   bears the gas cost of the failed flow.
2. Set up failure logging for `SubmitDelivery` failures to detect
   delivery_cutoff misses early.
3. Respect the oracle's `tipFloorEnabled` policy. If the buyer's
   `oracle_fee_bps` is below the oracle's published floor, expect the
   oracle to refuse the verdict; the seller SHOULD reject the SLA at
   intake rather than discover this at delivery time.

### 4.4 MAY

A seller **MAY**:

1. Trigger `ReleasePayment` after the oracle approves, taking the gas
   cost. This is the canonical actor for the post-approval release path
   and is the recommended default.
2. Trigger `RefundPayment` for buyer-initiated cancellation pre-outcome
   (the program permits seller as part of the buyer/seller/admin set on
   the not-expired pre-outcome path).
3. Use the bearer token's `seller/rotate` endpoint at any time to rotate
   credentials.

### 4.5 MUST NOT

A seller **MUST NOT**:

1. Submit `SubmitDelivery` with a `delivery_hash` that does not match the
   uploaded evidence bytes. The oracle will reject; the seller pays gas
   on `SubmitDelivery` regardless.
2. Resubmit `SubmitDelivery` after the oracle has rendered a verdict
   (`resolution_state != 0`). The program returns `InvalidPaymentState`.
3. Modify uploaded evidence bytes after registry upload. The bytes are
   content-addressed by SHA-256; modification produces a new hash and
   leaves the original on-chain reference unchanged.

### 4.6 Delegated authoring

Sellers **MUST** comply with `x402/delegated-authoring/v1`
and publish an intent contract. Domain-specific parameter names belong in
that contract or an informative binding — not in Layer 0–1 core specs.

### 4.7 Rationale

- **Why seller uploads the SLA, not the buyer**: minimizes buyer
  infrastructure. The buyer authors or accepts the SLA bytes; the
  seller (who needs to integrate registry HTTP anyway) carries the
  upload cost. The buyer's correctness comes from the on-chain
  commit, not from controlling the upload.
- **Why delegated authoring requires determinism**: the seller's
  hash-and-upload happens at two different times against the same
  input. Any non-determinism (timestamps in the SLA, map ordering,
  floating-point variance) produces a different `sla_hash` on the
  second compute, breaking the binding the buyer signed. The
  reference seller's sorted-keys canonicalizer is one valid
  approach; any deterministic serialization works.
- **Why `delivery_cutoff_seconds` exists**: prevents the
  last-second-fake-delivery exploit where a seller submits garbage at
  `expires_at - 1s`, gives the oracle no time to reject, and self-releases
  via the expired-with-delivery branch. The program enforces a minimum
  evaluation window.

---

## 5. Oracle obligations

### 5.1 MUST

An oracle **MUST**:

1. Implement exactly one profile from the per-family normatives per
   binary. The `registeredProfiles` field of `/v1/policy` lists that
   single profile. Multi-profile deployments run multiple oracle
   binaries on distinct `oracle_authority` keypairs.
2. Listen for on-chain `DeliverySubmittedEvent` for payments where
   `oracle_authority == self.pubkey`. Oracles MAY use Solana
   `logsSubscribe` or polling to detect the event.
3. On detecting a relevant delivery event:
   - Fetch the SLA bytes by `sla_hash` from the registry
     (`GET /v1/registry/{sha256_hex}`).
   - Fetch the delivery evidence bytes by `delivery_hash` from the
     registry.
   - Re-verify both hashes against the bytes received.
   - Apply the per-family verdict logic.
4. Submit `ConfirmOracle` on-chain; the Payment PDA is identified by
   accounts, not instruction body fields. Instruction body carries
   `delivery_hash`, `resolution_hash`, `resolution_state`, and
   `resolution_reason`. The program rejects if `delivery_hash` does not
   match `payment.delivery_hash`.
5. Compute `resolution_hash` per
   `x402/oracles/resolution-envelope/v1`.
6. Refuse to verdict if `now > payment.expires_at` (the program will
   also reject with `PaymentExpired`, but the oracle SHOULD avoid wasting
   gas on a doomed transaction).

### 5.2 SHOULD

An oracle **SHOULD**:

1. Publish a `/v1/policy` snapshot that accurately reflects the operator's
   current `tipFloorEnabled`, `minVerdictTipDefaultRaw`, and per-mint
   floors. Sellers and buyers cite this for pre-flight policy decisions.
2. Persist the SLA and delivery bytes locally for at least the closure
   delay period. After `ClosePayment`, on-chain references to the hashes
   remain valid as audit anchors only if the registry retains the bytes.
3. Implement guardian timing (`ORACLE_GUARDIAN_*` env) to abort
   long-running profile evaluations and emit a deterministic timeout
   verdict rather than leaving the payment unresolved.

### 5.3 MAY

An oracle **MAY**:

1. Operate as a multi-cluster service by running separate binaries per
   cluster (devnet binary = devnet program ID, mainnet binary = mainnet
   program ID). Cross-cluster oracles are out of scope; an oracle binary
   that mixes clusters is non-conformant.
2. Refuse delivery evaluation if the buyer's `oracle_fee_bps` is below
   the operator's published floor. The verdict in this case is a
   rejection with reason code in the operator economics range (200–219).
3. Charge for evaluations via the on-chain `oracle_fee_bps` tip mechanism;
   the tip is paid on both approval and rejection iff the oracle rendered
   a verdict.

### 5.4 MUST NOT

An oracle **MUST NOT**:

1. Sign `ConfirmOracle` for a `(payment_uid, delivery_hash)` pair where
   the bytes corresponding to `delivery_hash` were not actually evaluated
   against the bytes corresponding to `sla_hash`. Doing so violates the
   trust model and is detectable by any auditor who fetches the same
   bytes from the registry.
2. Re-submit `ConfirmOracle` after a verdict has been rendered. The
   program rejects; the on-chain `resolution_state` is one-shot.
3. Operate a profile not listed in the operator's `/v1/policy`
   `registeredProfiles` field.

### 5.5 Rationale

- **Why oracle binding is permanent**: the program cannot safely rotate
  oracle authority post-funding because the buyer's funding decision was
  made conditional on a specific authority's reputation. Rotation would
  let an attacker swap in a colluding oracle. Buyers needing flexibility
  use shorter TTLs.
- **Why oracle MUST refuse expired payments**: gas economics. The program
  rejects expired `ConfirmOracle` regardless; the SHOULD-level guard saves
  the oracle a doomed transaction fee.
- **Why per-cluster binaries**: `declare_id!` is compile-time. A binary
  built against the mainnet program ID will derive mainnet PDAs and fail
  on a devnet cluster ("Attempt to load a program that does not exist").
  This is enforced by the per-cluster `s5zk…` / `SEsc…` split.

---

## 6. pr402 facilitator obligations

The pr402 facilitator is **OPTIONAL infrastructure**. The protocol does not
require it. Buyers and sellers MAY interact with the on-chain program
directly using SDK builders (`EscrowSdk::*`).

### 6.1 MUST (when operating)

If a deployment chooses to run a pr402 facilitator, that facilitator **MUST**:

1. Verify any `FundPayment` transaction it builds or settles conforms to
   the `paymentRequirements.extra` it advertised, specifically:
   - `escrow_program_id` matches the cluster's deployed program;
   - `bank_address` and `config_address` derive from that program;
   - `oracle_authorities` listed in extra are the only acceptable
     `payment.oracle_authority` values for verify success.
2. Reject `FundPayment` transactions that route to an `escrow_pda` not
   derivable from the advertised `(mint, bank_pda)` pair. The on-chain
   destination check would catch this anyway, but the facilitator's
   pre-submission check saves the buyer a failed transaction.

### 6.2 SHOULD (when operating)

A facilitator **SHOULD**:

1. Expose its supported profiles via the `/supported` endpoint so buyers
   can pre-select a facilitator that supports their intended profile.
2. Persist payment-attempt audit rows for buyer-side troubleshooting.
3. Set `sla_fund_tx_network_fee_payer = "buyer"` for the buyer-paid path
   and `"facilitator"` for the sponsored path. The on-chain program does
   not care; this advertised field exists for buyer cost expectations.

### 6.3 MAY (when operating)

A facilitator **MAY**:

1. Expose a settlement-keeper service that triggers `ReleasePayment`
   post-approval and/or `RefundPayment` post-rejection on payments it
   facilitated. This is convenience automation; the protocol grants any
   signer permission for these post-outcome paths.
2. Sponsor gas for `FundPayment` (the "facilitator-sponsored" path),
   recovering costs through service fees independent of the on-chain
   protocol fee.

### 6.4 MUST NOT (when operating)

A facilitator **MUST NOT**:

1. Hold or solicit the bank authority key. Bank authority is a separate
   role (program admin) and conflating it with facilitator infrastructure
   creates a single point of failure.
2. Hold buyer or seller signing keys. The facilitator builds transactions
   for the buyer to sign or sponsors gas with its own keypair. Custody of
   third-party keys is out of scope.
3. Modify `FundPayment` arguments after the buyer has signed. The buyer's
   signature commits to specific `payment_uid`, `sla_hash`,
   `oracle_authority`, etc.; substitution is forgery.

### 6.5 Rationale

- **Why pr402 is optional**: the protocol's normative interaction is
  buyer ↔ on-chain program ↔ seller ↔ oracle. pr402 is an HTTP convenience
  layer for buyers who prefer not to build Solana transactions directly.
  Treating it as required would violate the protocol's trust-minimized
  premise.
- **Why pr402 MAY but MUST NOT own settlement**: post-outcome triggers
  are permissionless on-chain. A facilitator running a settlement keeper
  is offering a service, not exercising authority. Documenting this as
  MAY rather than MUST keeps the protocol surface honest about what's
  required vs what's offered.

---

## 7. Settlement triggering

The on-chain program v0.4.0 grants permissionless settlement on all
post-outcome paths. The protocol does not designate a single owner. This
section enumerates the canonical actors and the actors that MAY also
trigger.

| Path | Trigger condition | Canonical actor | Other actors that MAY trigger |
|---|---|---|---|
| **Release post-approval** | `resolution_state == 1` AND not expired | seller | buyer, pr402, any third-party keeper |
| **Release expired-delivered** | expired AND delivery submitted AND `resolution_state == 0` (oracle silent) OR approved | seller | buyer, pr402, any third-party keeper |
| **Refund post-rejection** | `resolution_state == 2` (pre-expiry or expired) | buyer | seller, pr402, any third-party keeper |
| **Refund expired-undelivered** | expired AND no delivery AND not approved | buyer | seller, pr402, any third-party keeper |
| **Refund pre-outcome (cancellation)** | `resolution_state == 0` AND cooldown elapsed (buyer) OR by mutual agreement (seller / admin) | buyer (with cooldown) or seller | admin (override) |
| **Close** | terminal state AND `now ≥ closed_at` | any signer | rent always returns to recorded buyer |

### 7.1 Conformance

- Implementations **MUST** treat funds destinations as fixed: release goes
  to `payment.seller`, refund goes to `payment.buyer`, close-rent goes to
  `payment.buyer`. The program enforces these; this spec restates the
  guarantee for clarity.
- Implementations **SHOULD NOT** assume any specific actor is operating
  a settlement keeper. Buyers and sellers building user-facing tooling
  SHOULD provide a manual "settle now" affordance for cases where no
  keeper triggered the path.

### 7.2 Rationale

- **Why permissionless on post-outcome**: outcome is deterministic. The
  on-chain `resolution_state` is one-shot and final; the funds destination
  is recorded at funding time; no policy decision remains. Restricting
  who may pay gas serves no security invariant.
- **Why pre-outcome refund preserves buyer agency**: a third party
  triggering refund pre-outcome could grief an in-flight contract by
  forcibly canceling work the seller is actively performing, even though
  the buyer was patient and the seller hadn't ghosted. Restricting to
  buyer/seller/admin keeps the contract live unless an actor with stake
  in the contract chooses to terminate it.
- **Why close is permissionless with fixed rent destination**: rent is a
  property of the buyer's prior funding, not the closer. Allowing any
  signer to pay gas for cleanup while still returning rent to the buyer
  enables third-party housekeeping (e.g., pr402 close-sweepers) without
  giving them economic gain.

---

## 8. Cross-actor invariants

These invariants are enforced by the on-chain program and listed here for
implementer reference. Violation is a program-level error, not a spec
violation in the actor sense, but each actor SHOULD avoid construction
that would trigger these.

| Invariant | Surface where it manifests |
|---|---|
| `RefundPayment` always credits `payment.buyer` | `RefundPayment` instruction destination check |
| `ReleasePayment` always credits `payment.seller` | `ReleasePayment` instruction destination check |
| `ClosePayment` returns rent to `payment.buyer` | `ClosePayment` instruction destination check |
| `ConfirmOracle` requires prior `SubmitDelivery` | `ConfirmOracle` returns `DeliveryNotSubmitted` if `delivery_timestamp == 0` |
| Oracle verdict is one-shot (`resolution_state ∈ {1,2}` is terminal) | `ConfirmOracle` returns `InvalidPaymentState` on second submission |
| State transitions terminal: `Funded → Released \| Refunded` | `ReleasePayment` / `RefundPayment` assert `state == Funded` |
| `FundPayment` snapshots all timing (cooldown, closure delay, delivery cutoff) into `Payment` | values written at funding time and never re-read from `Config` |
| Oracle authority binding is permanent | the program exposes no oracle-authority rotation instruction |

---

## 9. Versioning

This spec is `x402/sla-escrow-protocol/v1` and corresponds to on-chain
program version `0.4.0` and later compatible versions on the same major
program line.

A future `v2` of this protocol spec will be drafted only in response to:

- A program upgrade that changes actor obligations (e.g., a new instruction
  that requires a new actor responsibility).
- A formal vote to change the trust model (e.g., introducing oracle
  rotation).

`v1` is stable for the program's `0.x` line. Errata MAY be issued as
`v1` revisions; substantive obligation changes require a `v2`.

---

## 10. References

| Reference | Purpose |
|---|---|
| Deployed program at `SEsc…rHprJ` (mainnet) and `s5zk…r4ZH` (devnet) | Authoritative validation logic |
| `x402/serialization-recipes/v1` | Named serializers |
| `x402/delegated-authoring/v1` | HTTP 402 purchase intent |
| `x402/informative/bindings/` | Non-normative product bindings |
| `x402/pr402-discovery/v1` | pr402 wire formats |
| `x402/oracles/resolution-envelope/v1` | `resolution_hash` recipe |
| `x402/oracle-policy-http-api/v1` | `GET /v1/policy` |
| `x402/registry-http-api/v1` | Registry HTTP API |
| `x402/sla-document/v1` | SLA byte commitment |
| Per-family normatives (`Layer 2 profile normatives (see `spec/sla-document/v1/NORMATIVE.md` §5 profile index)`) | Verdict semantics per profile |
| RFC 2119 / RFC 8174 | Keyword interpretation |

---

**Document version:** v1.2
**Tracks program version:** sla-escrow `0.4.0+` (mainnet), `0.2.11+` (devnet)
**Last verified against code:** 2026-05-23
