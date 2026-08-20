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
| `Dockerfile` | Multi-stage build: `rust:1.92-slim` builder → `debian:12-slim` runtime. Pass `CARGO_FEATURES=devnet` for devnet program id; one image tag per cluster. |
| `oracle-onchain-transfer-devnet.service` | systemd unit for the devnet container. References `oracle-onchain-transfer-devnet:current`. |
| `oracle-onchain-transfer-mainnet.service` | systemd unit for the mainnet container. References `oracle-onchain-transfer-mainnet:current`. |
| `onchain-transfer-devnet.env.example` | Reference env file for devnet. Copy to `/etc/oracle/onchain-transfer-devnet.env` and fill in. |
| `onchain-transfer-mainnet.env.example` | Reference env file for mainnet. Copy to `/etc/oracle/onchain-transfer-mainnet.env` and fill in. |
| `oracle-file-delivery-devnet.service` | systemd unit for preview file-delivery (devnet). References `oracle-file-delivery-devnet:current`. Port **4022**. |
| `file-delivery-devnet.env.example` | Reference env for preview file-delivery. Copy to `/etc/oracle/file-delivery-devnet.env`. |
| `oracle-file-delivery-devnet-setup.sh` | Idempotent one-shot: DB + keypair + env + unit on a host that already runs onchain-transfer, then first deploy. |
| `oracle-file-delivery-devnet-deploy.sh` | Wrapper: `oracle-deploy.sh --unit oracle-file-delivery-devnet`. |
| `oracle-deploy.sh` | Build + tag + restart + health-probe. Auto-rolls back on health failure. Pass `--unit oracle-onchain-transfer-devnet` (default), `--unit oracle-onchain-transfer-mainnet`, or `--unit oracle-file-delivery-devnet`. |

The workspace-level [`.dockerignore`](../../.dockerignore) keeps the
build context small (no `target/`, no `.git/`, no `.kiro/`, no keypair
files).

## How dual-cluster works

| Concern | devnet | mainnet | Why decoupled |
| --- | --- | --- | --- |
| Docker image | `oracle-onchain-transfer-devnet:current` | `oracle-onchain-transfer-mainnet:current` | Separate tags; devnet build uses `--features devnet`. |
| Container name | `oracle-onchain-transfer-devnet` | `oracle-onchain-transfer-mainnet` | systemd unit names match, no `docker run` collision. |
| systemd unit | `oracle-onchain-transfer-devnet.service` | `oracle-onchain-transfer-mainnet.service` | Each unit is enabled/restarted independently. |
| Env file | `/etc/oracle/onchain-transfer-devnet.env` | `/etc/oracle/onchain-transfer-mainnet.env` | Different RPC URL, ESCROW_PROGRAM_ID, TRANSFER_CLUSTER, BIND_ADDR. |
| Keypair file | `/var/lib/oracle/onchain-transfer-devnet/oracle-keypair.json` | `/var/lib/oracle/onchain-transfer-mainnet/oracle-keypair.json` | Cluster-bound: a mainnet `oracle_authority` must NEVER be reused on devnet (or vice versa). |
| BIND_ADDR | `127.0.0.1:4021` | `127.0.0.1:4031` | nginx routes by port; the two containers can't fight for the same port on `--network host`. |
| Postgres database | `oracle_onchain_transfer_devnet` | `oracle_onchain_transfer_mainnet` | Same Postgres instance, separate DBs. Blast-radius isolation: a regression on devnet can't corrupt mainnet's settlement audit log. |

## Postgres model: one instance, multiple databases

The deployment runs **one** PostgreSQL server (installed via
`apt-get install postgresql-16`) holding **two** databases — one per
cluster — that are fully isolated at the database layer. Understanding
the distinction matters because Postgres uses the word "cluster" for
its own concept (a server process and its files on disk), which
collides with "Solana cluster" (devnet, mainnet, testnet). Throughout
this README we say **instance** for the Postgres process and
**database** for the logical container of tables, to keep the two
ideas separate.

### The hierarchy

```
PostgreSQL server process (one per host, listening on 127.0.0.1:5432)
└── databases (e.g. oracle_onchain_transfer_devnet)
    └── schemas (default: "public")
        └── tables, indexes, sequences
```

Each database is fully isolated from its siblings:

- Its own tables, indexes, sequences. A `CREATE TABLE` inside one
  database is invisible from any other connected to the same instance.
- Its own backup unit: `pg_dump <db>` produces one file; `pg_restore`
  affects only that database.
- Its own connection: a `DATABASE_URL` selects exactly one database
  (the path component after the last `/`).

What's *shared* across databases on the same instance:

- The Postgres server process and its memory + disk.
- Server-level config (`postgresql.conf`, `pg_hba.conf`).
- Roles (a user `oracle_app` exists at the server level and gets
  per-database privileges via `OWNER` or `GRANT`).

