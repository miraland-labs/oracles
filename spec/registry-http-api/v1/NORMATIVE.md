# Registry HTTP API — Version 1 (Normative)

**Specification identifier:** `x402/registry-http-api/v1`
**Document status:** Normative wire-level specification for the
content-addressed registry HTTP API exposed by oracle binaries in the
x402 ecosystem.
**Mount point:** routes are mounted under `/v1/registry/...` on the
oracle's HTTP server.

> For the cross-actor flow that uses this API, see `x402/sla-escrow-protocol/v1`.
> For SLA byte commitment, see `x402/sla-document/v1`.

---

## Abstract

This document specifies the wire-level HTTP contract for the registry
service that hosts SLA documents, delivery evidence, and supplementary
blobs in an SLA-escrow payment lifecycle. Conformance enables
interoperable seller HTTP clients, buyer HEAD-check clients, and oracle
fetch logic across independent implementations.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) /
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## 1. Introduction

The registry serves three concerns:

1. **Content-addressed storage** of SLA documents and delivery evidence,
   so on-chain `sla_hash` and `delivery_hash` resolve to byte-identical
   payloads any party can fetch.
2. **Seller authentication** via wallet-signed challenge → bearer token,
   gating uploads to known sellers without requiring per-payment
   authorization tokens.
3. **Operator metadata** discovery (storage backend, size limits,
   registered profile, oracle authority pubkey, cluster).

All endpoints share content-addressed identity: every uploaded payload
is keyed by SHA-256 of its bytes. The server recomputes the digest
during upload (cap-enforced) and on every read; clients SHOULD also
re-verify on receipt.

---

## 2. Terminology

| Term | Definition |
|---|---|
| **`sha256_hex`** | 64-character lowercase hexadecimal SHA-256 digest of a payload's bytes. |
| **Bearer token** | Opaque token returned exactly once by `POST /seller/register` or `POST /seller/rotate`. The server stores only `SHA-256(token)`. |
| **Challenge** | A short-lived random string the registry issues; the seller signs it with their wallet keypair to prove ownership of the wallet pubkey. |
| **Profile ID** | A versioned identifier of a verdict family (e.g., `x402/oracles/onchain-transfer/v1`). |
| **Catalog** | The registry's bookkeeping table (`oracle_deliveries`) recording `(sha256_hex, kind, size_bytes, content_type, profile_id, ...)` for every uploaded payload. |
| **Backend** | The underlying storage implementation: `postgres` (BYTEA), `s3`, or `local` (filesystem). Selected per oracle deployment. |

### 2.1 Content type

- **JSON kinds** (SLA, delivery): UTF-8 encoded JSON. The server **MUST**
  parse the body as JSON. For `sla`, the body **MUST** also contain a
  non-empty top-level `profile_id` field. For `delivery`, `profile_id`
  is OPTIONAL but the body **MUST** still parse as JSON; non-JSON
  delivery payloads belong on `/blob`.
- **Blob kind**: arbitrary binary content. The seller MAY set a
  `Content-Type` request header; the server stores it verbatim.

### 2.2 Cryptographic primitives

- **Hashing**: SHA-256 throughout (catalog keys, content addressing,
  bearer-token storage digest).
- **Signing**: Ed25519 over Solana's standard pubkey/signature scheme
  (32-byte pubkey, 64-byte signature, base58 encoding).
- **Tokens**: 32 bytes of cryptographic randomness, base58-encoded.

---

## 3. Endpoints overview

| Method | Path | Auth | Purpose |
|---|---|---|---|
| GET | `/v1/registry/info` | none | Operator metadata |
| GET | `/v1/registry/seller/challenge` | none | Issue signing challenge |
| POST | `/v1/registry/seller/register` | signature | Register wallet → bearer token |
| POST | `/v1/registry/seller/rotate` | bearer | Revoke current token, issue new |
| POST | `/v1/registry/sla` | bearer | Upload SLA document (JSON) |
| POST | `/v1/registry/delivery` | bearer | Upload delivery evidence (JSON) |
| POST | `/v1/registry/blob` | bearer | Upload arbitrary blob |
| GET | `/v1/registry/{sha256_hex}` | none | Fetch payload by content hash |
| HEAD | `/v1/registry/{sha256_hex}` | none | Existence check by content hash |

