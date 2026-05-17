# Operator scripts — Ubuntu 24.04 deployment

Four scripts in this directory cover the full deploy lifecycle of one or more
x402 oracle binaries on a fresh Ubuntu 24.04 host:

| Script                | What it does                                                              |
| --------------------- | ------------------------------------------------------------------------- |
| `install.sh`          | First-time install of one family binary (idempotent).                     |
| `upgrade.sh`          | Drop in a new binary + restart + `/health` probe.                         |
| `uninstall.sh`        | Disable + remove one family binary; preserves env file by default.        |
| `bootstrap-minio.sh`  | Self-host MinIO as the S3-compatible blob backend (idempotent).           |

The shared systemd templates `oracle@.service` and `oracle.target` are dropped
into `/etc/systemd/system/` by `install.sh` once and reused for every family.

## Smoke test on a clean Ubuntu 24.04 VM

The runbook below verifies install / upgrade / uninstall against a
freshly-provisioned VM (or container). Each step has a quick assertion so you
catch a regression at the right boundary.

### Prerequisites

```bash
sudo apt-get update
sudo apt-get install -y curl jq postgresql-client
```

A reachable Postgres (with `migrations/init.sql` already applied) and a
funded oracle keypair. For the file-delivery family, plan to run
`bootstrap-minio.sh` before the binary's first start.

### 1. `install.sh`

```bash
cd oracles
cargo build --release -p oracle-api-quality

sudo ./scripts/install.sh \
    api-quality \
    ./target/release/oracle-api-quality \
    ./oracle-api-quality/.env.example
```

Assertions:

```bash
systemctl is-active oracle@api-quality.service   # → active
ls -ld /opt/oracle/api-quality                   # owned by oracle:oracle
ls -l /etc/oracle/api-quality.env                # mode 0600, oracle:oracle
test -f /etc/systemd/system/oracle@.service       # template installed
test -f /etc/systemd/system/oracle.target          # aggregator installed
```

If `BIND_ADDR=0.0.0.0:4020` is the default, `curl http://127.0.0.1:4020/health`
returns `503` until you've configured a real RPC + keypair (the WebSocket
hasn't connected yet) — that's expected.

### 2. `upgrade.sh`

After editing `/etc/oracle/api-quality.env` to point at a real RPC + keypair:

```bash
cargo build --release -p oracle-api-quality
sudo ./scripts/upgrade.sh \
    api-quality \
    ./target/release/oracle-api-quality
```

For unattended deploys add `--auto-rollback` so a failed `/health` probe
restores the previous binary and restarts automatically.

Assertions:

```bash
# The script captures /opt/oracle/api-quality/oracle-api-quality.bak.<UTC-ts>
# before swapping. Newest 5 backups are kept (KEEP_BACKUPS env override).
ls -1 /opt/oracle/api-quality/oracle-api-quality.bak.* 2>/dev/null

# The script's own /health probe passed iff the WS came up; if it returns
# non-zero, journalctl -u oracle@api-quality.service --since '1 minute ago'
systemctl status oracle@api-quality.service --no-pager
curl -fsS http://127.0.0.1:4020/health | jq .status   # → "healthy" or "degraded"
```

Exit codes (`upgrade.sh`):

| Code | Meaning                                                   |
| ---- | --------------------------------------------------------- |
| 0    | Upgrade succeeded; old backups pruned per `KEEP_BACKUPS`. |
| 1    | Upgrade unhealthy; manual rollback required.              |
| 2    | Auto-rollback completed (with `--auto-rollback`).         |
| 3    | Auto-rollback FAILED.                                     |

### 3. Multi-family install

```bash
cargo build --release -p oracle-onchain-transfer -p oracle-file-delivery
sudo ./scripts/install.sh onchain-transfer ./target/release/oracle-onchain-transfer ./oracle-onchain-transfer/.env.example
sudo ./scripts/install.sh file-delivery    ./target/release/oracle-file-delivery   ./oracle-file-delivery/.env.example
systemctl list-units 'oracle@*.service' 'oracle.target'
```

`oracle.target` aggregates all installed family services so
`systemctl restart oracle.target` bounces them in unison.

### 4. `bootstrap-minio.sh`

```bash
sudo MINIO_ROOT_USER=oracle MINIO_ROOT_PASSWORD=changeme \
    ./scripts/bootstrap-minio.sh
```

Assertions:

```bash
systemctl is-active minio.service           # → active
curl -fsS http://127.0.0.1:9000/minio/health/live  # → 200
mc alias set local http://127.0.0.1:9000 oracle changeme
mc ls local/oracle-blobs                    # → empty bucket exists
```

After bootstrap, paste the printed `ORACLE_REGISTRY_S3_*` lines into
`/etc/oracle/file-delivery.env` and `systemctl restart oracle@file-delivery.service`.

### 5. `uninstall.sh`

```bash
sudo ./scripts/uninstall.sh api-quality
# Removes the binary + dirs but preserves /etc/oracle/api-quality.env so the
# operator's edits survive a redeploy.

# To remove the env file too:
sudo PRESERVE_ENV=0 ./scripts/uninstall.sh onchain-transfer
```

Assertions:

```bash
systemctl is-active oracle@api-quality.service     # → inactive (failed-to-start)
test -f /etc/oracle/api-quality.env                # default: still present
```

## CI smoke

The workspace's GitHub Actions matrix runs `shellcheck` against every
script in this directory; failures block PRs. Adding a new script means
adding a new line to the matrix in `.github/workflows/oracles.yml`.

## Production checklist

* `oracle:oracle` is a system user (no login shell, no home shell).
* Every `/etc/oracle/<family>.env` is mode `0600`, owner `oracle:oracle`.
* The oracle keypair is owned by `oracle:oracle`, mode `0600`, and lives
  under `/var/lib/oracle/<family>/`.
* `journalctl -u oracle@<family>.service` is the canonical log location.
* `/health` is gated by your reverse proxy or VPN if exposed publicly.
* `POST /evaluate` is gated by `ORACLE_OPERATOR_TOKEN_SHA256` (NEVER deploy
  with `ORACLE_ALLOW_UNAUTHENTICATED_MANUAL_EVALUATE=true` to production).