For the dual-cluster oracle setup this is the right shape:
**blast-radius isolation** where it matters (a corruption or accidental
`DELETE` in devnet's tables cannot reach mainnet) and **resource
sharing** where it's cheap (one Postgres process barely registers in
monitoring at our settlement rates).

### Creating the two databases

The bring-up below does this in three commands:

```bash
sudo -u postgres createuser oracle_app -P    # interactive password prompt
sudo -u postgres createdb -O oracle_app oracle_onchain_transfer_devnet
sudo -u postgres createdb -O oracle_app oracle_onchain_transfer_mainnet
```

What's happening:

- `createuser oracle_app -P` creates a Postgres role (the runtime user
  the oracle binary connects as). The `-P` flag prompts for a password.
  Don't reuse the `postgres` superuser — production hygiene says runtime
  processes should never have superuser rights.
- `createdb -O oracle_app <name>` creates a database **owned by**
  `oracle_app`, meaning that role can do anything inside it without
  further `GRANT` statements.
- The two databases are independent containers. Running the same schema
  migration against each (`psql -f init.sql` calls in step 2 below)
  produces two parallel, empty oracle ledgers.

### How connection strings stay separate

The only difference between the two oracle env files is the database
name at the end of the URL:

```
# /etc/oracle/onchain-transfer-devnet.env
DATABASE_URL=postgres://oracle_app:PASSWORD@127.0.0.1:5432/oracle_onchain_transfer_devnet

# /etc/oracle/onchain-transfer-mainnet.env
DATABASE_URL=postgres://oracle_app:PASSWORD@127.0.0.1:5432/oracle_onchain_transfer_mainnet
```

Same server (`127.0.0.1:5432`), same role (`oracle_app`), same password
— different database name. Postgres enforces that a connection to one
database cannot read or write tables in the other. Each oracle's
connection pool is bound to its own database.

### When you'd want a separate Postgres instance

The "one instance, multiple databases" model is the right default.
Reasons to run a *second* Postgres process (entirely separate server)
are narrow:

- **Different Postgres major versions** for the two clusters. Rare in
  practice; most operators upgrade in lockstep.
- **Dedicated resources**: e.g. mainnet on its own instance because
  devnet's traffic must never touch mainnet's CPU/IO. At our
  settlement rates this is unjustified.
- **Different network endpoints**: e.g. mainnet on a managed RDS,
  devnet on the local host. Possible later; not required for bring-up.

If you ever do split, the only change per oracle is the host/port in
each `DATABASE_URL`.

## One-time setup

Bring-up has six numbered groups. Devnet and mainnet are symmetric — do
each group for both clusters.

