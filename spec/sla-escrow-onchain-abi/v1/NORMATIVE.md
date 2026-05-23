# SLA-Escrow On-Chain ABI — Version 1 (Normative)

**Specification identifier:** `x402/sla-escrow-onchain-abi/v1`
**Document status:** Normative wire-level specification of the `sla-escrow`
Solana program's public on-chain interface.
**Tracks deployed program version:** `0.4.x` (mainnet) / `0.2.11+` (devnet).
**Deployed program addresses:**

- Solana mainnet-beta: `SEscZ6n23pVak34xipBKoGCikHUj3w6XPNyty4rHprJ`
- Solana devnet: `s5zkKiy8FD9nFdAhQZoHHV3G8s4QCPzE4cR9U4Hr4ZH`

> For per-actor obligations and authorization rules, see
> `x402/sla-escrow-protocol/v1`.
> For HTTP 402 delegated authoring, see
> `x402/delegated-authoring/v1`.
> For the registry HTTP contract, see
> `x402/registry-http-api/v1`.
> For fee and tip formulas, see §7.3 below.

---

## Abstract

This specification documents the on-chain instruction set, account
layouts, PDA derivation rules, and event formats of the deployed
`sla-escrow` Solana program. It is the contract any client library
(Rust, TypeScript, Python, Go, …) must conform to in order to interact
with the program correctly.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) /
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## 1. Conventions

### 1.1 Endianness

All multi-byte integers are **little-endian** unless noted otherwise.
This applies to instruction arguments, account state fields, and event
payloads.

### 1.2 Encoding

- **Pubkeys** are 32-byte values. Wire encoding is raw bytes; text
  representation is base58.
- **Hashes** (`sla_hash`, `delivery_hash`, `resolution_hash`,
  `payment_uid`) are 32-byte values. Wire encoding is raw bytes; text
  representation is lowercase hexadecimal.
- **Strings**: where any string is hashed (e.g., SLA JSON), it is
  UTF-8.
- **Timestamps** are `i64` Unix epoch seconds.
- **Bps values** (basis points) are `u16`. 10000 bps = 100%.

### 1.3 Cluster identity

The same logical program is deployed at distinct addresses per cluster.
All PDAs are derived against the program's deployed address; PDA values
differ between clusters by construction. Implementations MUST treat the
program address as a runtime parameter, not a compile-time constant.

### 1.4 Account discriminator framing

All program-owned accounts (Bank, Config, Escrow, Payment,
AuthorityTransfer) share a common 8-byte header before the typed body:

```
Byte 0:     u8 discriminator (account kind; see §3)
Bytes 1-7:  reserved (zero-padded; not part of any typed field)
Bytes 8+:   typed account body (Pod-packed fields, in declaration order)
```

Clients reading account data MUST:

1. Verify `data[0]` matches the expected discriminator for the account
   kind.
2. Skip 8 bytes (the header).
3. Deserialize the typed body from `data[8..]`.

Clients writing account data (only relevant for SDK / facilitator
implementations that simulate the program) MUST follow the same layout.

---

## 2. PDA derivation

All PDAs are derived via `find_program_address(seeds, program_id)` per
the Solana standard. Seeds are listed in derivation order.

### 2.1 Seed table

| PDA | Seeds | Derivation parents |
|---|---|---|
| Bank | `b"bank"` | program_id |
| Config | `b"config"` | program_id |
| Escrow (per mint) | `b"escrow"`, mint pubkey, bank PDA | program_id |
| SOL storage (per escrow) | `b"sol_storage"`, mint pubkey, bank PDA, escrow PDA | program_id |
| Payment (per uid) | `b"payment"`, payment_uid (32 bytes), bank PDA | program_id |
| Authority transfer | `b"authority_transfer"`, bank PDA | program_id |

### 2.2 Singleton PDAs

`Bank` and `Config` are singletons per program deployment. They are
seeded by string-literal seeds only (no per-mint or per-payment
discriminator), so each deployed program has exactly one Bank and one
Config.

### 2.3 Per-mint PDAs

`Escrow` is per-mint. The native SOL escrow uses
`mint = Pubkey::default()` (the all-zero pubkey). Each SPL mint has
its own Escrow PDA derived from the same Bank.