Auth notes:

- **none**: no authorization header required.
- **signature**: body carries an Ed25519 signature over the challenge.
- **bearer**: `Authorization: Bearer <token>` header required.

---

## 4. Common response framing

### 4.1 Success

JSON responses use the shape documented per endpoint. `Content-Type`
of JSON responses is `application/json`. Binary fetches return the raw
bytes with `Content-Type` set to either the value supplied at upload
time or `application/octet-stream`.

### 4.2 Errors

Application-level errors use the JSON shape:

```json
{ "error": "<human-readable message>" }
```

Status codes are standard HTTP. Each endpoint section below enumerates
the expected error codes.

**Framework-level errors:** extreme requests (e.g., bodies far larger
than the configured cap) MAY be rejected by the HTTP framework before
reaching the application. The response body shape for framework-level
errors is implementation-defined and **MAY** differ from the JSON shape
above. Clients **SHOULD** treat any `4xx` or `5xx` as a failure
without relying on a specific body shape.

### 4.3 Idempotency

All POST upload endpoints (`/sla`, `/delivery`, `/blob`) are idempotent
on `(sha256, kind)`. A duplicate upload of identical bytes returns
`200 OK` with the same response shape; the catalog row is upserted.
Clients MAY rely on this for retry safety.

---

## 5. `GET /v1/registry/info`

Public operator metadata. Used by buyers and indexers for capability
discovery.

### 5.1 Request

No headers, no body, no query parameters.

### 5.2 Response

`200 OK`:

```json
{
  "backend": "postgres" | "s3" | "local",
  "max_bytea_bytes": 4194304,
  "max_blob_bytes": 5368709120,
  "registered_profile_id": "x402/oracles/onchain-transfer/v1",
  "oracle_pubkey": "<base58-pubkey>",
  "normative_spec_url": "https://...",   // optional
  "cluster": "devnet"                     // optional
}
```

| Field | Type | Notes |
|---|---|---|
| `backend` | string | One of `postgres` \| `s3` \| `local`. |
| `max_bytea_bytes` | integer | Max body size for `/sla` and `/delivery`. |
| `max_blob_bytes` | integer | Max body size for `/blob`. |
| `registered_profile_id` | string | The profile this binary serves. |
| `oracle_pubkey` | string | Base58 oracle authority. Buyers cite this in `FundPayment.oracle_authority`. |
| `normative_spec_url` | string \| omitted | Link to the per-family `NORMATIVE.md`. Omitted if operator does not advertise. |
| `cluster` | string \| omitted | Solana cluster (`mainnet-beta` \| `devnet` \| `testnet`). Omitted for cluster-agnostic profiles. |

### 5.3 Conformance

- The server **MUST** populate `oracle_pubkey` with the same Solana
  pubkey it uses to sign `ConfirmOracle`.
- The server **MUST** publish exactly one `registered_profile_id` per
  binary. Multi-profile binaries are out of scope for this spec.
- The server **MUST** populate `cluster` for cluster-pinned profiles.

---

## 6. `GET /v1/registry/seller/challenge`

Issue a fresh challenge for a wallet to sign.

### 6.1 Request

```
GET /v1/registry/seller/challenge?wallet=<base58-pubkey>
```

### 6.2 Response

`200 OK`:

```json
{
  "challenge": "<base58-32-bytes-random>",
  "expires_at": "2026-05-22T13:45:00Z"
}
```

| Field | Type | Notes |
|---|---|---|
| `challenge` | string | Random 32-byte value, base58-encoded. |
| `expires_at` | RFC 3339 timestamp | Server-side TTL deadline (default 5 minutes). |

