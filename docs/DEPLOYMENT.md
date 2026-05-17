# Deployment Guide

Bring-up runbook for the x402 oracle workspace on a production Ubuntu 24.04
VPS. This guide takes you from a clean machine to three running oracles
settling against the deployed `sla-escrow` program.

For day-2 operations (monitoring, incidents, rotations, backup), see
[`OPERATIONS.md`](OPERATIONS.md). For the smoke-test variant of `install.sh` /
`upgrade.sh` / `uninstall.sh`, see [`../scripts/README.md`](../scripts/README.md).

---

## 1. Topology choices

Pick one of the three patterns before provisioning:

| Pattern                      | Best for                                       | Tradeoff                                        |
| ---------------------------- | ---------------------------------------------- | ----------------------------------------------- |
| **Single host, all three**   | Bootstrapping; low-volume mainnet              | Shared blast radius; one bad upgrade affects all |
| **Three hosts, one per family** | Production with independent SLOs per family   | Higher cost; per-family Postgres + MinIO        |
| **Hybrid**                   | api-quality on shared host, file-delivery on its own (large blobs)  | Mixed ops complexity                          |

In all three patterns each oracle binary holds **its own** keypair and
Postgres database — that's a hard design constraint (per
[`design.md` §C7, §Per-Family Postgres Isolation](../../.kiro/specs/multi-category-oracle-architecture/design.md))
and keeps the blast radius bounded.

## 2. Prerequisites

### Operating system

- Ubuntu 24.04 LTS (other distros work but scripts use `apt` and systemd
  paths).
- Root or `sudo` access.
- A non-root user `oracle:oracle` (created automatically by `install.sh`).

### Hardware sizing

| Family                  | vCPU   | RAM   | Disk                                    | Network    |
| ----------------------- | ------ | ----- | --------------------------------------- | ---------- |
| `oracle-api-quality`    | 1      | 1 GiB | 2 GiB binary+logs; Postgres external    | RPC + WS   |
| `oracle-onchain-transfer` | 1    | 1 GiB | 2 GiB                                   | RPC + WS   |
| `oracle-file-delivery`  | 2      | 2 GiB | 50 GiB+ for MinIO (sized to traffic)    | RPC + WS + MinIO |

The 64 KiB streaming buffer keeps oracle memory bounded regardless of blob
size; capacity sits in MinIO + Postgres.

### Kernel and OS limits

```bash
# /etc/security/limits.d/oracle.conf
oracle hard nofile 65535
oracle soft nofile 65535
```

NTP/`chronyd` enabled — clock drift breaks `is_eligible` Clock-vs-wall
fallback and confuses event timestamps in the lifecycle log.

### Network

