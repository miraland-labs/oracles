# Dockerized oracle deployment (dual-cluster reference)

Production deployment for `oracle-onchain-transfer` on a single Linux
host running **both Solana clusters**: one container against devnet, one
against mainnet, sharing the host's Postgres instance via per-cluster
databases. Postgres runs natively on the host; the oracle binary runs in
Docker, supervised by systemd.

This sits alongside the native-binary path in
[`scripts/install.sh`](../install.sh). Pick one shape, don't mix them.

## Files in this directory

| File | Role |
| --- | --- |
| `Dockerfile` | Multi-stage build: `rust:1.92-slim` builder → `debian:12-slim` runtime. Cluster-agnostic; the same image runs against any cluster via env vars. |
| `oracle-onchain-transfer-devnet.service` | systemd unit for the devnet container. References `oracle-onchain-transfer:current`. |
| `oracle-onchain-transfer-mainnet.service` | systemd unit for the mainnet container. Same image, different env file, different port. |
| `onchain-transfer-devnet.env.example` | Reference env file for devnet. Copy to `/etc/oracle/onchain-transfer-devnet.env` and fill in. |
| `onchain-transfer-mainnet.env.example` | Reference env file for mainnet. Copy to `/etc/oracle/onchain-transfer-mainnet.env` and fill in. |
| `oracle-deploy.sh` | Build + tag + restart + health-probe. Auto-rolls back on health failure. Pass `--unit oracle-onchain-transfer-devnet` (default) or `--unit oracle-onchain-transfer-mainnet`. |

The workspace-level [`.dockerignore`](../../.dockerignore) keeps the
build context small (no `target/`, no `.git/`, no `.kiro/`, no keypair
files).

## How dual-cluster works

| Concern | devnet | mainnet | Why decoupled |
| --- | --- | --- | --- |
| Docker image | `oracle-onchain-transfer:current` | `oracle-onchain-transfer:current` | Same image; cluster bound at runtime via env vars. |
| Container name | `oracle-onchain-transfer-devnet` | `oracle-onchain-transfer-mainnet` | systemd unit names match, no `docker run` collision. |
| systemd unit | `oracle-onchain-transfer-devnet.service` | `oracle-onchain-transfer-mainnet.service` | Each unit is enabled/restarted independently. |
| Env file | `/etc/oracle/onchain-transfer-devnet.env` | `/etc/oracle/onchain-transfer-mainnet.env` | Different RPC URL, ESCROW_PROGRAM_ID, TRANSFER_CLUSTER, BIND_ADDR. |
| Keypair file | `/var/lib/oracle/onchain-transfer-devnet/oracle-keypair.json` | `/var/lib/oracle/onchain-transfer-mainnet/oracle-keypair.json` | Cluster-bound: a mainnet `oracle_authority` must NEVER be reused on devnet (or vice versa). |
| BIND_ADDR | `127.0.0.1:4021` | `127.0.0.1:4031` | nginx routes by port; the two containers can't fight for the same port on `--network host`. |
| Postgres database | `oracle_onchain_transfer_devnet` | `oracle_onchain_transfer_mainnet` | Same Postgres instance, separate DBs. Blast-radius isolation: a regression on devnet can't corrupt mainnet's settlement audit log. |

## One-time setup

Bring-up has six numbered groups. Devnet and mainnet are symmetric — do
each group for both clusters.