### 6.3 Errors

| Status | Body | Cause |
|---|---|---|
| `400 Bad Request` | `{"error": "wallet query parameter required"}` | Empty or missing `wallet` query parameter. |

### 6.4 Conformance

- Challenges **MUST** be 32 bytes of cryptographic randomness.
- Challenges **MUST** expire server-side after a documented TTL
  (RECOMMENDED: 5 minutes).
- A challenge **MUST** be valid for exactly one `(wallet, challenge)`
  pair. Servers **MUST** reject use of a challenge by a wallet other
  than the one that requested it.

### 6.5 Rationale

Challenge-then-sign establishes wallet ownership without trusting the
client to generate randomness. The 5-minute TTL bounds the window for
intercept attacks; sellers are expected to register interactively.

---

## 7. `POST /v1/registry/seller/register`

Register a wallet by submitting an Ed25519 signature over the challenge.
On success, the registry returns a fresh bearer token exactly once.

### 7.1 Request

```
POST /v1/registry/seller/register
Content-Type: application/json
```

Body:

```json
{
  "wallet": "<base58-pubkey>",
  "signature": "<base58-signature>",
  "challenge": "<base58-challenge>"
}
```

| Field | Type | Notes |
|---|---|---|
| `wallet` | string | 32-byte Solana pubkey, base58. |
| `signature` | string | 64-byte Ed25519 signature over the **raw UTF-8 bytes** of `challenge`, base58. **Not** SIMD-0048 envelope. |
| `challenge` | string | The exact challenge string returned by `/seller/challenge` for this wallet. |

### 7.2 Response

`200 OK`:

```json
{
  "id": 42,
  "token": "<base58-32-bytes-token>"
}
```

| Field | Type | Notes |
|---|---|---|
| `id` | integer | Database row id. Returned for operator audit; clients SHOULD NOT depend on its exact value. |
| `token` | string | Bearer token, base58-encoded 32 bytes of randomness. **Returned exactly once.** Server stores only `SHA-256(token)`. |

### 7.3 Errors

| Status | Body | Cause |
|---|---|---|
| `400 Bad Request` | `{"error": "challenge expired or unknown for this wallet"}` | Challenge was never issued, expired, or already consumed. |
| `400 Bad Request` | `{"error": "signature invalid: <detail>"}` | Signature verification failed. Detail may include `invalid wallet`, `invalid signature`. |
| `500 Internal Server Error` | `{"error": "auth: <detail>"}` | Database insertion failure. |

### 7.4 Conformance

- The server **MUST** verify the signature against the **raw UTF-8
  bytes** of the challenge string, not over a wrapper envelope.
  Implementations using Solana CLI's `sign-offchain-message` (which wraps
  with the SIMD-0048 envelope) are non-conformant for this endpoint.
- The server **MUST** consume the challenge on success: a second
  `register` call with the same `(wallet, challenge)` pair **MUST**
  return `400 Bad Request`.
- The server **MUST NOT** store the raw bearer token. Only `SHA-256`
  digest is persisted.
- The bearer token **MUST** be at least 256 bits of cryptographic
  randomness, base58-encoded.

### 7.5 Rationale

- **Why raw bytes, not SIMD-0048 envelope**: the server's verifier is
  pure Ed25519 over the challenge string. Forcing the SIMD-0048 envelope
  would require server-side parsing of an opaque wrapper format with no
  added security; the challenge is already random and bounded.
  Implementations relying on `solana sign-offchain-message` will fail
  here. Use a direct `nacl.sign(challenge_bytes)` flow.
- **Why one-shot tokens**: enables credential rotation via
  `/seller/rotate` without coordination. The server cannot retrieve a
  lost token.

---

## 8. `POST /v1/registry/seller/rotate`

Revoke the bearer token presented in the request and issue a new token
for the same wallet.

### 8.1 Request

```
POST /v1/registry/seller/rotate
Authorization: Bearer <current-token>
```

No body.