### 2.4 SOL storage PDA

The SOL storage PDA is a zero-data, system-program-owned account that
holds native SOL liquidity for SOL escrows. It is derived only when
the escrow's mint is `Pubkey::default()` (native SOL). For SPL escrows,
liquidity lives in an associated token account owned by the escrow
PDA, not in a separate storage PDA.

### 2.5 Payment uid normalization

The Payment PDA seed `payment_uid` is exactly 32 bytes. Clients
producing instructions from human-readable identifiers (e.g., UUID
strings, ULIDs) must normalize to 32 bytes before deriving the PDA.

The reference normalization algorithm (used by the Rust SDK):

1. Remove `-` characters from the input string.
2. Take the first 32 UTF-8 bytes; truncate or zero-pad as needed.

Hex-encoded payment UIDs (64 lowercase hex chars representing 32 bytes)
are decoded directly without normalization. The 32 raw bytes from
`Payment.payment_uid` on-chain are the canonical seed value; clients
that hold those bytes directly SHOULD use them verbatim rather than
re-normalizing through a string round-trip.

### 2.6 Payment PDA collision

`payment_uid` is 32 bytes of buyer-chosen entropy. Uniqueness is the
buyer's responsibility per `sla-escrow-protocol/v1` §3. Two
`FundPayment` calls with the same `payment_uid` against the same Bank
will produce the same Payment PDA; the second call fails with the
Solana program error `Account already in use`.

---

## 3. Account discriminator values

| Account kind | Discriminator (u8) |
|---|---|
| Bank | 100 |
| Config | 101 |
| Escrow | 102 |
| Payment | 103 |
| AuthorityTransfer | 104 |

These are the values appearing at byte 0 of the on-chain account data.

---

## 4. Account state layouts

All layouts use C-repr `Pod` packing: fields appear in declaration
order with native alignment. Typed body starts at byte 8 of the
on-chain account data (after the discriminator + reserved header per
§1.4).

### 4.1 Bank

| Offset (from body start) | Size | Type | Field | Notes |
|---|---|---|---|---|
| 0 | 32 | Pubkey | `authority` | Admin who controls bank-wide config |
| 32 | 8 | i64 | `open_at` | Unix seconds; bank initialization timestamp |
| 40 | 2 | u16 | `fee_bps` | Default protocol fee in basis points |
| 42 | 6 | bytes | _padding | Reserved; readers ignore |

Body size: 48 bytes. On-chain account data size: 8 + 48 = 56 bytes.

### 4.2 Config

| Offset (from body start) | Size | Type | Field | Notes |
|---|---|---|---|---|
| 0 | 8 | i64 | `closure_delay_seconds` | Delay before `ClosePayment` is allowed after terminal state |
| 8 | 8 | i64 | `refund_cooldown_seconds` | Buyer-initiated pre-outcome refund cooldown (0 = disabled, otherwise [3600, 2592000]) |
| 16 | 8 | i64 | `delivery_cutoff_seconds` | Minimum window before `expires_at` for `SubmitDelivery` |
| 24 | 8 | i64 | `updated_at` | Unix seconds; last `UpdateConfig` timestamp |

Body size: 32 bytes. On-chain account data size: 8 + 32 = 40 bytes.

### 4.3 Escrow

| Offset (from body start) | Size | Type | Field | Notes |
|---|---|---|---|---|
| 0 | 32 | Pubkey | `mint` | The mint this escrow services (`Pubkey::default()` for SOL) |
| 32 | 32 | Pubkey | `escrow_tokens` | ATA for SPL; SOL storage PDA for SOL |
| 64 | 8 | i64 | `open_at` | Unix seconds; escrow open timestamp |
| 72 | 8 | u64 | `fee_balance` | Accumulated protocol fees, raw mint units |
| 80 | 8 | u64 | `token_liquidity` | Currently-escrowed payment liquidity |
| 88 | 8 | u64 | `min_payment_amount` | Lower bound on FundPayment.amount |
| 96 | 8 | u64 | `max_payment_amount` | Upper bound on FundPayment.amount |
| 104 | 8 | u64 | `min_fee_amount` | Floor on the bps fee at release |
| 112 | 2 | u16 | `fee_bps` | Per-escrow protocol fee override |
| 114 | 2 | u16 | `oracle_fee_bps` | Per-escrow oracle tip (0 = disabled) |
| 116 | 1 | u8 | `paused` | 0 = active, 1 = paused (FundPayment refused) |
| 117 | 3 | bytes | _padding | Reserved; readers ignore |

