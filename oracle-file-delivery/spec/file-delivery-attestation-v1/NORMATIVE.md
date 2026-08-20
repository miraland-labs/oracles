# File Delivery Attestation Profile — Version 1 (Normative)

**Profile identifier:** `x402/oracles/file-delivery/attestation/v1`
**Document status:** Normative specification for the `oracle-file-delivery`
reference implementation.
**Scope:** Off-chain SLA documents and *blob-bytes* delivery for content
already committed under SHA-256 to the on-chain `delivery_hash`.

> For the cross-actor flow (buyer / seller / oracle / pr402) that surrounds
> this profile, see [`SLA_ESCROW_PROTOCOL.md`](../../../docs/SLA_ESCROW_PROTOCOL.md).
> This document is normative for the per-profile rules; the protocol doc is
> normative for the wire-level interaction.


---

## Abstract

This profile defines the **attestation-only** trust model for large-file
delivery: the oracle verifies that the registry serves bytes whose SHA-256
matches the on-chain `delivery_hash`, and that those bytes satisfy declared
size and MIME bounds. The oracle does **not** decode, render, or inspect the
file's semantic content.

**Keywords:** content-addressed storage, file delivery, oracle, attestation.

---

## 1. Introduction

For payments where the seller's deliverable is a binary file (e.g., a
generated video, ML model artifact, or rendered image), the on-chain commit
binds `delivery_hash = SHA256(file_bytes)`. The seller posts the file to a
content-addressed registry; the buyer pays; the oracle re-fetches the bytes,
verifies the digest, and attests that the file's size and MIME match the
SLA's declared bounds.

This profile is **normative** for verdicts produced by `oracle-file-delivery`
at profile version `1`.

---

## 2. Terminology