### 8.2 Response

`200 OK`:

```json
{
  "id": 43,
  "token": "<base58-new-token>"
}
```

The old token is revoked; the new token is associated with the same
wallet pubkey as the old.

### 8.3 Errors

| Status | Body | Cause |
|---|---|---|
| `401 Unauthorized` | `{"error": "missing or malformed bearer token"}` | No `Authorization: Bearer ...` header. |
| `401 Unauthorized` | `{"error": "bearer token revoked"}` | Token was previously rotated or revoked. |
| `401 Unauthorized` | `{"error": "bearer token not recognized"}` | Token does not match any seller key digest. |
| `404 Not Found` | `{"error": "seller key id not found"}` | Wallet binding lookup failed (server inconsistency; SHOULD NOT occur in normal operation). |
| `500 Internal Server Error` | `{"error": "auth: <detail>"}` | Database error. |

### 8.4 Conformance

- The server **MUST** revoke the old token before issuing the new one.
  Even if rotation fails partway through, the old token **MUST NOT**
  remain valid alongside a new one.
- The new token **MUST** be a fresh 32-byte random value.

---

## 9. `POST /v1/registry/sla`

Upload an SLA document. The body **MUST** be valid JSON containing a
non-empty `profile_id` field.

### 9.1 Request

```
POST /v1/registry/sla
Authorization: Bearer <token>
Content-Type: application/json
```

Body: UTF-8 JSON, `≤ max_bytea_bytes` bytes (per `/info` response).
The JSON object **MUST** include:

```json
{
  "profile_id": "x402/oracles/<family>/v<n>",
  ...
}
```

The server parses only `profile_id` and stores the bytes content-addressed.
The full SLA shape is defined per profile in
`<profile>/NORMATIVE.md`; this endpoint does not validate SLA
semantics.

### 9.2 Response

`200 OK`:

```json
{
  "sha256": "<64-hex>",
  "url": "/v1/registry/<64-hex>",
  "size_bytes": 512,
  "kind": "sla",
  "stored_at": "2026-05-22T13:45:00Z",
  "content_type": "application/json"
}
```

| Field | Type | Notes |
|---|---|---|
| `sha256` | string | Lowercase hex SHA-256 of the uploaded bytes. |
| `url` | string | Path to fetch (relative). Clients construct the full URL by prefixing the registry base. |
| `size_bytes` | integer | Verified body length. |
| `kind` | string | Always `"sla"` for this endpoint. |
| `stored_at` | RFC 3339 timestamp | Server-side timestamp at catalog upsert. |
| `content_type` | string \| omitted | Echoed if the request set `Content-Type`; otherwise omitted. |

### 9.3 Errors

| Status | Body | Cause |
|---|---|---|
| `401 Unauthorized` | (auth errors per §8.3) | Missing/invalid bearer. |
| `400 Bad Request` | `{"error": "body is not valid JSON: <detail>"}` | JSON parse failure. |
| `400 Bad Request` | `{"error": "SLA JSON must contain a non-empty 'profile_id' field"}` | `profile_id` missing or empty. |
| `413 Payload Too Large` | `{"error": "body exceeds max_bytea_bytes (<n>)"}` | Body larger than configured cap. |
| `500 Internal Server Error` | (storage / db errors) | Backend failure. |
| `504 Gateway Timeout` | `{"error": "db timeout"}` | Catalog upsert exceeded server-side timeout. |

### 9.4 Conformance

- The server **MUST** compute SHA-256 over the body and return the
  computed digest in `sha256`. Clients **MUST** re-verify by hashing
  the bytes they sent.
- The server **MUST** reject bodies exceeding `max_bytea_bytes` from
  `/info` with `413`.
- The server **MUST** parse the body as JSON. Non-JSON `/sla` uploads
  are non-conformant.
- A duplicate upload of identical bytes **MUST** succeed with `200 OK`
  and the same `sha256`.

---

## 10. `POST /v1/registry/delivery`