- Outbound HTTPS to your Solana RPC endpoint and WebSocket.
- Outbound HTTPS to the evidence registry mirrors (if separate from the
  oracle's own registration HTTP).
- Inbound on `BIND_ADDR` for the registration HTTP and `/health` /
  `/metrics` (gated by your reverse proxy).
- Inbound on `:9000` for MinIO if hosted on the oracle box, or outbound to
  the MinIO host otherwise.

### RPC provider

Pick a provider with:

- **WebSocket `logsSubscribe` retention** — required by the chain monitor.
- **`getSignaturesForAddress` history** — required by the startup backfill.
- **`getTransaction(jsonParsed)` support** — required by
  `oracle-onchain-transfer` for delta re-derivation.
- **Reasonable rate limits** — sustained `getTransaction` volume is the
  hottest endpoint.

For Mainnet, treat the RPC URL as critical infra: have a primary + a
fallback list. The oracle reads `EVIDENCE_REGISTRY_URLS` from
comma-separated config; the same pattern applies for RPC mirrors at the
load-balancer layer.

### Firewall (`ufw` example)

```bash
sudo ufw allow 22/tcp           # ssh
sudo ufw allow 4020:4022/tcp    # oracle HTTP (or restrict to your reverse proxy)
sudo ufw allow 9000/tcp         # MinIO admin (or restrict to oracle host)
sudo ufw enable
```

For production, **bind the oracle to `127.0.0.1`** and front it with nginx /
Caddy / your load-balancer; do not expose `BIND_ADDR=0.0.0.0:4020` directly.

## 3. Postgres provisioning

One database per family. The schema (`oracle-common/migrations/init.sql`) is
identical for all three.

### 3.1 Install + create databases

```bash
sudo apt-get install -y postgresql-16

sudo -u postgres psql <<'SQL'
CREATE ROLE oracle_app LOGIN PASSWORD '<strong-password>';
CREATE DATABASE oracle_api_quality      OWNER oracle_app;
CREATE DATABASE oracle_onchain_transfer OWNER oracle_app;
CREATE DATABASE oracle_file_delivery    OWNER oracle_app;
SQL
```

### 3.2 Apply schema to each database

```bash
for db in oracle_api_quality oracle_onchain_transfer oracle_file_delivery; do
    PGPASSWORD='<strong-password>' \
    psql -U oracle_app -h 127.0.0.1 -d "$db" \
         -f oracle-common/migrations/init.sql
done
```

Sanity check (per database):

```bash
PGPASSWORD='...' psql -U oracle_app -h 127.0.0.1 -d oracle_api_quality \
    -c '\dt oracle_*'
```

Expected tables: `oracle_jobs`, `oracle_verdicts`, `oracle_lifecycle_events`,
`oracle_parameters`, `oracle_seller_keys`, `oracle_deliveries`,
`oracle_artifacts`, `oracle_registered_profiles`.

### 3.3 TLS

Production deployments **must** use TLS to Postgres:

```dotenv
DATABASE_URL=postgres://oracle_app:secret@db.internal/oracle_api_quality?sslmode=require
```

The oracle uses `postgres-openssl` and respects standard libpq TLS env vars
(`PGSSLROOTCERT`, `PGSSLMODE`).

### 3.4 Backups

`pg_dump` nightly, kept ≥ 30 days. Optional WAL archiving / PITR for
high-stakes deployments. See [`OPERATIONS.md`](OPERATIONS.md#backup--restore)
for restore procedure.

### 3.5 Sizing

`oracle_jobs` and `oracle_verdicts` grow ~1 row per settled payment; the
audit log `oracle_lifecycle_events` grows ~5 rows per settlement. At 100k
settlements, expect ≤ 200 MB. `oracle_artifacts` (Postgres backend only)
holds the bytes themselves — switch to MinIO if you expect any blob > a
few MiB.

## 4. MinIO provisioning (`oracle-file-delivery` only)

Required only for `oracle-file-delivery` at scale, or anywhere
`ORACLE_REGISTRY_BACKEND=s3`.

### 4.1 Bootstrap

```bash
sudo MINIO_ROOT_USER=oracle MINIO_ROOT_PASSWORD='<strong-password>' \
    bash oracles/scripts/bootstrap-minio.sh
```

The script is idempotent: it installs MinIO, enables the systemd unit,
creates the bucket `oracle-blobs`, and prints the env vars to copy into
`/etc/oracle/file-delivery.env`:

```dotenv
ORACLE_REGISTRY_BACKEND=s3
ORACLE_REGISTRY_S3_ENDPOINT=http://127.0.0.1:9000
ORACLE_REGISTRY_S3_BUCKET=oracle-blobs
ORACLE_REGISTRY_S3_ACCESS_KEY=oracle
ORACLE_REGISTRY_S3_SECRET_KEY=<strong-password>
ORACLE_REGISTRY_S3_REGION=us-east-1
```

### 4.2 Single-node vs distributed

For most deployments a single MinIO node is fine; the oracle treats it as
the storage primary. For higher durability, run MinIO in distributed
erasure-coded mode (4-node minimum) and point the oracle's
`ORACLE_REGISTRY_S3_ENDPOINT` at the load-balancer in front. The oracle
re-verifies SHA-256 over every fetched body, so a corrupt MinIO disk
surfaces as a fail-closed `500` from `GET /v1/registry/{sha256}` rather
than a wrong verdict.

### 4.3 Alternatives

The S3 backend works against any S3-compatible endpoint without code
changes. Swap MinIO for **AWS S3**, **Cloudflare R2**, **Backblaze B2**, or
**Wasabi** by changing `ORACLE_REGISTRY_S3_ENDPOINT` plus credentials.

## 5. Oracle keypair lifecycle

Each binary holds **one** Ed25519 keypair as its `oracle_authority`. This
is the same pubkey buyers fund escrows against, so its security is paramount.

### 5.1 Generate

```bash
sudo -u oracle solana-keygen new \
    --no-bip39-passphrase \
    -o /var/lib/oracle/api-quality/oracle-keypair.json
sudo chmod 600 /var/lib/oracle/api-quality/oracle-keypair.json
```

Repeat per family with distinct keypairs — running two binaries with the
same keypair is unsupported (the on-chain program accepts exactly one
settlement per `payment_uid`; the second loses the race).

### 5.2 Fund

```bash
solana airdrop 2 \
    "$(solana-keygen pubkey /var/lib/oracle/api-quality/oracle-keypair.json)" \
    --url devnet
```

For Mainnet, send SOL from a hot wallet — typical settlement fee is ~5000
lamports plus oracle tip (verdict-neutral; paid by `sla-escrow` regardless
of approve/reject). Budget for `n` settlements/day × ~10000 lamports as a
safety margin.

### 5.3 Storage

- **Hot** (online, on the oracle host): the keypair file at
  `/var/lib/oracle/<family>/oracle-keypair.json`, mode 0600,
  owner `oracle:oracle`.
- **Warm** (encrypted offline copy, separate machine): for fast disaster
  recovery. Encrypt with `age` or `gpg`; rotate the master key annually.
- **Cold** (paper / hardware): mnemonic-printed paper backup in a safe.

### 5.4 Rotation

Rotation = generate new keypair, register it with `pr402` as the new
advertised authority, drain in-flight settlements on the old key, switch
the env var, restart. See
[`OPERATIONS.md` §Rotations](OPERATIONS.md#rotations) for the full
procedure.

## 6. Per-family install

The `install.sh` script is idempotent — re-running it leaves operator edits
intact.

### 6.1 Build

```bash
cd oracles
cargo build --workspace --release
```

Binaries land at `target/release/oracle-{api-quality,onchain-transfer,file-delivery}`.

### 6.2 Install

```bash
sudo ./scripts/install.sh \
    api-quality \
    ./target/release/oracle-api-quality \
    ./oracle-api-quality/.env.example

sudo ./scripts/install.sh \
    onchain-transfer \
    ./target/release/oracle-onchain-transfer \
    ./oracle-onchain-transfer/.env.example

sudo ./scripts/install.sh \
    file-delivery \
    ./target/release/oracle-file-delivery \
    ./oracle-file-delivery/.env.example
```

The script:

1. Creates `oracle:oracle` system user (no login shell) if missing.
2. Drops the binary at `/opt/oracle/<family>/oracle-<family>`, mode 0755.
3. Creates `/var/lib/oracle/<family>/` (state) and `/etc/oracle/` (env).
4. Copies the `.env.example` to `/etc/oracle/<family>.env` mode 0600 only
   if no env file already exists (so re-running does NOT clobber operator
   edits).
5. Installs `oracle@.service` template + `oracle.target` aggregator.
6. Enables and starts `oracle@<family>.service`.

### 6.3 Configure

Edit each env file to reference real values:

```bash
sudo -u oracle vi /etc/oracle/api-quality.env
```

Minimum required edits:

- `SOLANA_RPC_URL`, `SOLANA_WS_URL` — point at your provider.
- `ORACLE_KEYPAIR_PATH` — path to the funded keypair from §5.
- `ESCROW_PROGRAM_ID` — devnet or mainnet program id from the `sla-escrow`
  deployment.
- `DATABASE_URL` — the per-family Postgres URL from §3.
- `EVIDENCE_REGISTRY_URL` or `EVIDENCE_REGISTRY_URLS` — your registry
  mirror list (set even for self-hosted: this is what the chain monitor
  uses to fetch SLA bytes).
- `ORACLE_OPERATOR_TOKEN_SHA256` — `sha256` of the operator token (`echo
  -n "$TOKEN" | sha256sum`); production must NOT use
  `ORACLE_ALLOW_UNAUTHENTICATED_MANUAL_EVALUATE=true`.
- `ORACLE_REGISTRY_BACKEND` — `postgres` for api-quality / onchain-transfer
  (small JSON only), `s3` for file-delivery.
- For `oracle-onchain-transfer`: `TRANSFER_CLUSTER` (`mainnet` /
  `devnet` / `testnet` / `custom`) — the evaluator refuses to verify
  signatures from a different cluster.

### 6.4 Restart and verify

```bash
sudo systemctl restart oracle@api-quality.service
sudo systemctl status oracle@api-quality.service
sudo journalctl -u oracle@api-quality.service --since '1 minute ago' -n 100
```

Expected log lines on a healthy boot:

```
INFO oracle_api_quality: HTTP server listening on 127.0.0.1:4020
INFO oracle_common::chain: WebSocket connected
INFO oracle_common::chain: Backfill: scanning up to 2000 signatures for ...
```

`/health` returns `200` once chain + WS are both up:

```bash
curl -fsS http://127.0.0.1:4020/health | jq .
```

## 7. Reverse proxy and TLS

Bind oracles to `127.0.0.1` and front with nginx (recommended) or Caddy.

### 7.1 nginx example

```nginx
# /etc/nginx/sites-available/oracle-api-quality
server {
    listen 443 ssl http2;
    server_name oracle-api.example.com;

    ssl_certificate     /etc/letsencrypt/live/oracle-api.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/oracle-api.example.com/privkey.pem;

    # Public endpoints
    location = /health  { proxy_pass http://127.0.0.1:4020; }
    location = /metrics {
        # restrict to your monitoring CIDR
        allow 10.0.0.0/8;
        deny all;
        proxy_pass http://127.0.0.1:4020;
    }

    # Seller registration HTTP — bearer-gated by the oracle itself.
    location /v1/registry/ {
        client_max_body_size 5G;       # mirror ORACLE_REGISTRY_MAX_BLOB_BYTES
        proxy_request_buffering off;    # streaming uploads
        proxy_read_timeout 600s;
        proxy_pass http://127.0.0.1:4020;
    }

    # Operator-only manual evaluate — restrict at proxy layer too.
    location = /evaluate {
        allow 10.0.0.0/8;               # operator network
        deny all;
        proxy_pass http://127.0.0.1:4020;
    }

    # Default deny.
    location / { return 404; }
}
```

### 7.2 CORS

`ORACLE_CORS_ALLOWED_ORIGINS` defaults to none (server-to-server only).
Only set it if you have a browser-based operator console:

```dotenv
ORACLE_CORS_ALLOWED_ORIGINS=https://ops.example.com
```

The oracle accepts both `Authorization: Bearer ...` and `X-Oracle-Token: ...`
for operator endpoints; the CORS layer allowlists both headers.

## 8. Seller onboarding

This is the seller-facing flow that produces a bearer token usable for
`POST /v1/registry/{sla,delivery,blob}`. The seller wallet must sign an HMAC
challenge.

### 8.1 Challenge → register → bearer

```bash
# 1. Challenge (oracle returns a 32-byte challenge bound to the seller wallet)
curl -fsS "https://oracle-api.example.com/v1/registry/seller/challenge?wallet=$SELLER_PUBKEY" | jq .

# 2. Seller signs the challenge bytes with their wallet keypair (Ed25519).
#    Then registers.
curl -fsS -X POST "https://oracle-api.example.com/v1/registry/seller/register" \
    -H "Content-Type: application/json" \
    -d '{
      "wallet": "'"$SELLER_PUBKEY"'",
      "challenge": "'"$CHALLENGE_HEX"'",
      "signature": "'"$ED25519_SIG_BASE58"'",
      "label": "seller-prod-key-1"
    }' | jq .

# Response includes the raw bearer ONCE; store it securely.
# {"id":1,"bearer":"orc-...","wallet":"..."}
```

The oracle stores **only `SHA256(bearer)`** in `oracle_seller_keys`. The
raw token is never recoverable.

### 8.2 Rotate

```bash
curl -fsS -X POST "https://oracle-api.example.com/v1/registry/seller/rotate" \
    -H "Authorization: Bearer $OLD_BEARER" | jq .
# Returns the new bearer; the old one is revoked atomically.
```

### 8.3 Upload SLA + delivery + blob

```bash
# SLA (small JSON; profile_id required)
curl -fsS -X POST "https://oracle-api.example.com/v1/registry/sla" \
    -H "Authorization: Bearer $BEARER" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq .

# Delivery evidence JSON (api-quality, onchain-transfer)
curl -fsS -X POST "https://oracle-api.example.com/v1/registry/delivery" \
    -H "Authorization: Bearer $BEARER" \
    -H "Content-Type: application/json" \
    --data-binary @delivery.json | jq .

# Blob (file-delivery family; up to ORACLE_REGISTRY_MAX_BLOB_BYTES)
curl -fsS -X POST "https://oracle-file.example.com/v1/registry/blob" \
    -H "Authorization: Bearer $BEARER" \
    -H "Content-Type: video/mp4" \
    --data-binary @video.mp4 | jq .
```

The response includes the SHA-256 hex (the seller commits this as
`delivery_hash` on-chain).

## 9. pr402 integration

Once the oracle is healthy and reachable, register it with `pr402` so it
gets advertised on `GET /api/v1/facilitator/capabilities` under
`slaEscrowOracleProfiles[]`. Sellers and buyers discover oracles through
that endpoint plus the seller's own HTTP-402 challenge:

```json
{
  "accepts": [{
    "scheme": "v2:solana:sla-escrow",
    "extra": {
      "oracleProfiles": [{
        "profileId": "x402/oracles/api-quality/v1",
        "operatorPubkey": "OracLe...",
        "registry": "https://oracle-api.example.com/v1/registry"
      }]
    }
  }]
}
```

### 9.1 Tell pr402 about your oracle

A copy-paste helper generates the SQL pr402's operator runs against the
facilitator's `parameters` table:

```bash
bash oracles/scripts/announce-to-pr402.sh \
    https://oracle-api.example.com
```

Output is a short `INSERT INTO parameters ... ON CONFLICT DO UPDATE` block.

**If you operate pr402 yourself**, run the SQL directly. Within ~60
seconds (parameters cache TTL), `GET /capabilities` exposes your oracle.
Verify with the `curl | jq` suggestion the script prints.

**If pr402 is operated by someone else (the typical case)**, open a
registration issue against the pr402 repository:

> https://github.com/miralandlabs/pr402/issues/new?template=register-oracle.md

The template prompts for the SQL block, contact info, and a small set of
operator attestations (keypair custody, uptime commitment, devnet
evidence). The pr402 operator reviews and runs the SQL on accept; the
issue thread is the public audit trail. Listing is **editorial** —
treat it as a brief review process, not self-service.

Both paths produce the same end state: your oracle visible at
`GET /api/v1/facilitator/capabilities → slaEscrowOracleProfiles[]` for
sellers and buyers to discover.

The helper reads your oracle's own `/v1/registry/info` and `/health`
endpoints — no auth, no DB access required.

### 9.2 Help sellers reference your oracle

Sellers paste an `oracleProfiles[]` entry into their HTTP-402 challenge.
Generate it with one command:

```bash
bash oracles/scripts/seller-emit-oracle-profile.sh \
    https://oracle-api.example.com
```

Output is a single JSON object the seller drops into
`accepts[].extra.oracleProfiles[]`.

### 9.3 What pr402 enforces

- Every `operatorPubkey` in the seller's `oracleProfiles[]` MUST appear in
  `oracleAuthorities[]` (the flat allowlist).
- The buyer's chosen `oracle_authority` at `POST /build-sla-escrow-payment-tx`
  MUST match an advertised profile's `operatorPubkey`.
- When `PR402_SLA_ESCROW_REQUIRE_PROFILE_MATCH=true` is set on pr402, the
  matched profile's `profileId` MUST also be one of the profiles pr402
  advertises on `/capabilities`. Off by default; flip on once the
  ecosystem has stabilised.

See [`oracle-common/docs/PR402_CONTRACT.md`](../oracle-common/docs/PR402_CONTRACT.md)
for the full normative shape.

## 10. Bring-up validation checklist

Run all of these before declaring the deployment production-ready.

- [ ] All three `oracle@*.service` units `active (running)` per
      `systemctl status oracle.target`.
- [ ] `curl https://oracle-api.example.com/health | jq .status` returns
      `"healthy"` (not `"degraded"`); `chain_connected=true` and
      `websocket_connected=true`.
- [ ] `oracle_balance_lamports` from `/health` is at least 1 SOL on Mainnet
      (or `>= 0.1` SOL on Devnet).
- [ ] `curl -H "Authorization: Bearer $BAD" https://...evaluate` returns
      `401`; `... -H "Authorization: Bearer $GOOD" ...` reaches the handler
      (returns `404` for an unassigned payment, which is correct).
- [ ] Prometheus scrape from your monitoring stack returns metrics:
      `total_evaluated`, `queue_depth`, `oracle_balance_lamports`, etc.
      See [`OPERATIONS.md` §Monitoring](OPERATIONS.md#monitoring) for the
      full alert set.
- [ ] Postgres ledger reachable: `psql ... -c 'SELECT 1 FROM oracle_jobs LIMIT 1'`
      succeeds (empty table is fine).
- [ ] (file-delivery only) `mc ls oracle/oracle-blobs` returns OK; a 5 MiB
      test upload via `POST /v1/registry/blob` round-trips correctly.
- [ ] One end-to-end devnet flow per family: see
      `oracle-*/tests/devnet/*.sh` — fund an escrow, submit delivery,
      observe `oracle_jobs.status='settled'`.
- [ ] At least one **negative** flow: e.g., malformed SLA → ledger row
      transitions to `failed`, not to `settled`.
- [ ] `journalctl -u oracle@*.service --since '1 hour ago' | grep -i error`
      is empty (or only carries explained-by-flow errors like
      `EvidenceNotFound` from the negative test).

## 11. What to do if something doesn't work

- `/health` returns `503` after start: tail
  `journalctl -u oracle@<family>.service -f` and check for
  `WebSocket connect failed` (RPC issue), `database connection refused`
  (DB issue), or `keypair not found`.
- `/health` flaps `chain_connected: true → false → true`: usually RPC
  rate-limit. Switch to a paid endpoint or add a fallback URL.
- Settlements never happen even though `deliveries_observed` increments:
  `EVIDENCE_REGISTRY_URL` mismatch — the chain monitor can't fetch SLA
  bytes. Verify `curl $REGISTRY_URL/<sla_hash>` works from the oracle
  host.
- Settlements fail with `BlockhashNotFound`: known RPC condition; the
  oracle retries up to `ORACLE_DEAD_LETTER_MAX_ATTEMPTS`. If persistent,
  rotate to a different RPC.

For deeper diagnostics, see [`OPERATIONS.md` §Incident
playbooks](OPERATIONS.md#incident-playbooks).
