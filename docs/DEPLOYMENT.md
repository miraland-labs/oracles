# Deployment Guide

Bring up an x402 oracle on a clean Ubuntu 24.04 host. Two paths:

- **Quickstart** (~30 minutes, single host, devnet) — §1.
- **Production-grade** (TLS, MinIO, separate hosts, mainnet) — §2.

Each oracle binary holds its own keypair, its own Postgres database, and
its own `oracle_authority` on-chain. That isolation is the whole point;
don't share keypairs across binaries.

For day-2 operations see [`OPERATIONS.md`](OPERATIONS.md). For the
cross-actor protocol, see [`SLA_ESCROW_PROTOCOL.md`](SLA_ESCROW_PROTOCOL.md).
For script smoke tests see [`../scripts/README.md`](../scripts/README.md).

> **Two deployment shapes.** This guide assumes the **native binary +
> systemd** path (binary at `/opt/oracle/<family>/`, supervised by
> `oracle@<family>.service`). For the **Dockerized binary + systemd**
> path — same Postgres-on-host model, just the binary inside a
> container — see [`../scripts/docker/README.md`](../scripts/docker/README.md).
> Pick one. Don't mix them on the same host.

---

## 1. Quickstart

Goal: a healthy `oracle-api-quality` settling devnet payments in 30
minutes. Everything else (other families, TLS, MinIO, mainnet) is just
"do this again with different env values."

```bash
# 1. Postgres + schema (one DB per family).
sudo apt-get install -y postgresql-16
sudo -u postgres createuser oracle_app -P    # set a password
sudo -u postgres createdb -O oracle_app oracle_api_quality
PGPASSWORD='...' psql -U oracle_app -h 127.0.0.1 -d oracle_api_quality \
    -f oracle-common/migrations/init.sql

# 2. Funded keypair (the oracle's authority).
sudo install -d -o oracle -g oracle -m 0750 /var/lib/oracle/api-quality
sudo -u oracle solana-keygen new --no-bip39-passphrase \
    -o /var/lib/oracle/api-quality/oracle-keypair.json
sudo chmod 600 /var/lib/oracle/api-quality/oracle-keypair.json
PUBKEY=$(solana-keygen pubkey /var/lib/oracle/api-quality/oracle-keypair.json)
solana airdrop 2 "$PUBKEY" --url devnet

# 3. Build + install.
cargo build --workspace --release
sudo ./scripts/install.sh \
    api-quality \
    ./target/release/oracle-api-quality \
    ./oracle-api-quality/.env.example

# 4. Edit /etc/oracle/api-quality.env. Minimum: SOLANA_RPC_URL,
#    SOLANA_WS_URL, ORACLE_KEYPAIR_PATH, ESCROW_PROGRAM_ID, DATABASE_URL,
#    EVIDENCE_REGISTRY_URL, ORACLE_OPERATOR_TOKEN_SHA256.
sudo -u oracle vi /etc/oracle/api-quality.env

# 5. Restart and confirm.
sudo systemctl restart oracle@api-quality.service
curl -fsS http://127.0.0.1:4020/health | jq .status   # → "healthy"
```

Repeat steps 1–5 for `onchain-transfer` and `file-delivery` if you want
more than one family. Default ports: 4020 / 4021 / 4022.

When you see `"healthy"` and a non-zero `oracle_balance_lamports` in
`/health`, you're done. Move to §2 only if you need production-grade
hardening.

---

## 2. Production-grade additions

Each subsection below is independent. Enable the ones you need.

### 2.1 Bind to localhost, front with TLS

Set `BIND_ADDR=127.0.0.1:4020` in the env file and put a reverse proxy
in front. nginx, Caddy, or any HTTPS frontend works; the only oracle
constraints are:

- Stream the registration POSTs (no buffering): `proxy_request_buffering
  off`, `client_max_body_size` at least
  `ORACLE_REGISTRY_MAX_BLOB_BYTES`.
- Restrict `/metrics` and `/evaluate` to your operator network.
- `/health` and `/v1/registry/...` may be public (the registry is
  bearer-gated by the oracle).

A reference nginx site is in
[`scripts/examples/oracle-nginx.conf`](../scripts/examples/oracle-nginx.conf)
(if you want a starting point).

### 2.2 Postgres for production

- `DATABASE_URL=...?sslmode=require` (the oracle uses `postgres-openssl`
  and respects libpq env vars).
- One database per family. The schema in
  [`oracle-common/migrations/init.sql`](../oracle-common/migrations/init.sql)
  is identical for all three.
- `pg_dump` nightly, retention ≥ 30 days.

### 2.3 MinIO for `oracle-file-delivery`

```bash
sudo MINIO_ROOT_USER=oracle MINIO_ROOT_PASSWORD='...' \
    bash scripts/bootstrap-minio.sh
```

The script is idempotent. It prints the env block to paste into
`/etc/oracle/file-delivery.env`. For distributed (4+ node) MinIO, see
the comments at the top of `bootstrap-minio.sh`. Any S3-compatible
backend (AWS S3, Cloudflare R2, Backblaze B2) works by changing
`ORACLE_REGISTRY_S3_ENDPOINT` plus credentials.

The other two families default to `ORACLE_REGISTRY_BACKEND=postgres`
(the artifacts are small JSON; Postgres is fine).