Upload delivery evidence. Same content-type, size, and idempotency
rules as `/sla`, with one difference: `profile_id` is OPTIONAL in
the JSON body (some profiles, e.g. `file-delivery`, do not carry it
on the delivery side).

The body **MUST** be valid JSON. Non-JSON delivery payloads belong on
`/blob` and are rejected by this endpoint.

### 10.1 Errors

Identical to §9.3 except the `profile_id` presence check is not
performed. Invalid JSON returns `400 Bad Request` with the same
shape (`{"error": "body is not valid JSON: <detail>"}`).

### 10.2 Conformance

- The server **MUST** parse the body as JSON. Non-JSON bodies are
  rejected with `400 Bad Request`.
- The server **MUST** allow JSON bodies without `profile_id`.
- The server **MUST** sniff `profile_id` for catalog tagging when the
  field is present; absent or empty `profile_id` results in a NULL
  catalog row entry.
- The server **MUST** keep delivery uploads in the same content-addressed
  storage namespace as SLA uploads (no separate hash space).

---

## 11. `POST /v1/registry/blob`

Upload an arbitrary binary blob (e.g., signed delivery proofs,
oracle-side audit dumps).

### 11.1 Request

```
POST /v1/registry/blob
Authorization: Bearer <token>
Content-Type: <any>          // optional; echoed in response and stored
```

Body: arbitrary bytes, `≤ max_blob_bytes` (much larger cap than
`/sla` / `/delivery` — typically 5 GiB).

### 11.2 Response

Same shape as `/sla` (§9.2) with `kind: "blob"`. The `content_type`
field echoes the request `Content-Type` when present.

### 11.3 Errors

Identical to §9.3 except:

- No JSON parsing, so `400 Bad Request` for `body is not valid JSON`
  does not occur.
- The size cap is `max_blob_bytes` (per `/info`).

### 11.4 Conformance

- The server **MUST** store the request `Content-Type` verbatim in the
  catalog if provided, and echo it in the response.
- The server **MUST NOT** content-sniff the blob to derive a content
  type; the seller's declaration is authoritative.

---

## 12. `GET /v1/registry/{sha256_hex}`

Fetch a payload by content hash.

### 12.1 Request

```
GET /v1/registry/<64-hex>
```

Path parameter: `sha256_hex` MUST be 64 lowercase hexadecimal
characters. No auth.

### 12.2 Response

`200 OK`: raw bytes. `Content-Type` is the value stored in the catalog
(set at upload time), or `application/octet-stream` if absent.

### 12.3 Errors

| Status | Body | Cause |
|---|---|---|
| `400 Bad Request` | `{"error": "path must be 64 lowercase hex chars"}` | Path parameter malformed. |
| `404 Not Found` | `{"error": "not found"}` | Hash not in storage. |
| `500 Internal Server Error` | `{"error": "stored bytes do not hash to requested digest"}` | Server-side integrity violation; the registry detected corruption between storage and serving. Clients **SHOULD** treat this as a permanent failure for that hash. |
| `500 Internal Server Error` | `{"error": "storage: <detail>"}` | Backend I/O error. |

### 12.4 Conformance

- The server **MUST** re-verify SHA-256 over the bytes before serving.
  This protects against silent storage corruption.
- The server **MUST** serve identical bytes for repeated requests of
  the same hash.

### 12.5 Client recommendation

Clients **SHOULD** re-verify the hash on receipt. The server's
re-verification is a defense in depth; the canonical correctness check
is on the consumer side, since servers can be misconfigured or
compromised.

---

## 13. `HEAD /v1/registry/{sha256_hex}`

Existence check. Same path parameter rules as `GET`.

### 13.1 Request

```
HEAD /v1/registry/<64-hex>
```

No auth.

### 13.2 Response

`200 OK` if the payload exists. Headers:

- `Content-Length`: payload size in bytes.
- `Content-Type`: stored content type (omitted if not stored).

No response body.

`404 Not Found` if the payload is absent. No response body.