```bash
# ----- 0. Host prerequisites (once for the box) ------------------------------
sudo apt-get install -y postgresql-16 docker.io git curl jq
# Add yourself to the docker group only if you want to run docker commands
# without sudo for diagnostics; the systemd unit doesn't need this.
# sudo usermod -aG docker "$USER" && newgrp docker

# Solana CLI (required for Step 3 keypair generation and devnet airdrop).
sh -c "$(curl -sSfL https://release.anza.xyz/stable/install)"
# Activate it in this shell and persist for future logins:
export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"
echo 'export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"' >> ~/.bashrc
solana --version    # confirm

# ----- 1. Clone the repo ----------------------------------------------------
sudo install -d -o root -g root -m 0755 /opt/src
sudo git clone https://github.com/miraland-labs/oracles.git /opt/src/oracles
cd /opt/src/oracles
# All later steps assume CWD = /opt/src/oracles. The repo is root-owned;
# you can read it as your user, and the docker build / install commands
# below run under sudo so writes work.

# ----- 2. Postgres databases (one per cluster) ------------------------------
# (Run from /tmp to avoid the harmless "could not change directory to
# /home/..." warning when sudo'ing to the postgres user.)
cd /tmp

sudo -u postgres createuser oracle_app -P
# When prompted, set a strong password; use the same password for the
# two database connections below.
sudo -u postgres createdb -O oracle_app oracle_onchain_transfer_devnet
sudo -u postgres createdb -O oracle_app oracle_onchain_transfer_mainnet

# Apply the schema to both databases (use absolute paths — the postgres
# user must be able to read the file, and /opt/src/oracles is world-
# readable from the install -d above).
PGPASSWORD='<oracle_app password>' \
    psql -U oracle_app -h 127.0.0.1 -d oracle_onchain_transfer_devnet \
    -f /opt/src/oracles/oracle-common/migrations/init.sql

PGPASSWORD='<oracle_app password>' \
    psql -U oracle_app -h 127.0.0.1 -d oracle_onchain_transfer_mainnet \
    -f /opt/src/oracles/oracle-common/migrations/init.sql

# Sanity check: each database should now have ~9 oracle_* tables.
PGPASSWORD='<oracle_app password>' \
    psql -U oracle_app -h 127.0.0.1 -d oracle_onchain_transfer_devnet -c '\dt'

cd /opt/src/oracles   # back to repo root for Steps 3-6

# ----- 3. Keypairs (one per cluster, NEVER shared) --------------------------
# Devnet keypair:
sudo install -d -o root -g root -m 0750 /var/lib/oracle/onchain-transfer-devnet

# Generate as your user (so solana-keygen's PATH resolves), capture the
# pubkey, then atomically install with root ownership and 0600 mode.
solana-keygen new --no-bip39-passphrase -o /tmp/oracle-keypair-devnet.json
ORACLE_DEVNET_PUBKEY=$(solana-keygen pubkey /tmp/oracle-keypair-devnet.json)
echo "Devnet oracle pubkey: $ORACLE_DEVNET_PUBKEY"
sudo install -m 0600 -o root -g root \
    /tmp/oracle-keypair-devnet.json \
    /var/lib/oracle/onchain-transfer-devnet/oracle-keypair.json
shred -u /tmp/oracle-keypair-devnet.json

solana airdrop 2 "$ORACLE_DEVNET_PUBKEY" --url devnet

# Mainnet keypair:
sudo install -d -o root -g root -m 0750 /var/lib/oracle/onchain-transfer-mainnet

solana-keygen new --no-bip39-passphrase -o /tmp/oracle-keypair-mainnet.json
ORACLE_MAINNET_PUBKEY=$(solana-keygen pubkey /tmp/oracle-keypair-mainnet.json)
echo "Mainnet oracle pubkey: $ORACLE_MAINNET_PUBKEY"
sudo install -m 0600 -o root -g root \
    /tmp/oracle-keypair-mainnet.json \
    /var/lib/oracle/onchain-transfer-mainnet/oracle-keypair.json
shred -u /tmp/oracle-keypair-mainnet.json

# Mainnet keypair: airdrop is not available. Send real SOL from your treasury:
#   solana transfer <mainnet_pubkey> 1 --url mainnet-beta --keypair <treasury>

# Back up BOTH keypair files NOW (3 copies: hot/warm/cold).
# Read them via sudo, since they're now root-owned 0600:
#   sudo cat /var/lib/oracle/onchain-transfer-devnet/oracle-keypair.json
#   sudo cat /var/lib/oracle/onchain-transfer-mainnet/oracle-keypair.json

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

**Image tag pattern** (one namespace per cluster — no shared `:current`):

- `oracle-onchain-transfer-devnet:<sha>` / `oracle-onchain-transfer-mainnet:<sha>` — what the deploy script builds.
- `oracle-onchain-transfer-<cluster>:current` — what the matching systemd unit references.
- `oracle-onchain-transfer-<cluster>:previous` — preserved automatically before every retag, so `--rollback` always has a target.

**Build vs runtime cluster binding:**

- **Build time:** `--unit …-devnet` passes `CARGO_FEATURES=devnet` so `sla-escrow-api` links the devnet `declare_id!`. Mainnet/testnet units build without features (mainnet program id).
- **Runtime:** each env file still sets `ESCROW_PROGRAM_ID`, `SOLANA_RPC_URL`, and `TRANSFER_CLUSTER`. Monitor and settler use the runtime program id; compile-time id is fallback only.

Deploy both clusters from **one git branch** — no branch checkout per cluster:

```bash
git pull   # single branch (e.g. main)
sudo bash scripts/docker/oracle-deploy.sh --unit oracle-onchain-transfer-devnet
sudo bash scripts/docker/oracle-deploy.sh --unit oracle-onchain-transfer-mainnet
```

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

## Preview: `oracle-file-delivery` on the same host

Same Docker + systemd + host-Postgres shape as onchain-transfer, **devnet
only** (Forge preview). Isolation from the sibling family:

| Concern | onchain-transfer | file-delivery preview |
| --- | --- | --- |
| Image / container / unit | `oracle-onchain-transfer-{devnet,mainnet}` | `oracle-file-delivery-devnet` |
| BIND_ADDR | `4021` / `4031` | `4022` |
| Postgres database | `oracle_onchain_transfer_*` | `oracle_file_delivery_devnet` |
| Keypair | `/var/lib/oracle/onchain-transfer-*/` | `/var/lib/oracle/file-delivery-devnet/` |
| Extra env | `TRANSFER_CLUSTER` | `FORGE_VERDICT_BASE_URL=https://preview.forge.http402.trade` |

Do not share keypairs across families. Do not point this unit at
`https://forge.http402.trade`. There is no file-delivery mainnet unit yet.

On a box that already has onchain-transfer:

```bash
cd /opt/src/oracles   # or your checkout
sudo bash scripts/docker/oracle-file-delivery-devnet-setup.sh \
    --keypair /path/to/oracle-keypair.json
```

That creates the DB, installs the unit/env/keypair, then builds and
starts the container. Later rebuilds:

```bash
sudo bash scripts/docker/oracle-file-delivery-devnet-deploy.sh
```

Health: `curl -fsS http://127.0.0.1:4022/health`.

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
