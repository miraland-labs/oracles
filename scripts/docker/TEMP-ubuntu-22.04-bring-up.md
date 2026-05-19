# TEMP: Bring-up on Ubuntu 22.04 (jammy)

> **This is a temporary document.** The canonical bring-up target is
> Ubuntu 24.04 + Postgres 16, documented in
> [`README.md`](README.md) and [`../../docs/DEPLOYMENT.md`](../../docs/DEPLOYMENT.md).
>
> This file exists only to walk through the same flow on a temporary
> Ubuntu 22.04 host (default repos ship Postgres 14, not 16). When you
> migrate to your 24.04 target, follow the canonical README — this file
> will be deleted.

## What's different on 22.04

| Concern | Ubuntu 24.04 (target) | Ubuntu 22.04 (temp) |
| --- | --- | --- |
| Postgres major version | 16 (default repo) | 14 (default repo) |
| Apt package name | `postgresql-16` | `postgresql` (metapackage) |
| Docker | `docker-ce` from Docker's repo (recommended) | `docker-ce` from Docker's repo OR `docker.io` (apt) |
| Solana CLI | unchanged | unchanged |
| Oracle binary | unchanged | unchanged |
| `init.sql` migration | unchanged | unchanged |

The oracle's SQL is portable across Postgres 14, 15, 16, and 17 — it
uses no version-specific features. So **only the apt install command
changes** on 22.04; everything downstream is identical to the canonical
README.

## Bring-up sequence (22.04 substitutions only)

The list below mirrors §"One-time setup" in [`README.md`](README.md);
**only Step 0 differs** from the canonical 24.04 path.

### 0. Host prerequisites — 22.04 substitution

```bash
sudo apt-get update
sudo apt-get install -y postgresql postgresql-client git curl jq
```

This installs Postgres 14 (whatever 22.04 ships) plus the standard
tooling. Verify it came up healthy:

```bash
sudo systemctl status postgresql       # active
sudo -u postgres psql -c '\conninfo'    # confirms server is reachable
psql --version                          # psql (PostgreSQL) 14.x
```

**Docker** is intentionally NOT in the apt one-liner above. Three cases:

- **You already have `docker-ce` installed** (from Docker's official
  repo — check with `dpkg -l | grep docker-ce`). You're done; skip to
  the verify step. This is the recommended state.

- **No Docker yet, want the recommended setup** — install `docker-ce`
  from Docker's official repo:

  ```bash
  sudo install -m 0755 -d /etc/apt/keyrings
  sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg \
      -o /etc/apt/keyrings/docker.asc
  sudo chmod a+r /etc/apt/keyrings/docker.asc
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu jammy stable" \
      | sudo tee /etc/apt/sources.list.d/docker.list
  sudo apt-get update
  sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin
  ```

- **No Docker yet, want the minimal Ubuntu-packaged version** —
  `sudo apt-get install -y docker.io`. This conflicts if you ever
  pulled `containerd.io` from Docker's repo, in which case use the
  `docker-ce` path above instead.

Verify Docker is healthy regardless of path:

```bash
sudo systemctl status docker          # active (running)
sudo docker version                   # client + server both report
sudo docker run --rm hello-world      # smoke test
```

The systemd unit name is `docker.service` for both `docker-ce` and
`docker.io`, so the oracle systemd units (which declare
`After=docker.service`) work unchanged.

If you also want the Solana CLI (required for Step 3 keypair generation
and devnet airdrop):

```bash
sh -c "$(curl -sSfL https://release.anza.xyz/stable/install)"
echo 'export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
solana --version
```

### 1–6. Identical to README.md

Run Steps 1 through 6 from [`README.md`](README.md#one-time-setup)
unchanged. The `createuser` / `createdb` / `psql -f init.sql` / Docker
build / systemd install commands are all Postgres-version-agnostic.

For convenience, the three Postgres-side commands inside Step 2 of the
canonical README are reproduced here so you don't have to flip windows:

```bash
# Inside Step 2 ("Postgres databases (one per cluster)") of the canonical README:

sudo -u postgres createuser oracle_app -P
# At the prompt, set a strong password.

sudo -u postgres createdb -O oracle_app oracle_onchain_transfer_devnet
sudo -u postgres createdb -O oracle_app oracle_onchain_transfer_mainnet

# Apply the schema. Substitute YOUR_PASSWORD with what you just set above.
PGPASSWORD='YOUR_PASSWORD' \
    psql -U oracle_app -h 127.0.0.1 -d oracle_onchain_transfer_devnet \
    -f oracle-common/migrations/init.sql

PGPASSWORD='YOUR_PASSWORD' \
    psql -U oracle_app -h 127.0.0.1 -d oracle_onchain_transfer_mainnet \
    -f oracle-common/migrations/init.sql
```

Sanity check after:

```bash
sudo -u postgres psql -c '\l' | grep oracle_
# oracle_onchain_transfer_devnet  | oracle_app | UTF8 | ...
# oracle_onchain_transfer_mainnet | oracle_app | UTF8 | ...

PGPASSWORD='YOUR_PASSWORD' \
    psql -U oracle_app -h 127.0.0.1 -d oracle_onchain_transfer_devnet \
    -c '\dt oracle_*' | wc -l
# Should report 9 oracle_* tables (plus a header line).
```

## Limitations of validating on 22.04

The 22.04 bring-up validates everything that matters for Wave-1 hardening
— the binary, the schema, the systemd unit, the docker image, the dual-
cluster pattern. What it does **not** validate:

- The exact apt package name on your real 24.04 host (`postgresql-16`).
  When you migrate, run `sudo apt-get install -y postgresql-16` and
  confirm the install succeeds before proceeding.
- Postgres 16-specific features. The migration uses none, so this is
  not a real risk — but worth noting that any future `init.sql` change
  that adopts a Postgres 16 feature must be tested on a 16 install,
  not a 14 install.

## When you migrate to 24.04

1. Run the 24.04 bring-up from scratch using [`README.md`](README.md).
2. `pg_dump` the 22.04 databases if you want to preserve any settled
   verdicts you accumulated during the temp validation:
   ```bash
   PGPASSWORD='...' pg_dump -F c -f /tmp/devnet.dump oracle_onchain_transfer_devnet
   PGPASSWORD='...' pg_dump -F c -f /tmp/mainnet.dump oracle_onchain_transfer_mainnet
   ```
   Restore with `pg_restore` on the 24.04 host. Postgres dumps are
   forward-compatible (14 → 16 works); reverse compatibility is not
   guaranteed and not relevant here.
3. Delete this file. It served its purpose.