| Term                    | Definition                                                                                                                                                |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **SLA document**        | UTF-8 JSON object describing the agreed file bounds.                                                                                                      |
| **Delivery (the blob)** | Raw bytes the seller uploaded to the registry. The on-chain `delivery_hash` commits to these bytes directly.                                              |
| **Streaming fetch**     | Memory-bounded download performed by `RegistryStreamingFetcher`: 64 KiB read buffer, 512-byte MIME-sniff window, incremental SHA-256.                     |
| **Sniffed MIME**        | The MIME type reported by [`infer`](https://crates.io/crates/infer) over the leading 512 bytes of the body.                                               |

### 2.1 Trust model

`x402/oracles/file-delivery/attestation/v1` proves:

* The bytes the registry serves hash to the on-chain `delivery_hash` (via
  the streaming fetcher's incremental SHA-256, P-HASH-3).
* The byte size lies in `[expected_size_bytes_min, expected_size_bytes_max]`
  (P-FD-1).
* (When set) the sniffed MIME of the leading window matches `expected_mime`
  (P-FD-2).
* (When set) the seller-supplied Ed25519 signature over the blob hash
  verifies under `attestor_pubkey` (P-FD-3).

It does **not** prove:

* That the file is "good" by any human or perceptual definition (no decoding,
  no model inference).
* That the file is unique to this delivery (the same cat video can be hashed
  once and resold; the SLA's `expected_extension` field is documentary only).
* That the file was delivered confidentially. Once `delivery_hash` is on-chain,
  anyone can fetch the bytes from the registry. Use a future
  `x402/file-delivery/handoff/v1` profile if confidentiality matters.

**Recommended for:** bulk-content delivery where the buyer mainly wants to
be sure the file exists, has the right size and format, and is committed
on-chain.

**Migration path:** profiles `semantic/v1` (deep-content checks) and
`handoff/v1` (escrowed handoff with key release) are explicit roadmap items.

---

## 3. Cryptographic binding

* `sla_hash = SHA256(B_sla)` — UTF-8 octets of the SLA JSON.
* `delivery_hash = SHA256(B_blob)` — **raw blob bytes** (NOT a JSON envelope).
  This is the key difference from `x402/oracles/api-quality/v1`, which binds
  `delivery_hash` to a JSON evidence document.

The streaming fetcher computes SHA-256 incrementally during the body read and
fails closed (`OracleError::EvidenceNotFound`) the moment the running digest
is known to mismatch the on-chain hash. There is no path that approves a blob
whose bytes do not hash correctly.

---

## 4. SLA document

### 4.1 Schema

The SLA document MUST validate against
[`schema/sla-document.schema.json`](schema/sla-document.schema.json).

### 4.2 Field semantics

| Field                       | Type                  | Required | Notes                                                                                                                  |
| --------------------------- | --------------------- | -------- | ---------------------------------------------------------------------------------------------------------------------- |
| `version`                   | `u32`                 | yes      | MUST be `1`.                                                                                                           |
| `profile_id`                | `string`              | yes      | MUST be `x402/oracles/file-delivery/attestation/v1`.                                                                            |
| `listing_id`                | `string`              | yes      | Forge listing identity exactly as Forge publishes it (the `{listing_id}` path segment of the oracle verdict door). Identifies the listing being judged; never derived from or replaced by `payment_uid`. |
| `payment_uid`                | 64-char hex `string` | yes      | On-chain `Payment.payment_uid` this SLA is bound to. Carried on the verdict request as `X-Forge-Payment-Uid` alongside `listing_id`, not as a substitute for it.       |
| `expected_size_bytes_min`   | `u64`                 | yes      | Lower bound (inclusive) on raw byte size.                                                                              |
| `expected_size_bytes_max`   | `u64`                 | yes      | Upper bound (inclusive). Defends against a 1-byte file passing as a video.                                             |
| `expected_mime`             | `string` (optional)   | no       | If present, sniffed MIME of the leading 512 bytes must match (case-insensitive prefix match against IANA media types). |
| `expected_extension`        | `string` (optional)   | no       | Audit-only; not enforced.                                                                                              |
| `attestor_pubkey`           | base58 `string`       | no       | If present, evidence MUST include a seller-signed Ed25519 manifest over the blob hash (Property P-FD-3).               |

---

## 5. Delivery evidence

### 5.1 Shape

In v1 there is **no separately uploaded evidence JSON**. The on-chain
`delivery_hash` commits directly to the blob bytes; the oracle's
`FileDeliveryEvidence` is the *outcome* of the streaming fetch (size, sniffed
MIME, verified hash) and is internal to the oracle process.

A future `attestation/v2` may add an attestor manifest (signed envelope
binding `delivery_hash` to a seller-signed metadata blob) carried as a
separate registry entry.

### 5.2 Evidence record (internal)

```rust
pub struct FileDeliveryEvidence {
    pub size_bytes: u64,
    pub sniffed_mime: Option<String>,
    pub blob_sha256_hex: String,  // == on-chain delivery_hash
}
```

---

## 6. Evaluation semantics

Given validated SLA `S` and the streaming-fetch outcome `E`:

1. The streaming fetcher has already verified `SHA256(blob_bytes) ==
   delivery_hash`. Failure surfaces as `OracleError::EvidenceNotFound` at the
   pipeline boundary; the worker writes a `failed` ledger row, does NOT settle
   on-chain, and the buyer's refund cooldown / TTL expiry path takes over
   (Property P-HASH-3).
2. **Size check** (`Custom(320)` `BlobSizeOutOfRange` on failure):
   `E.size_bytes >= S.expected_size_bytes_min && E.size_bytes <= S.expected_size_bytes_max`.
3. **MIME check** (when `S.expected_mime` is set; `Custom(321)`
   `BlobMimeMismatch` on failure): `E.sniffed_mime` equals
   `S.expected_mime` (case-insensitive equality OR prefix match).
4. **Attestor signature check** (when `S.attestor_pubkey` is set;
   `Custom(322)` `BlobAttestorSignatureInvalid` on failure): the seller's
   Ed25519 signature over `blob_sha256_hex` MUST verify under
   `attestor_pubkey`. *In v1 the streaming-evidence path does not yet carry
   signatures, so this check is recorded as failed whenever
   `attestor_pubkey` is set.* Operators who require attestor binding should
   wait for `attestation/v2` or use a domain-specific oracle.

If every applicable check passes, the verdict is **approved** with reason
`0` (`ResolutionReason::None`, P-VER-3).

The first failing check determines the rejection reason (P-VER-2).

---

## 7. Resolution-hash details

The `details` slot of the canonical `x402/oracles/resolution-envelope/v1` envelope:

```json
{
  "blobSha256": "<hex>",
  "sizeBytes": 5242880,
  "sniffedMime": "video/mp4",
  "checks": [
    { "name": "size", "passed": true, "detail": "5242880 bytes (min 1048576, max 10485760)" },
    { "name": "mime", "passed": true, "detail": "sniffed='video/mp4' expected='video/mp4'" }
  ]
}
```

---

## 8. Versioning

* Documentation fixes do not change the profile id.
* Adding deep-content checks → new profile `x402/file-delivery/semantic/v1`.
* Adding ciphertext + key-release handoff → new profile
  `x402/file-delivery/handoff/v1`.

---

## 9. References

* Cross-actor protocol: [`SLA_ESCROW_PROTOCOL.md`](../../../docs/SLA_ESCROW_PROTOCOL.md).
* Implementation:
  [`oracle-file-delivery/src/evaluator.rs`](../../src/evaluator.rs)
  and [`oracle-file-delivery/src/fetcher.rs`](../../src/fetcher.rs).
