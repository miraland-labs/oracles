# oracle-file-delivery

Streaming file delivery quality oracle for the x402/pr402 ecosystem.
Implements the [`x402/oracle/file-delivery/attestation/v1`](spec/file-delivery-attestation-v1/NORMATIVE.md)
profile: streams the seller's blob from the registry, computes SHA-256
incrementally, sniffs MIME from the leading 512 bytes, and approves when
size, MIME, and (optional) attestor-pubkey constraints all hold.

> **Are you a seller integrating with this oracle?** This README is the
> operator-facing doc (run, deploy, observe). Sellers should read
> [`oracles/docs/SELLER_GUIDE.md`](../docs/SELLER_GUIDE.md) — see §4.C
> for the file-delivery recipe.
>
> **Are you a buyer paying for an SLA-escrow service?** Read
> [`oracles/docs/BUYER_GUIDE.md`](../docs/BUYER_GUIDE.md) — how to pick an
> oracle and fund the escrow via pr402.

This is the largest of the three sibling oracles in terms of evidence size
(default cap 5 GiB). It is purpose-built for video / large-binary delivery
flows where the seller's deliverable is the file itself, not a JSON.

## Quick start (development)

The recommended dev / staging blob backend is MinIO. A one-line bootstrap
is provided:

```bash
sudo bash oracles/scripts/bootstrap-minio.sh
# prints ORACLE_REGISTRY_S3_ACCESS_KEY / SECRET_KEY on completion
```

Then:

```bash
cd oracles
cp oracle-file-delivery/.env.example /tmp/oracle-file-delivery.env
# edit /tmp/oracle-file-delivery.env: ORACLE_REGISTRY_BACKEND=s3 plus the
# ORACLE_REGISTRY_S3_* values printed by bootstrap-minio.sh.
psql "$DATABASE_URL" < oracle-common/migrations/init.sql
env $(grep -v '^#' /tmp/oracle-file-delivery.env | xargs) \
    cargo run --release -p oracle-file-delivery
```

Default port: `:4022`. HTTP surface is identical to the api-quality binary
(see its README); blob uploads via `POST /v1/registry/blob` are the
high-volume path.

## Production install (Ubuntu 24.04)

```bash
sudo bash oracles/scripts/install.sh \
    file-delivery \
    https://github.com/miraland-labs/oracles/releases/download/oracle-file-delivery-vX/oracle-file-delivery \
    /tmp/oracle-file-delivery.env
sudo systemctl status oracle@file-delivery
```

## Devnet runbook

[`tests/devnet/file_v1.sh`](tests/devnet/file_v1.sh) drives the full path:
seller uploads a 5–10 MiB MP4, registry returns SHA-256 + size, buyer funds
the escrow, seller submits delivery, oracle streams the blob, computes
incremental SHA-256, sniffs MIME, settles.

### Prerequisites

- `solana-keygen` / `solana` CLI 2.x; an oracle keypair funded with Devnet SOL
- `jq`, `curl`, `shasum`, `stat`
- `DATABASE_URL` Postgres with migrations applied
- A MinIO bucket reachable from the oracle host (or any S3-compatible
  endpoint — Wasabi, B2, R2, AWS S3 all work via `ORACLE_REGISTRY_S3_*`).
- The `oracle-file-delivery` binary running (default port `:4022`)
- A 5–10 MiB MP4 fixture. Drop it at `./test-fixtures/example.mp4` or set
  `BLOB_FILE` to its path.
- Exported: `ORACLE_HOST`, `SELLER_TOKEN`, `PAYMENT_UID`

### Operational notes

- Running two binaries with the same oracle keypair is unsupported (race).
- The streaming fetcher uses a fixed 64 KiB read buffer and a 512-byte
  MIME-sniff window. Both are constants in
  `oracle-common/src/fetcher.rs::RegistryStreamingFetcher` and intentionally
  small to keep memory footprint bounded across concurrent jobs.
- A hash mismatch on the streamed body is fail-closed and surfaces as
  `Custom(320)` (`SizeOutOfRange` is `321` etc — see the table below).
- Running with `ORACLE_REGISTRY_BACKEND=postgres` is supported but limited
  to `ORACLE_REGISTRY_MAX_BYTEA_BYTES` (default 4 MiB). For real-world
  video / large-binary delivery use S3 (MinIO).
- Capacity planning: with the default 5 GiB blob cap, allocate `min(5GiB,
  expected_blob_size) × concurrent_jobs` of network + disk headroom on the
  MinIO host. The oracle itself only buffers 64 KiB at a time.

## Specification

- [`spec/file-delivery-attestation-v1/NORMATIVE.md`](spec/file-delivery-attestation-v1/NORMATIVE.md)
- [`spec/file-delivery-attestation-v1/schema/sla-document.schema.json`](spec/file-delivery-attestation-v1/schema/sla-document.schema.json)
- [`spec/file-delivery-attestation-v1/examples/`](spec/file-delivery-attestation-v1/examples/) —
  approve 5MB MP4, reject undersized, reject MIME-mismatch.

## Resolution-reason codes

This binary emits codes in `[320, 322]` and `0` / `255`:

| Code | Meaning                       |
| ---- | ----------------------------- |
| 0    | Approved                      |
| 320  | HashMismatch (streamed body)  |
| 321  | SizeOutOfRange                |
| 322  | MimeMismatch                  |
| 255  | Unspecified / generic failure |

The full reserved range for this family is `[320, 383]`; codes `[323, 383]`
are reserved for future variants (e.g., attestor-signed envelopes).