### 2.4 RPC provider

Pick a provider with `logsSubscribe` retention,
`getSignaturesForAddress` history (for backfill), and reasonable rate
limits. For mainnet, configure a fallback URL list at the load-balancer
layer (the oracle itself takes one URL).

`oracle-onchain-transfer` is the most RPC-hungry: it issues one
`getTransaction(jsonParsed)` per delivery. Size your RPC tier
accordingly.

### 2.5 OS hygiene

`ulimits`, `chronyd`, `ufw`, NTP — configure as you would for any
production HTTP service. `oracle:oracle` user is created by
`install.sh`. The systemd unit handles restart-on-crash.

### 2.6 Mainnet vs devnet

The only differences are `SOLANA_RPC_URL` / `SOLANA_WS_URL`,
`ESCROW_PROGRAM_ID` (the deployed program id for the cluster), and the
keypair funding (real SOL vs airdrop). For
`oracle-onchain-transfer` also set `TRANSFER_CLUSTER=mainnet`.

---

## 3. Bring-up checklist

Before declaring the deployment done:

- [ ] `systemctl status oracle@<family>.service` is `active (running)`.
- [ ] `/health` → `"status":"healthy"` with `chain_connected=true`,
      `websocket_connected=true`.
- [ ] `oracle_balance_lamports` ≥ 1 SOL on mainnet (≥ 0.1 SOL on devnet).
- [ ] Operator token works: `curl -H "Authorization: Bearer $GOOD"
      .../evaluate` reaches the handler (404 for an unassigned payment
      is correct fail-closed behavior).
- [ ] One end-to-end devnet flow: fund an escrow, submit delivery,
      observe `oracle_jobs.status='settled'`. Negative test:
      malformed SLA → `oracle_jobs.status='failed'`.
- [ ] `journalctl -u oracle@*.service --since '1 hour ago' | grep -i
      error` is empty (or only carries explained-by-flow errors).

---

## 4. Telling pr402 about your oracle

Once healthy, register so pr402 advertises your oracle on
`GET /capabilities`. Two paths:

**You operate pr402**: run

```bash
bash scripts/announce-to-pr402.sh https://oracle-api.example.com
```

The output is one `INSERT INTO parameters ... ON CONFLICT DO UPDATE`
block. Apply it to your pr402 database. Within ~60 seconds (parameters
cache TTL), `GET /capabilities` exposes your oracle.

**Someone else operates pr402** (typical): open a registration issue
with the SQL block in the body —

> https://github.com/miralandlabs/pr402/issues/new?template=register-oracle.md

The pr402 operator reviews and applies. The issue thread is the public
audit trail.

To help sellers reference your oracle in their HTTP-402 challenge:

```bash
bash scripts/seller-emit-oracle-profile.sh https://oracle-api.example.com
```

prints a single JSON object the seller drops into
`accepts[].extra.oracleProfiles[]`.

---

## 5. Seller bearer tokens

Sellers prove ownership of their wallet to get a bearer token for
uploading SLA + delivery bytes. This is run by the seller, not the
operator, but it's worth knowing the flow exists:

1. `GET /v1/registry/seller/challenge?wallet=<pubkey>` → 32-byte challenge.
2. Seller signs the challenge with their wallet keypair.
3. `POST /v1/registry/seller/register` with `{wallet, challenge,
   signature}` → bearer (returned **once**; only `SHA256(bearer)` is
   stored).
4. Rotate via `POST /v1/registry/seller/rotate` with the old bearer.

Helper: [`scripts/seller-register.sh`](../scripts/seller-register.sh).
Full flow in [`SELLER_GUIDE.md`](SELLER_GUIDE.md).

---

## 6. Topology choices (skip on first read)

| Pattern | Best for | Tradeoff |
| --- | --- | --- |
| Single host, all three | Bootstrapping; low-volume mainnet | Shared blast radius |
| Three hosts, one per family | Production with independent SLAs | Higher cost; per-family Postgres + MinIO |
| Hybrid (api-quality + onchain-transfer shared, file-delivery separate) | Mixed workloads | Mixed ops complexity |

Sizing per family: 1 vCPU, 1 GiB RAM, 2 GiB disk for api-quality and
onchain-transfer. 2 vCPU, 2 GiB RAM, 50+ GiB MinIO storage for
file-delivery. The 64 KiB streaming buffer keeps memory bounded
regardless of blob size; capacity sits in MinIO + Postgres.

---

## 7. Troubleshooting

If `/health` returns 503 after start, tail
`journalctl -u oracle@<family>.service -f` and check:

- `WebSocket connect failed` → RPC issue (check URL, rate limit).
- `database connection refused` → Postgres URL wrong, sslmode missing,
  or DB unreachable.
- `keypair not found` → `ORACLE_KEYPAIR_PATH` wrong or file unreadable
  to `oracle:oracle`.

If settlements never happen but `deliveries_observed` increments, the
chain monitor saw the event but couldn't fetch SLA bytes. Verify
`curl $EVIDENCE_REGISTRY_URL/<sla_hash>` works from the oracle host.

Anything else: see [`OPERATIONS.md`](OPERATIONS.md) — the incident
playbooks cover the recurring failures.