```bash
# ----- 0. Host prerequisites (once for the box) ------------------------------
sudo apt-get install -y postgresql-16 docker.io git curl jq
# Add yourself to the docker group only if you want to run docker commands
# without sudo for diagnostics; the systemd unit doesn't need this.
# sudo usermod -aG docker "$USER" && newgrp docker

# ----- 1. Clone the repo ----------------------------------------------------
git clone https://github.com/miraland-labs/oracles.git /opt/src/oracles
cd /opt/src/oracles

# ----- 2. Postgres databases (one per cluster) ------------------------------
sudo -u postgres createuser oracle_app -P
# When prompted, set a strong password; use the same password for the
# two database connections below.
sudo -u postgres createdb -O oracle_app oracle_onchain_transfer_devnet
sudo -u postgres createdb -O oracle_app oracle_onchain_transfer_mainnet

# Apply the schema to both databases.
PGPASSWORD='<oracle_app password>' \
    psql -U oracle_app -h 127.0.0.1 -d oracle_onchain_transfer_devnet \
    -f oracle-common/migrations/init.sql

PGPASSWORD='<oracle_app password>' \
    psql -U oracle_app -h 127.0.0.1 -d oracle_onchain_transfer_mainnet \
    -f oracle-common/migrations/init.sql

# ----- 3. Keypairs (one per cluster, NEVER shared) --------------------------
# Devnet keypair:
sudo install -d -o root -g root -m 0750 /var/lib/oracle/onchain-transfer-devnet
sudo solana-keygen new --no-bip39-passphrase \
    -o /var/lib/oracle/onchain-transfer-devnet/oracle-keypair.json
sudo chmod 0600 /var/lib/oracle/onchain-transfer-devnet/oracle-keypair.json
solana airdrop 2 \
    "$(solana-keygen pubkey /var/lib/oracle/onchain-transfer-devnet/oracle-keypair.json)" \
    --url devnet

# Mainnet keypair:
sudo install -d -o root -g root -m 0750 /var/lib/oracle/onchain-transfer-mainnet
sudo solana-keygen new --no-bip39-passphrase \
    -o /var/lib/oracle/onchain-transfer-mainnet/oracle-keypair.json
sudo chmod 0600 /var/lib/oracle/onchain-transfer-mainnet/oracle-keypair.json
# Mainnet keypair: airdrop is not available. Send real SOL from your treasury:
#   solana transfer <mainnet_pubkey> 1 --url mainnet-beta --keypair <treasury>

# Back up BOTH keypair files NOW (3 copies: hot/warm/cold).

# ----- 4. Env files (one per cluster) ---------------------------------------
sudo install -d -o root -g root -m 0750 /etc/oracle

sudo install -m 0640 -o root -g root \
    scripts/docker/onchain-transfer-devnet.env.example \
    /etc/oracle/onchain-transfer-devnet.env
sudo install -m 0640 -o root -g root \
    scripts/docker/onchain-transfer-mainnet.env.example \
    /etc/oracle/onchain-transfer-mainnet.env

# Edit each file. Minimum: SOLANA_RPC_URL, SOLANA_WS_URL, ESCROW_PROGRAM_ID,
# DATABASE_URL, ORACLE_OPERATOR_TOKEN_SHA256.
sudo -e /etc/oracle/onchain-transfer-devnet.env
sudo -e /etc/oracle/onchain-transfer-mainnet.env

# ----- 5. systemd units -----------------------------------------------------
sudo install -m 0644 \
    scripts/docker/oracle-onchain-transfer-devnet.service \
    /etc/systemd/system/oracle-onchain-transfer-devnet.service
sudo install -m 0644 \
    scripts/docker/oracle-onchain-transfer-mainnet.service \
    /etc/systemd/system/oracle-onchain-transfer-mainnet.service
sudo systemctl daemon-reload

# ----- 6. Build + start (devnet first, mainnet after burn-in) ---------------
sudo bash scripts/docker/oracle-deploy.sh \
    --unit oracle-onchain-transfer-devnet
sudo systemctl enable oracle-onchain-transfer-devnet.service

# When devnet is healthy and you've completed the integration burn-in
# (see oracles/.kiro/specs/oracle-onchain-transfer-production-hardening
# Tasks 5 and 6), repeat for mainnet:
sudo bash scripts/docker/oracle-deploy.sh \
    --unit oracle-onchain-transfer-mainnet
sudo systemctl enable oracle-onchain-transfer-mainnet.service
```

When `oracle-deploy.sh` exits 0 with `/health → healthy`, that cluster
is ready. Don't promote mainnet until you've completed Task 5 (devnet
end-to-end runbook) and Task 6 (7-day burn-in) from the
production-hardening spec.

## Day-to-day commands

Substitute `<cluster>` with `devnet` or `mainnet`:

```bash
# Status (single cluster)
sudo systemctl status oracle-onchain-transfer-<cluster>.service
curl -fsS http://127.0.0.1:4021/health | jq .   # devnet (4031 for mainnet)

# Status (both at once)
sudo systemctl status oracle-onchain-transfer-devnet.service \
                     oracle-onchain-transfer-mainnet.service

# Logs
sudo journalctl -u oracle-onchain-transfer-<cluster>.service -f
docker logs -f oracle-onchain-transfer-<cluster>

# Restart (no rebuild — uses :current image)
sudo systemctl restart oracle-onchain-transfer-<cluster>.service

# Redeploy (rebuild + restart + auto-rollback on /health failure)
sudo bash scripts/docker/oracle-deploy.sh \
    --unit oracle-onchain-transfer-<cluster>

# Roll back to the previous SHA (no rebuild)
sudo bash scripts/docker/oracle-deploy.sh \
    --unit oracle-onchain-transfer-<cluster> --rollback
```

## How it works

**Image tag pattern**:
- `oracle-onchain-transfer:<sha>` is what the deploy script builds.
- `oracle-onchain-transfer:current` is what every systemd unit references.
- `oracle-onchain-transfer:previous` is preserved automatically before
  every retag, so `--rollback` always has a target.