Body size: 120 bytes. On-chain account data size: 8 + 120 = 128 bytes.

### 4.4 Payment

| Offset (from body start) | Size | Type | Field | Notes |
|---|---|---|---|---|
| 0 | 32 | bytes | `payment_uid` | Buyer-chosen 32-byte uid; PDA seed material |
| 32 | 32 | Pubkey | `escrow` | The Escrow PDA this payment belongs to |
| 64 | 32 | Pubkey | `buyer` | Authoritative buyer wallet (refund destination) |
| 96 | 32 | Pubkey | `seller` | Authoritative seller wallet (release destination) |
| 128 | 32 | Pubkey | `mint` | Same as `escrow.mint` |
| 160 | 32 | Pubkey | `oracle_authority` | Bound at FundPayment; ConfirmOracle signer must match |
| 192 | 32 | bytes | `sla_hash` | SHA-256 over SLA JSON bytes |
| 224 | 32 | bytes | `delivery_hash` | SHA-256 over delivery evidence bytes (zero until SubmitDelivery) |
| 256 | 32 | bytes | `resolution_hash` | Oracle's attestation digest (zero until ConfirmOracle) |
| 288 | 8 | u64 | `amount` | Escrowed amount in raw mint units |
| 296 | 8 | u64 | `min_fee_amount` | Snapshot from Escrow at FundPayment time |
| 304 | 8 | i64 | `created_at` | Unix seconds; Payment PDA creation |
| 312 | 8 | i64 | `expires_at` | Unix seconds; `created_at + ttl_seconds` |
| 320 | 8 | i64 | `closed_at` | Unix seconds; settled at terminal-state transition + closure_delay |
| 328 | 8 | i64 | `delivery_timestamp` | Unix seconds; SubmitDelivery time (zero until then) |
| 336 | 8 | i64 | `oracle_authority_set_at` | Unix seconds; equals `created_at` at funding |
| 344 | 8 | i64 | `closure_delay_seconds` | Snapshot from Config at FundPayment time |
| 352 | 8 | i64 | `refund_cooldown_seconds` | Snapshot from Config at FundPayment time |
| 360 | 8 | i64 | `delivery_cutoff_seconds` | Snapshot from Config at FundPayment time |
| 368 | 2 | u16 | `resolution_reason` | Oracle-supplied verdict reason code |
| 370 | 2 | u16 | `fee_bps` | Snapshot from Escrow at FundPayment time |
| 372 | 2 | u16 | `oracle_fee_bps` | Snapshot from Escrow at FundPayment time |
| 374 | 1 | u8 | `state` | 0 = Funded, 1 = Released, 2 = Refunded |
| 375 | 1 | u8 | `resolution_state` | 0 = Pending, 1 = Approved, 2 = Rejected |

Body size: 376 bytes. On-chain account data size: 8 + 376 = 384 bytes.

### 4.5 AuthorityTransfer

Transient account that exists only while a `UpdateAuthority` proposal
is pending. Created by `UpdateAuthority`, consumed by `AcceptAuthority`
or `CancelAuthorityProposal`.

| Offset (from body start) | Size | Type | Field | Notes |
|---|---|---|---|---|
| 0 | 32 | Pubkey | `proposed_authority` | Must sign `AcceptAuthority` |
| 32 | 8 | i64 | `proposed_at` | Unix seconds; acceptance gated by program-internal delay |

Body size: 40 bytes. On-chain account data size: 8 + 40 = 48 bytes.

---

## 5. Instructions

Every instruction's data layout is `[discriminator(1) || body(N)]`.
The discriminator is the `u8` value listed in §5.1. The body bytes are
defined per-instruction in §5.2 onward; field offsets in the body table
exclude the discriminator.

Account orderings list each AccountMeta by index, with **W** for
writable and **S** for signer. The program is `program_id` for every
instruction (the deployed sla-escrow program).

### 5.1 Discriminator table