### 13.3 Errors

| Status | Cause |
|---|---|
| `400 Bad Request` | Path parameter malformed (response body is JSON `{"error": ...}`). |
| `500 Internal Server Error` | Backend I/O error. |

### 13.4 Conformance

- The server **MUST NOT** require auth for `HEAD`. This enables the
  buyer's pre-funding liveness check (see
  `sla-escrow-protocol/v1/NORMATIVE.md` §3.2).
- The server **MUST** consult the storage backend's metadata, not just
  the catalog. A catalog entry without backing bytes (e.g., S3 object
  expired) **MUST** return `404`.

---

## 14. Cross-cutting requirements

### 14.1 TLS

Production deployments **MUST** terminate TLS in front of the registry
(e.g., an Nginx reverse proxy). The registry binary itself MAY listen
on plaintext HTTP behind the TLS terminator; cross-cluster path-based
TLS deployment is documented in the operator's deployment guide.

### 14.2 Rate limiting and DOS

This spec does NOT mandate rate limits; operators MAY apply them at the
TLS terminator or at the application layer. Compliant clients **SHOULD**
back off on `429 Too Many Requests` if returned.

### 14.3 CORS

The registry endpoints serve in-process oracle traffic plus seller HTTP
clients. CORS policy is operator-determined and not specified here.

### 14.4 Versioning

This spec is `x402/registry-http-api/v1`. Future incompatible changes
**MUST** mount under `/v2/registry/...`. Substantive backward-compatible
additions (new optional response fields, new endpoints) MAY be added
within `v1` and documented as errata.

### 14.5 Encoding

- All textual identifiers (`wallet`, `signature`, `challenge`,
  `bearer token`) are base58.
- All hashes (`sha256_hex`, `bearer_sha256` storage digest) are
  lowercase hexadecimal.
- All timestamps are RFC 3339 UTC.

---

## 15. Conformance summary

A conformant **server** implementation:

- Mounts all routes in §3 under `/v1/registry/...` with documented
  status codes and response shapes.
- Validates Ed25519 signatures over raw challenge bytes.
- Stores only `SHA-256(token)` for bearer tokens.
- Recomputes SHA-256 on uploads and re-verifies on reads.
- Provides idempotent upload behavior on `(sha256, kind)`.

A conformant **seller client** implementation:

- Performs the challenge → register → bearer flow once per wallet,
  caches the token securely, rotates on schedule.
- Sends bearer token on `Authorization: Bearer ...` for all upload and
  rotate endpoints.
- Treats upload responses as content-addressed: re-verifies returned
  `sha256` matches local hash.

A conformant **buyer client** implementation:

- MAY perform `HEAD /v1/registry/{sla_hash_hex}` before signing
  `FundPayment` to confirm the seller-relayed upload actually exists.
- MUST NOT depend on bearer auth for read paths.

A conformant **oracle implementation** (consuming the registry):

- Fetches by `sha256_hex` only.
- Re-verifies SHA-256 on receipt before parsing.
- Treats `404` as evidence-unavailable; does not retry indefinitely.

---

## 16. References

| Reference | Purpose |
|---|---|
| `oracle-common/src/registry/api.rs` | Reference implementation (this repo) |
| `oracle-common/src/registry/auth.rs` | Reference auth implementation (this repo) |
| `oracle-common/src/registry/storage.rs` | Reference storage backends (this repo) |
| `x402/oracle-policy-http-api/v1` | `GET /v1/policy` (tip floors) |
| `x402/sla-escrow-protocol/v1` | Cross-actor protocol |
| `x402/sla-document/v1` | SLA byte commitment |
| RFC 2119 / RFC 8174 | Keyword interpretation |
| RFC 3339 | Timestamp format |
| Ed25519 (RFC 8032) | Signature scheme |
| SHA-256 (NIST FIPS 180-4) | Hash function |

---

**Document version:** v1.1
**Last verified against reference implementation:** 2026-05-23