The image is cluster-agnostic. The same `oracle-onchain-transfer:current`
tag is used by both the devnet and mainnet units; cluster binding is in
the env file (`SOLANA_RPC_URL`, `ESCROW_PROGRAM_ID`, `TRANSFER_CLUSTER`),
not in the binary.

**Networking**: `--network host` puts each container on the host's network
namespace. `127.0.0.1:5432` inside the container reaches the host's
Postgres. The two clusters bind on different ports (`127.0.0.1:4021` for
devnet, `127.0.0.1:4031` for mainnet) so they don't collide. nginx (or
your load balancer) terminates TLS publicly and proxies to the correct
port; see [`scripts/examples/oracle-nginx.conf`](../examples/oracle-nginx.conf)
for a reference site.

**Filesystem isolation**: Each container runs `--read-only` with a 64 MiB
tmpfs at `/tmp`. Only the keypair file is host-mounted (read-only). The
env file is read once at startup via `--env-file`.

**Privileges**: Container runs as root inside the rootfs. The rootfs is
read-only, no writable mounts beyond the tmpfs, and `--network host` makes
non-root containers no more secure for this threat model. The justification
is documented inline in the Dockerfile.

**Resource limits**: `--memory 1g --cpus 1` per container. The oracle is
I/O-bound, not compute-bound. Override the unit's `--memory`/`--cpus` if
you co-locate other workloads on the same box.

**Logs**: `--log-driver journald --log-opt tag=oracle-onchain-transfer-<cluster>`
sends container stdout/stderr to journald with a per-cluster tag. Both
`journalctl -u oracle-onchain-transfer-devnet.service` and
`journalctl -t oracle-onchain-transfer-devnet` find the logs;
`docker logs -f oracle-onchain-transfer-devnet` is equivalent.

## Adapting for other families

The Dockerfile is family-parameterized via `--build-arg FAMILY=`. The
deploy script accepts `--family api-quality` (or auto-detects from
`--unit`). To add `oracle-api-quality` running on devnet:

1. Copy `oracle-onchain-transfer-devnet.service` → `oracle-api-quality-devnet.service`,
   replace every `onchain-transfer` with `api-quality`, change BIND_ADDR
   to `4020` in the env example.
2. Copy `onchain-transfer-devnet.env.example` → `api-quality-devnet.env.example`,
   adapt the family-specific env vars (no `TRANSFER_CLUSTER` for
   api-quality, for example).
3. Run `oracle-deploy.sh --unit oracle-api-quality-devnet`. Same lifecycle.

## Why these choices

**Why not docker-compose**: a single binary plus an externally-managed
Postgres doesn't benefit from compose. Two binaries plus shared Postgres
also doesn't — the systemd unit per cluster gives independent restart
control, which compose's `restart: on-failure` per-service can match
but with extra YAML and a separate `compose up -d` step. systemd already
handles supervision; compose duplicates it.

**Why not all-Docker (Postgres in a container)**: valid alternative; we
chose host-Postgres because Postgres versions change rarely while the
oracle binary changes every release. The deployment philosophy: "Docker
for the moving piece, native for the boring piece." See
[`DEPLOYMENT.md`](../../docs/DEPLOYMENT.md) §2.2 for the all-containerized
shape.

**Why one image, two clusters**: the binary's cluster behavior is
configured at runtime through env vars. Building one image per cluster
would mean two `docker build` invocations producing identical bytes —
cheap to build but expensive to reason about (which container has which
binary?). The runtime-config pattern is what every Solana production
deployment converges on.

## Backups, rotations, audit

Everything in [`oracles/docs/OPERATIONS.md`](../../docs/OPERATIONS.md)
applies, with two substitutions:

| Operation | Native install | Dockerized |
| --- | --- | --- |
| Update binary | `cargo build` + `scripts/upgrade.sh` | `bash scripts/docker/oracle-deploy.sh --unit ...` |
| Read logs | `journalctl -u oracle@onchain-transfer.service` | `journalctl -u oracle-onchain-transfer-<cluster>.service` |
| Restart | `systemctl restart oracle@onchain-transfer.service` | `systemctl restart oracle-onchain-transfer-<cluster>.service` |
| Manual eval | `curl -X POST http://127.0.0.1:4021/evaluate ...` | identical (same port + token) |

For Postgres backups, see [`OPERATIONS.md` §4.2](../../docs/OPERATIONS.md#42-backup--restore).
The two cluster databases are separate `pg_dump` targets:

```bash
PGPASSWORD='...' pg_dump -F c \
    -f /backup/oracle_onchain_transfer_devnet_$(date +%F).dump \
    oracle_onchain_transfer_devnet
PGPASSWORD='...' pg_dump -F c \
    -f /backup/oracle_onchain_transfer_mainnet_$(date +%F).dump \
    oracle_onchain_transfer_mainnet
```