Public buyer/seller/oracle instructions (this spec's scope):

| Discriminator (u8) | Instruction | Caller |
|---|---|---|
| 0 | `FundPayment` | Buyer |
| 1 | `ReleasePayment` | Permissionless post-approval; admin pre-outcome |
| 2 | `RefundPayment` | Buyer/Seller/Admin pre-outcome (with cooldown for buyer); permissionless post-rejection |
| 3 | `ClosePayment` | Permissionless after `closed_at` |
| 4 | `ExtendPaymentTTL` | Buyer |
| 5 | `SubmitDelivery` | Seller (or admin) |
| 6 | `ConfirmOracle` | Oracle (`payment.oracle_authority`) |

Admin instructions occupy discriminators 100–109 and are out of scope
for this spec (see §5.9).

For the full authorization rules per instruction, including
post-v0.4.0 permissionless settlement, see
`x402/sla-escrow-protocol/v1`
§7.

### 5.2 `FundPayment` (discriminator 0)

Buyer locks tokens into escrow. Creates the Payment PDA.

**Body** (176 bytes):

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | Pubkey | `seller` |
| 32 | 32 | Pubkey | `mint` (`Pubkey::default()` for SOL) |
| 64 | 32 | Pubkey | `oracle_authority` |
| 96 | 32 | bytes | `payment_uid` |
| 128 | 32 | bytes | `sla_hash` |
| 160 | 8 | u64 | `amount` (raw mint units) |
| 168 | 8 | i64 | `ttl_seconds` (must be in `[60, 2592000]`) |

**Accounts (SOL path)**, indices 0–7:

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | buyer | W | S | The signer; pays gas + locks funds |
| 1 | bank PDA | | | |
| 2 | config PDA | | | |
| 3 | escrow PDA | W | | |
| 4 | payment PDA (to be created) | W | | |
| 5 | mint (Pubkey::default() account) | | | |
| 6 | sol_storage PDA | W | | |
| 7 | system program | | | |

**Accounts (SPL path)**, indices 0–9:

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | buyer | W | S | |
| 1 | bank PDA | | | |
| 2 | config PDA | | | |
| 3 | escrow PDA | W | | |
| 4 | payment PDA (to be created) | W | | |
| 5 | mint | | | |
| 6 | escrow_tokens (ATA) | W | | |
| 7 | buyer_tokens (ATA) | W | | |
| 8 | token program | | | spl-token or spl-token-2022 |
| 9 | system program | | | |

### 5.3 `ReleasePayment` (discriminator 1)

Releases escrowed tokens to `payment.seller`. Permissionless once
`resolution_state == 1` (approved); admin-override path pre-outcome.
See `sla-escrow-protocol/v1` §7 for the full caller matrix.

**Body**: empty (0 bytes after the discriminator).

**Accounts (SOL path)**, indices 0–8 (+ optional 9):

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | caller | W | S | Pays gas; identity per §7 of protocol spec |
| 1 | bank PDA | | | |
| 2 | config PDA | | | |
| 3 | escrow PDA | W | | |
| 4 | payment PDA | W | | |
| 5 | mint (Pubkey::default() account) | | | |
| 6 | sol_storage PDA | W | | |
| 7 | seller wallet | W | | MUST equal `payment.seller` |
| 8 | system program | | | |
| 9 | oracle_authority | W | | OPTIONAL; required iff `payment.oracle_fee_bps > 0` and `resolution_state != 0` |

**Accounts (SPL path)**, indices 0–11 (+ optional 12-13):

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | caller | W | S | |
| 1 | bank PDA | | | |
| 2 | config PDA | | | |
| 3 | escrow PDA | W | | |
| 4 | payment PDA | W | | |
| 5 | mint | | | |
| 6 | escrow_tokens (ATA) | W | | |
| 7 | seller_tokens (ATA) | W | | Created if absent |
| 8 | seller wallet | W | | MUST equal `payment.seller` |
| 9 | token program | | | |
| 10 | associated_token program | | | For seller_tokens creation |
| 11 | system program | | | |
| 12 | oracle_tokens (ATA) | W | | OPTIONAL; oracle tip path |
| 13 | oracle_authority | W | | OPTIONAL; oracle tip path |

### 5.4 `RefundPayment` (discriminator 2)

Returns escrowed tokens to `payment.buyer`. Caller authorization per
`sla-escrow-protocol/v1` §7.

**Body**: empty.

**Accounts (SOL path)**, indices 0–8 (+ optional 9):

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | caller | W | S | |
| 1 | bank PDA | | | |
| 2 | config PDA | | | |
| 3 | escrow PDA | W | | |
| 4 | payment PDA | W | | |
| 5 | mint (Pubkey::default() account) | | | |
| 6 | sol_storage PDA | W | | |
| 7 | buyer wallet | W | | MUST equal `payment.buyer` |
| 8 | system program | | | |
| 9 | oracle_authority | W | | OPTIONAL; oracle tip on refund |

**Accounts (SPL path)**, indices 0–8 (+ optional 9-12):

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | caller | W | S | |
| 1 | bank PDA | | | |
| 2 | config PDA | | | |
| 3 | escrow PDA | W | | |
| 4 | payment PDA | W | | |
| 5 | mint | | | |
| 6 | escrow_tokens (ATA) | W | | |
| 7 | buyer_tokens (ATA) | W | | Owner MUST be `payment.buyer`; mint MUST equal `payment.mint` |
| 8 | token program | | | |
| 9 | oracle_tokens (ATA) | W | | OPTIONAL; oracle tip path |
| 10 | oracle_authority | W | | OPTIONAL; oracle tip path |
| 11 | associated_token program | | | OPTIONAL; oracle tip path |
| 12 | system program | | | OPTIONAL; oracle tip path |

### 5.5 `ClosePayment` (discriminator 3)

Closes a terminal-state Payment PDA. Permissionless after
`payment.closed_at`. Rent reclaim destination is fixed to
`payment.buyer`.

**Body**: empty.

**Accounts** (7 accounts, indices 0–6):

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | caller | | S | Any signer |
| 1 | buyer wallet | W | | MUST equal `payment.buyer`; rent recipient |
| 2 | bank PDA | | | |
| 3 | config PDA | | | |
| 4 | escrow PDA | W | | |
| 5 | payment PDA | W | | To be closed |
| 6 | system program | | | |

### 5.6 `ExtendPaymentTTL` (discriminator 4)

Buyer extends the payment's `expires_at`. Total TTL from
`created_at` MUST NOT exceed `MAX_TTL_SECONDS` (2592000 = 30 days).
Buyer-only.

**Body** (8 bytes):

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 8 | i64 | `additional_seconds` (must be > 0) |

**Accounts** (7 accounts, indices 0–6):

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | buyer | | S | MUST equal `payment.buyer` |
| 1 | bank PDA | | | |
| 2 | config PDA | | | |
| 3 | escrow PDA | | | |
| 4 | payment PDA | W | | |
| 5 | system program | | | |
| 6 | rent sysvar | | | |

### 5.7 `SubmitDelivery` (discriminator 5)

Seller anchors `delivery_hash` on-chain. Triggers the
`DeliverySubmittedEvent` that oracles consume.

**Body** (32 bytes):

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | bytes | `delivery_hash` |

**Accounts** (5 accounts, indices 0–4):

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | seller | | S | MUST equal `payment.seller` (or bank authority) |
| 1 | bank PDA | | | |
| 2 | config PDA | | | |
| 3 | escrow PDA | | | |
| 4 | payment PDA | W | | |

**Constraints**: `payment.state == Funded`, `payment.resolution_state == 0`,
`payment.expires_at - delivery_cutoff_seconds >= clock.unix_timestamp`.

### 5.8 `ConfirmOracle` (discriminator 6)

Oracle verdict. Sets `payment.resolution_state`, `resolution_reason`,
`resolution_hash`. One-shot: rejected once `resolution_state != 0`.

**Body** (72 bytes):

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | bytes | `delivery_hash` (must equal `payment.delivery_hash`) |
| 32 | 32 | bytes | `resolution_hash` (oracle's attestation digest, opaque to program) |
| 64 | 2 | u16 | `resolution_reason` |
| 66 | 1 | u8 | `resolution_state` (1 = Approved, 2 = Rejected) |
| 67 | 5 | bytes | _padding |

**Accounts** (5 accounts, indices 0–4):

| Idx | Account | W | S | Notes |
|---|---|---|---|---|
| 0 | oracle_authority | | S | MUST equal `payment.oracle_authority` |
| 1 | bank PDA | | | |
| 2 | config PDA | | | |
| 3 | escrow PDA | | | |
| 4 | payment PDA | W | | |

**Constraints**: `payment.state == Funded`, `payment.resolution_state == 0`
(one-shot), `payment.delivery_timestamp != 0`,
`now <= payment.expires_at`.

### 5.9 Admin instructions (out of scope for this spec)

Discriminators 100–109 are admin-only (require `bank.authority`
signature, with `Initialize` further requiring the compile-time
`INITIALIZER_ADDRESS` key). They are invoked by program operators
during deployment and policy maintenance, not by buyers, sellers, or
oracles. Their argument layouts are out of scope for this ABI spec;
operator tooling references the deployed program's source directly.

The standard public instruction surface that buyers, sellers, and
oracles need is fully covered by §5.2–§5.8.

---

## 6. Events

The program emits structured event records via Solana program logs as
`Program data:` lines (the standard mechanism Solana indexers use).
Wire format:

```
Program data: <base64>
```

The base64 decodes directly to the **raw Pod body bytes** of the event
struct — there is no event-kind header byte and no other framing. Each
emitted event is a single `Program data:` line carrying exactly
`size_of::<EventStruct>()` bytes.

Clients dispatch by:

1. The program log lines that surround the `Program data:` line
   (`Program <program_id> invoke [N]`, `Program log: ...`) identify
   which instruction emitted it. A given instruction emits a known
   event type, so context disambiguates.
2. The **byte length** of the decoded payload, when ambiguity remains.
   Each event struct has a distinct size; a base64 payload that
   decodes to `size_of::<DeliverySubmittedEvent>()` bytes (104) is
   unambiguously a `DeliverySubmittedEvent`.

Implementations that parse multiple event types from the same
transaction's logs typically attempt deserialization against expected
event types in turn, gating each attempt on the decoded length.

### 6.1 Events emitted by buyer/seller/oracle paths

These are the events relevant to integrators (buyers, sellers,
oracles, indexers). Admin-emitted events are listed in §6.2.

| Event | Emitted by | Purpose |
|---|---|---|
| `PaymentCreatedWithFundEvent` | `FundPayment` | Buyer funded a new payment |
| `DeliverySubmittedEvent` | `SubmitDelivery` | Oracle's primary trigger |
| `PaymentOracleConfirmedEvent` | `ConfirmOracle` | Oracle verdict landed |
| `PaymentReleasedEvent` | `ReleasePayment` | Funds released to seller |
| `PaymentRefundedEvent` | `RefundPayment` | Funds refunded to buyer |
| `PaymentClosedEvent` | `ClosePayment` | Payment PDA closed |
| `PaymentTTLExtendedEvent` | `ExtendPaymentTTL` | Payment TTL extended |

### 6.2 Admin events (out of scope)

The program also emits events for admin instructions
(`BankInitializedEvent`, `EscrowCreatedEvent`, `EscrowClosedEvent`,
`EscrowPausedEvent`, `EscrowSettingsUpdatedEvent`,
`AuthorityProposedEvent`, `AuthorityProposalCancelledEvent`,
`AuthorityUpdatedEvent`, `FeesWithdrawnEvent`, `ConfigUpdatedEvent`).
These are observable on-chain and consumed by operator tooling; their
layouts are out of scope for this spec.

### 6.3 Pod body layouts

Each event body is a Pod struct with native alignment, fields in
declaration order, little-endian integers. Padding fields exist where
needed to satisfy 8-byte alignment.

#### `DeliverySubmittedEvent`

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | bytes | `payment_uid` |
| 32 | 32 | bytes | `delivery_hash` |
| 64 | 8 | i64 | `timestamp` |
| 72 | 32 | Pubkey | `seller` |

Body size: 104 bytes.

#### `PaymentOracleConfirmedEvent`

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | bytes | `payment_uid` |
| 32 | 32 | Pubkey | `oracle_authority` |
| 64 | 32 | bytes | `delivery_hash` |
| 96 | 32 | bytes | `resolution_hash` |
| 128 | 32 | bytes | `sla_hash` |
| 160 | 8 | u64 | `amount` |
| 168 | 8 | i64 | `timestamp` |
| 176 | 2 | u16 | `resolution_reason` |
| 178 | 1 | u8 | `resolution_state` |
| 179 | 5 | bytes | _padding |

Body size: 184 bytes.

#### `PaymentReleasedEvent`

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | bytes | `payment_uid` |
| 32 | 32 | Pubkey | `mint` |
| 64 | 8 | u64 | `amount` |
| 72 | 8 | u64 | `oracle_tip` |
| 80 | 8 | i64 | `timestamp` |
| 88 | 32 | Pubkey | `seller` |
| 120 | 1 | u8 | `is_expired` (1 = expired-path release, 0 = normal) |
| 121 | 7 | bytes | _padding |

Body size: 128 bytes.

#### `PaymentRefundedEvent`

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | bytes | `payment_uid` |
| 32 | 32 | Pubkey | `mint` |
| 64 | 8 | u64 | `amount` |
| 72 | 8 | u64 | `oracle_tip` |
| 80 | 8 | i64 | `timestamp` |
| 88 | 32 | Pubkey | `buyer` |

Body size: 120 bytes.

#### `PaymentCreatedWithFundEvent`

Emitted by `FundPayment`. Body size: 256 bytes.

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | bytes | `payment_uid` |
| 32 | 32 | bytes | `sla_hash` |
| 64 | 32 | Pubkey | `escrow` |
| 96 | 32 | Pubkey | `buyer` |
| 128 | 32 | Pubkey | `seller` |
| 160 | 32 | Pubkey | `mint` |
| 192 | 32 | Pubkey | `oracle_authority` |
| 224 | 8 | u64 | `amount` |
| 232 | 8 | i64 | `expires_at` |
| 240 | 8 | i64 | `timestamp` |
| 248 | 1 | u8 | `state` |
| 249 | 7 | bytes | _padding |

#### `PaymentClosedEvent`

Emitted by `ClosePayment`. Body size: 72 bytes.

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | bytes | `payment_uid` |
| 32 | 8 | i64 | `timestamp` |
| 40 | 32 | Pubkey | `closer` |

#### `PaymentTTLExtendedEvent`

Emitted by `ExtendPaymentTTL`. Body size: 88 bytes.

| Offset | Size | Type | Field |
|---|---|---|---|
| 0 | 32 | bytes | `payment_uid` |
| 32 | 8 | i64 | `additional_seconds` |
| 40 | 8 | i64 | `new_expires_at` |
| 48 | 8 | i64 | `timestamp` |
| 56 | 32 | Pubkey | `buyer` |

---

## 7. Resolution reason codes

`Payment.resolution_reason` and `ConfirmOracle.resolution_reason` are
`u16` codes. Standard codes are interoperable across all oracles.
Custom codes are partitioned per family.

### 7.1 Standard codes

| Code | Name | Meaning |
|---|---|---|
| 0 | `None` | Approval / no specific rejection reason |
| 1 | `StatusCodeOutOfRange` | HTTP status outside SLA range |
| 2 | `LatencyExceeded` | Response latency exceeded SLA cap |
| 3 | `SchemaValidationFailed` | JSON Schema mismatch |
| 4 | `RequiredFieldsMissing` | Required fields absent |
| 5 | `BodyTooShort` | Response body below minimum length |
| 6 | `HashMismatch` | Evidence hash mismatch |
| 7 | `EvidenceUnavailable` | Off-chain evidence not retrievable |
| 100 | `SLA_UNAVAILABLE` | Active Guardian: SLA bytes not retrievable |
| 101 | `EVIDENCE_UNAVAILABLE` | Active Guardian: Evidence bytes not retrievable |
| 102 | `EVALUATION_TIMEOUT` | Active Guardian: pipeline timeout |
| 200..=219 | Operator economics | Tip-floor refusals (oracle-common implements `200` = `TIP_BELOW_OPERATOR_FLOOR`) |
| 255 | `GeneralRejection` | Catch-all for standard rejections |

### 7.2 Custom code ranges

| Range | Owner |
|---|---|
| 256..=319 | `x402/oracles/onchain-transfer/*` |
| 320..=383 | `x402/oracles/file-delivery/*` |
| 384..=447 | Reserved (`x402/oracles/compute-result/*` future) |
| 448..=511 | Reserved ecosystem-wide |
| 512..=65535 | Per-deployment / new oracle families |

New oracle families requesting an allocated range coordinate with
ecosystem maintainers. Until then, use 512+ and document codes in
the oracle's per-family `NORMATIVE.md`.

For per-family code allocations, see the per-family `NORMATIVE.md`
under `oracles/oracle-*/spec/<profile>/`.

### 7.3 Fee and oracle tip formulas

Protocol fee at release (deducted from escrowed principal):

```text
protocol_fee_raw = min(payment.amount,
                       max(payment.min_fee_amount,
                           floor(payment.amount * payment.fee_bps / 10000)))
```

Oracle tip at release or refund when a verdict was rendered
(`resolution_state ∈ {1, 2}`) and `oracle_fee_bps > 0`:

```text
oracle_tip_raw = floor(payment.amount * payment.oracle_fee_bps / 10000)
```

Oracle tip accounts are required on `ReleasePayment` / `RefundPayment`
when `oracle_fee_bps > 0` and `resolution_state != 0` (see §5.3, §5.4).
Tip floor policy is off-chain (`oracle-policy-http-api/v1`).

---

## 8. State machine

```
                  FundPayment
                       │
                       ▼
              ┌─────────────────┐
              │ state = Funded  │◀──────────────┐
              │ resolution = 0  │               │
              └────────┬────────┘               │
                       │                        │
        ┌──────────────┼───────────────┐        │
        │              │               │        │
        ▼              ▼               ▼        │
  SubmitDelivery   RefundPayment   (expiry)     │
        │     (pre-outcome)              │      │
        ▼                                │      │
   delivery_hash                         │      │
   set; event                            │      │
   emitted                               │      │
        │                                │      │
        ▼                                │      │
   ConfirmOracle                         │      │
        │                                │      │
        ▼                                │      │
   resolution_state ∈ {1, 2}             │      │
        │                                │      │
   ┌────┴────┐                           │      │
   ▼         ▼                           │      │
 = 1      = 2                            │      │
ReleasePay  RefundPay                    │      │
   │         │                           │      │
   ▼         ▼                           │      │
state =    state =                       │      │
Released   Refunded                      │      │
                                         │      │
                                         ▼      │
                            (post-expiry settlement: refund |
                             release per §7 of protocol spec) ─┘

Terminal states (Released | Refunded): wait closure_delay_seconds → ClosePayment
```

`state` and `resolution_state` are independent fields. `resolution_state`
records the oracle's verdict; `state` records the funds movement.
Final state is reached at `ReleasePayment` or `RefundPayment`.

---

## 9. Versioning

This spec is `x402/sla-escrow-onchain-abi/v1`. Tracks the program's
`0.4.x` mainnet line and `0.2.x` devnet line, which share an identical
on-chain ABI. Future spec versions are issued only when:

- A program upgrade changes instruction discriminators, account layouts,
  or PDA seeds (none of which has been done since v0.4.0; v0.4.0 itself
  was a logic-only change with no ABI impact).
- A new instruction is added to the public buyer/seller/oracle surface.
- An existing instruction adds new required arguments.

Logic-only program upgrades that don't alter the ABI (e.g., a future
v0.4.1 changing internal validation) do NOT require a spec bump.

---

## 10. References

| Reference | Purpose |
|---|---|
| Deployed program at `SEsc…rHprJ` (mainnet) and `s5zk…r4ZH` (devnet) | Authoritative bytes |
| `x402/sla-escrow-protocol/v1` | Per-actor obligations and authorization rules |
| `x402/registry-http-api/v1` | HTTP contract for the oracle registry |
| `x402/sla-document/v1` | Cross-family SLA envelope |
| `x402/oracles/resolution-envelope/v1` | Resolution hash |
| `x402/oracle-policy-http-api/v1` | Tip floors |
| `x402/serialization-recipes/v1` | Recipe registry |
| `x402/delegated-authoring/v1` | HTTP 402 intent |
| `x402/pr402-discovery/v1` | pr402 wire formats |
| RFC 2119 / RFC 8174 | Keyword interpretation |

---

**Document version:** v1.1
**Last verified against deployed binary:** 2026-05-23
