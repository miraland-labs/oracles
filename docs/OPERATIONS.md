# Operations Guide

Day-2 runbook for the x402 oracle workspace. Covers monitoring, incident
playbooks, rotations, backups, audits, failover, and capacity. For initial
deployment, see [`DEPLOYMENT.md`](DEPLOYMENT.md).

This guide assumes oracles installed via
[`../scripts/install.sh`](../scripts/install.sh) on Ubuntu 24.04 with the
templated systemd unit `oracle@<family>.service` and the aggregator
`oracle.target`.

---

## Table of contents

- [Daily routine](#daily-routine)
- [Monitoring](#monitoring)
- [Incident playbooks](#incident-playbooks)
- [Rotations](#rotations)
- [Backup & restore](#backup--restore)
- [Manual `/evaluate`](#manual-evaluate)
- [Audit & compliance](#audit--compliance)
- [Failover](#failover)
- [Capacity & scaling](#capacity--scaling)
- [Upgrades](#upgrades)

---

## Daily routine

1. Skim `oracle.target` status: `systemctl status oracle.target`.
2. Glance at `/health` for each family: `chain_connected`,
   `websocket_connected`, `oracle_balance_lamports`, `queue_depth`.
3. Glance at `/stats` for each family: `total_dead_letter` should be 0;
   `total_errors` rising rapidly is the loudest signal.
4. Tail `journalctl -u oracle@*.service --since '1 day ago' | grep -E
   '(WARN|ERROR)'` and triage anything new.

A 5-minute morning check beats a 30-minute incident triage.

## Monitoring

### Prometheus metrics surfaced at `GET /metrics`

Every family exposes the same counter set in `text/plain; version=0.0.4`:

| Metric                              | Type    | Meaning                                                                |
| ----------------------------------- | ------- | ---------------------------------------------------------------------- |
| `oracle_total_evaluated`            | counter | Cumulative evaluations completed.                                      |
| `oracle_total_approved`             | counter | Cumulative `approved=true` settlements.                                |
| `oracle_total_rejected`             | counter | Cumulative `approved=false` settlements.                               |
| `oracle_total_errors`               | counter | Pipeline errors before settlement.                                     |
| `oracle_total_dead_letter`          | counter | Jobs that exhausted retry budget.                                      |
| `oracle_total_evidence_fetch_failures` | counter | Hash-bound fetch failures (registry mirrors exhausted; post-retry).  |
| `oracle_queue_depth`                | gauge   | Current `monitor → worker` channel depth.                              |
| `oracle_websocket_connected`        | gauge   | 1 = connected, 0 = disconnected.                                       |
| `oracle_deliveries_observed`        | counter | Accepted delivery events since process start.                          |
| `oracle_last_seen_slot`             | gauge   | Highest slot observed in `logsSubscribe`.                              |
| `oracle_uptime_seconds`             | counter | Seconds since process start.                                           |

`oracle_balance_lamports` is exposed via `GET /health` only (sampled
on-demand against the RPC) — it is not in `/metrics`. Scrape `/health`
separately if you want to alert on balance.

### Suggested alert thresholds

| Alert                                              | Severity   | Condition                                               | Why                                                                |
| -------------------------------------------------- | ---------- | ------------------------------------------------------- | ------------------------------------------------------------------ |
| `oracle_websocket_connected == 0`                  | critical   | for ≥ 5 min                                             | Settlements blocked; deliveries pile up.                           |
| `oracle_balance_lamports < 200_000_000` (0.2 SOL)  | warning    | sustained                                               | Settlement TXs will start failing soon.                            |
| `oracle_balance_lamports < 50_000_000`  (0.05 SOL) | critical   | sustained                                               | Active settlement attempts already failing.                        |
| `oracle_total_dead_letter` increase ≥ 1            | critical   | over 1h                                                 | Jobs gave up; manual `/evaluate` or operator action required.      |
| `oracle_total_errors` rate > 5/min                 | warning    | for ≥ 15 min                                            | Something systemic (RPC, registry, DB).                            |
| `oracle_queue_depth > 50`                          | warning    | for ≥ 5 min                                             | Worker is falling behind chain monitor.                            |
| `oracle_queue_depth > 200`                         | critical   | for ≥ 5 min                                             | Channel full; chain monitor will block.                            |
| `up{job="oracle"} == 0`                            | critical   | scrape failed                                           | The process itself is down.                                        |
| `last_websocket_message_at` stale > 60s            | warning    | from `/health`                                          | WS subscribed but RPC node not delivering — common silent failure. |
| `chain_connected == false`                         | critical   | from `/health`                                          | RPC unreachable.                                                   |

### Health probe

`GET /health` returns the JSON shape:

```json
{
  "status": "healthy",                              // "healthy" | "degraded"
  "oracle_pubkey": "OracLe...",
  "program_id": "Escr4...",
  "chain_connected": true,
  "websocket_connected": true,
  "last_websocket_message_at": "2026-05-17T12:34:56Z",
  "queue_depth": 0,
  "deliveries_observed": 42,
  "last_seen_slot": 287654321,
  "registry_reachable": true,
  "oracle_balance_lamports": 1900000000,
  "database_enabled": true,
  "strict_profile": true
}
```

Returns `503` when degraded — wire your load-balancer health check
accordingly so a failing replica is taken out of rotation cleanly.

### Dashboards

A minimal Grafana dashboard renders three panels per family:

1. **Throughput**: `rate(oracle_total_evaluated[5m])`,
   `rate(oracle_total_approved[5m])`, `rate(oracle_total_rejected[5m])`.
2. **Backpressure**: `oracle_queue_depth` overlay
   `oracle_websocket_connected`.
3. **Cost**: `oracle_balance_lamports` (raw + 24h delta).

## Incident playbooks

### A. WebSocket disconnect

**Symptoms**: `oracle_websocket_connected=0`, `last_websocket_message_at`
stale, `deliveries_observed` flatline.

**Diagnosis**:

```bash
sudo journalctl -u oracle@api-quality.service --since '15 minutes ago' | grep -i 'websocket\|reconnect'
```

**Common causes**:

- RPC provider rate-limit triggered.
- RPC node restarted / failed.
- Network firewall closed.

**Resolution**:

1. Confirm RPC reachable: `curl -fsS $SOLANA_WS_URL` (404 is fine — means
   reachable).
2. Switch to a fallback RPC: edit `SOLANA_WS_URL` and `SOLANA_RPC_URL`,
   restart with `sudo systemctl restart oracle@<family>.service`.
3. The chain monitor's startup backfill (`ORACLE_BACKFILL_LOOKBACK_SIGNATURES`)
   recovers any `DeliverySubmittedEvent` missed during the gap. Verify by
   watching `deliveries_observed` rise.

### B. RPC backpressure / 429 storms

**Symptoms**: `oracle_total_errors` rate climbing, settlement attempts
failing with `429 Too Many Requests` in the journal.

**Resolution**:

1. Move to a paid RPC tier (Helius, Triton, Jito, Quicknode).
2. If the spike is `oracle-onchain-transfer`, this hot-path issues a
   `getTransaction(jsonParsed)` per delivery — that family is the most
   sensitive to rate limits.
3. As a quick mitigation, lower `ORACLE_JOB_CHANNEL_CAPACITY` to throttle
   ingestion; the chain monitor will block on a full queue.

### C. Registry outage / fetch failures

**Symptoms**: `oracle_total_evidence_fetch_failures` rising;
`/health.registry_reachable=false`.

**Diagnosis**:

```bash
curl -fsS "$EVIDENCE_REGISTRY_URL/<some-known-sha256>"
```

**Resolution**:

- Set `EVIDENCE_REGISTRY_URLS` to a comma-separated mirror list — the
  oracle tries each in order until one returns `SHA256(body) == hash`.
- If only one mirror exists, add a CDN cache (Cloudflare, Fastly) in
  front; the bytes are content-addressed and immutable, perfect for caching.
- Failed jobs retry per `EVIDENCE_FETCH_MAX_RETRIES` then dead-letter
  after `ORACLE_DEAD_LETTER_MAX_ATTEMPTS`. Recovery: see playbook D.

### D. Dead-letter spike

**Symptoms**: `oracle_total_dead_letter` increased.

**Diagnosis**:

```sql
SELECT payment_uid, status, attempts, last_error, updated_at
  FROM oracle_jobs
 WHERE status = 'dead_letter'
 ORDER BY updated_at DESC
 LIMIT 50;
```

**Resolution**:

1. Group by `last_error` to find the root cause.
2. Once root cause is fixed (RPC restored, registry repaired), kick the
   jobs back into the queue:

   ```sql
   UPDATE oracle_jobs
      SET status = 'detected', attempts = 0, last_error = NULL,
          locked_at = NULL, started_at = NULL, completed_at = NULL,
          updated_at = NOW()
    WHERE status = 'dead_letter'
      AND payment_uid IN ('<uid1>', '<uid2>', ...);
   ```

3. Manually trigger evaluation per payment via
   [`POST /evaluate`](#manual-evaluate); the worker will pick them up on
   the next backfill scan otherwise.

### E. MinIO disk full (`oracle-file-delivery`)

**Symptoms**: registry POST returns `500`; journal shows
`storage backend: aws-sdk-s3: 507 Insufficient Storage`.

**Resolution**:

1. Free disk on the MinIO host or extend the volume.
2. If the bucket is approaching capacity, consider an S3 lifecycle policy
   to expire blobs older than the longest possible escrow window.
3. The oracle's blob fetcher streams — restoring disk space immediately
   re-enables uploads without restarting the binary.

### F. Postgres connection exhaustion

**Symptoms**: `oracle_total_errors` rising; journal shows `pool: timed out
waiting for object`.

**Resolution**:

- The default pool is 8 connections per family (set in `main.rs`). If
  you've front-ended the oracle with a high-volume registration HTTP
  layer, raise the Postgres `max_connections` and/or pgbouncer.
- For a quick mitigation: `sudo systemctl restart oracle@<family>.service`
  releases all connections; the worker resumes from the ledger.

### G. Low oracle SOL balance

**Symptoms**: `oracle_balance_lamports` warning fires; settlements start
failing with `InsufficientFunds`.

**Resolution**:

```bash
solana transfer "$ORACLE_PUBKEY" 5 \
    --from /path/to/funder-keypair.json \
    --url mainnet-beta \
    --allow-unfunded-recipient
```

(Use Devnet airdrop for Devnet.) The settler retries on
`InsufficientFunds` so funded settlements catch up automatically once the
balance is restored.

### H. `oracle_lifecycle_events` log shows malformed SLA → repeating

**Symptoms**: same `payment_uid` cycling through `detected → failed →
detected → failed`.

**Resolution**: this is by design — the on-chain job exists, the SLA
bytes are bad. The worker dead-letters after
`ORACLE_DEAD_LETTER_MAX_ATTEMPTS`. Sellers must re-upload a corrected
SLA, register a new payment, or wait for the on-chain expiry. Operator
intervention is not appropriate.

## Rotations

### Bearer tokens (sellers)

```bash
curl -fsS -X POST "https://oracle-api.example.com/v1/registry/seller/rotate" \
    -H "Authorization: Bearer $OLD_BEARER" | jq .
# Server revokes the old token and returns a new one in the same response.
```

The seller must capture the new token and update their build pipeline.
The oracle stores only `SHA256(token)` so there's no way to recover the
old one.

### Operator token (`POST /evaluate`)

1. Generate a new token: `openssl rand -hex 32`.
2. Compute its hash: `echo -n "$NEW_TOKEN" | sha256sum`.
3. Update `/etc/oracle/<family>.env`:
   `ORACLE_OPERATOR_TOKEN_SHA256=<new-hex>`.
4. Restart: `sudo systemctl restart oracle@<family>.service`.
5. Distribute the raw token to operators via your secret-management tool.

### Oracle keypair (highest-stakes rotation)

The oracle's `oracle_authority` is committed at FundPayment time, so
rotation is **forward-only** — old payments still settle to the old key.

1. Generate the new keypair (§5.1 in [`DEPLOYMENT.md`](DEPLOYMENT.md)).
2. Fund it.
3. Update `pr402` capability advertisement to expose the **new** pubkey
   for new payments. (See
   [`oracle-common/docs/PR402_CONTRACT.md`](../oracle-common/docs/PR402_CONTRACT.md).)
4. Wait for in-flight payments bound to the old key to drain (their
   `expires_at` is the longest interval).
5. Stop the binary, swap `ORACLE_KEYPAIR_PATH`, restart.
6. Decommission the old key (move to cold backup, do not delete).

The simplest rollout: stand up a second binary on a new port with the
new key, drain traffic by switching pr402 advertisement, decom the old
binary once its ledger reports zero in-flight jobs.

### RPC endpoint

Edit `SOLANA_RPC_URL` and `SOLANA_WS_URL`, restart. The startup backfill
catches any deliveries observed by neither endpoint during the brief
restart window.

## Backup & restore

### Postgres

Per family:

```bash
PGPASSWORD='...' pg_dump -U oracle_app -h db.internal -F c \
    -f /backup/oracle_api_quality_$(date +%F).dump \
    oracle_api_quality
```

Cron nightly; retain ≥ 30 days; offsite copy via your backup tool.

**Restore**:

```bash
PGPASSWORD='...' pg_restore -U oracle_app -h db.internal \
    -d oracle_api_quality --clean --if-exists \
    /backup/oracle_api_quality_2026-05-16.dump
```

After restore, restart the oracle binary so the worker re-syncs its
in-memory dedupe `HashSet` from `is_terminal()`.

### MinIO

For a single-node deployment:

```bash
mc mirror oracle/oracle-blobs s3-backup/oracle-blobs-$(date +%F)
```

For distributed MinIO, use built-in `mc replicate` to a secondary cluster
in another region.

### Oracle keypair

Three copies (§5.3 in [`DEPLOYMENT.md`](DEPLOYMENT.md)). Verify
quarterly that the offline copy still decrypts and round-trips through
`solana-keygen pubkey ...`.

### Configuration files

`/etc/oracle/*.env` are not in version control by design (they hold
secrets). Back them up to your secrets manager (HashiCorp Vault, AWS
Secrets Manager, or sealed-secret in your gitops repo).

## Manual `/evaluate`

Use only for incident response (replaying a failed job after fixing
infra) or for spot-checking a specific payment. Production deployments
**must** require `ORACLE_OPERATOR_TOKEN_SHA256`.

```bash
curl -fsS -X POST "https://oracle-api.example.com/evaluate" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $OPERATOR_TOKEN" \
    -d '{"payment_pubkey":"PayMeNt..."}' | jq .
```

Behavior:

- `404` if the payment isn't assigned to this oracle authority (correct
  fail-closed behavior).
- `429` if the rate limit is exceeded
  (`ORACLE_MANUAL_EVALUATE_RATE_LIMIT` per `..._WINDOW_MS`).
- `200` with the verdict JSON otherwise.

Every manual call is recorded in `oracle_lifecycle_events` with
`event='manual_evaluate'` and the operator token hash in the payload.

## Audit & compliance

### Reconstructing a verdict

For any settled payment, the full audit trail lives in three tables:

```sql
-- Job state at settlement time
SELECT payment_uid, mint, amount, sla_hash, delivery_hash,
       oracle_authority, profile_id, status, settlement_signature,
       resolution_hash, started_at, completed_at
  FROM oracle_jobs
 WHERE payment_uid = '<hex>';

-- The verdict itself
SELECT approved, resolution_reason, resolution_hash,
       checks::jsonb,        -- per-check pass/fail
       registry_sources,     -- which mirror returned which artifact
       settlement_signature,
       created_at
  FROM oracle_verdicts
 WHERE oracle_job_id = (SELECT id FROM oracle_jobs WHERE payment_uid = '<hex>');

-- The append-only event log
SELECT event, payload::jsonb, created_at
  FROM oracle_lifecycle_events
 WHERE payment_uid = '<hex>'
 ORDER BY created_at;
```

The chain commits only `resolution_hash`. Counterparties can recompute it
from the ledger row to verify the verdict is consistent with the
on-chain commitment — that's what `cross_family_properties.rs` proves
deterministically.

### Counterparty verification recipe

Anyone (buyer, seller, third-party auditor) holding the SLA + delivery
bytes plus the `oracle_jobs` row can independently verify the verdict:

1. Verify `SHA256(sla_bytes) == job.sla_hash` (committed on-chain).
2. Verify `SHA256(delivery_bytes) == job.delivery_hash` (committed
   on-chain).
3. Re-run the same evaluator against the SLA + delivery (the spec at
   `oracle-*/spec/*/NORMATIVE.md` is normative for v1; identical inputs
   must produce an identical `approved` + `resolution_reason`).
4. Recompute `compute_resolution_hash(...)` using the canonical
   `x402/oracles/resolution-envelope/v1` recipe documented in
   [`design.md`](../../.kiro/specs/multi-category-oracle-architecture/design.md);
   confirm it equals `verdict.resolution_hash`.

If all four pass, the verdict is reproducible — the oracle has no hidden
state.

### Retention

Postgres retention is your call. Recommended:

- `oracle_jobs` + `oracle_verdicts`: keep indefinitely (audit primary).
- `oracle_lifecycle_events`: ≥ 1 year for ops; archive older to cold
  storage if disk pressure.
- `oracle_artifacts` (Postgres backend only): purge bytes for settled
  jobs older than your longest escrow expiry, keeping rows for the audit.

## Failover

The architecture is **single-writer per family**: exactly one binary
holds the oracle keypair and writes settlements at any moment. Running
two binaries with the same key races on-chain (the program accepts
exactly one settlement per `payment_uid`; the second loses).

Recommended pattern:

1. **Active**: full deployment, keypair on disk, `oracle@<family>.service`
   running.
2. **Standby**: identical host, `oracle@<family>.service` **stopped**,
   keypair file present but mode 0000 (unreadable to the oracle user) or
   stored offline.
3. **Failover trigger**: active host unhealthy.
4. **Procedure**:

   ```bash
   # On active (if reachable):
   sudo systemctl stop oracle@api-quality.service

   # On standby:
   sudo chmod 600 /var/lib/oracle/api-quality/oracle-keypair.json
   sudo systemctl start oracle@api-quality.service
   sudo journalctl -u oracle@api-quality.service -f
   ```

5. Confirm `/health` returns `healthy`; verify the standby's
   `last_seen_slot` advances.
6. Re-image the failed host or fix the root cause; promote standby to
   active and provision a new standby.

The Postgres ledger is shared (single source of truth) so the standby
boots with the same `oracle_jobs` view. The startup backfill recovers
any deliveries that landed during the cutover.

## Capacity & scaling

### Vertical (more CPU/RAM on the same host)

Rarely needed. The oracle is I/O-bound (RPC, registry fetch, Postgres) not
compute-bound. Two cases for a vertical bump:

- `oracle-file-delivery` with many concurrent large blobs — bumping RAM
  helps the SHA-256 + reqwest streaming.
- `oracle-onchain-transfer` with a high-throughput cluster (Mainnet) —
  more cores let you parallelize `getTransaction(jsonParsed)` calls.

### Horizontal (multiple oracles per family)

The single-writer-per-keypair constraint means horizontal scaling means
**multiple keypairs**. Sellers advertise multiple oracles (different
`operatorPubkey`); buyers pick one. Each oracle runs independently with
its own DB.

This is the right scaling pattern for high-volume profiles: deploy `N`
binaries with `N` keypairs, advertise all of them, let buyers' selection
algorithm load-balance.

### Triggers

| Signal                                    | Action                                                |
| ----------------------------------------- | ----------------------------------------------------- |
| `oracle_queue_depth > 50` p95             | Investigate evaluator latency.                        |
| Settlement P50 > 5s                       | Switch to a faster RPC tier.                          |
| Evidence-fetch P50 > 2s                   | Add a CDN in front of the registry.                   |
| Postgres CPU > 70% sustained              | Migrate to a beefier instance or split per-family.   |
| MinIO ingress > 50% line-rate             | Distribute MinIO; add CDN for read.                  |

## Upgrades

Use `upgrade.sh` for a routine binary swap:

```bash
cd oracles
cargo build --release -p oracle-api-quality

sudo ./scripts/upgrade.sh \
    api-quality \
    ./target/release/oracle-api-quality
```

The script:

1. Captures the running binary as `oracle-<family>.bak.<UTC-timestamp>`.
2. Stages the new binary as `oracle-<family>.new` next to it.
3. Atomic-renames the staged binary into place.
4. Restarts the service.
5. Probes `/health` 5×2s.
6. Healthy → prunes old `.bak.*` files, keeping the newest `KEEP_BACKUPS=5`
   (configurable via env). Unhealthy → leaves the binary in place and exits
   non-zero with a manual rollback command in stderr.

For unattended deploys (CI / blue-green), pass `--auto-rollback` (or set
`AUTO_ROLLBACK=1`) so a failed health probe restores the most recent backup
and restarts automatically:

```bash
sudo ./scripts/upgrade.sh api-quality \
    ./target/release/oracle-api-quality --auto-rollback
```

Exit codes:

| Code | Meaning                                                   |
| ---- | --------------------------------------------------------- |
| 0    | Upgrade succeeded; old backups pruned per `KEEP_BACKUPS`. |
| 1    | Upgrade unhealthy; manual rollback required.              |
| 2    | Auto-rollback completed; old binary restored.             |
| 3    | Auto-rollback FAILED; manual intervention required.       |

> **Operational note.** Backups are kept at
> `/opt/oracle/<family>/oracle-<family>.bak.<UTC-ts>`. Tune retention with
> `KEEP_BACKUPS` (default 5) or set `KEEP_BACKUPS=0` to keep all backups.

Migration upgrades (schema changes) are out of scope for this workspace
in v1 — the schema is frozen and `init.sql` is idempotent. Future
breaking changes will ship a versioned `migrations/<timestamp>__*.sql`
companion.

### Pre-upgrade checklist

- [ ] `cargo test --workspace` passes locally on the upgrade source.
- [ ] `oracle-common/docs/PR402_CONTRACT.md` unchanged (or change
      announced to seller integrations).
- [ ] Ledger backup in the last 24h (§Backup & restore).
- [ ] Standby host on the same binary version (so failover is symmetric).

### Post-upgrade verification

- [ ] `/health` returns `healthy` within 30 s.
- [ ] One successful settlement in the journal (or a manual `/evaluate`
      smoke test).
- [ ] No new `ERROR` lines in `journalctl --since '5 minutes ago'`.
- [ ] `cargo test --workspace --no-default-features` ledger schema
      compatibility check (against your real DB) is green.

If anything fails, **rollback** (`### Rollback`):

```bash
# Latest backup is at /opt/oracle/<family>/oracle-<family>.bak.<UTC-ts>
LATEST_BACKUP=$(ls -1t /opt/oracle/api-quality/oracle-api-quality.bak.* | head -n 1)
sudo systemctl stop oracle@api-quality.service
sudo cp -p "$LATEST_BACKUP" /opt/oracle/api-quality/oracle-api-quality
sudo chown oracle:oracle /opt/oracle/api-quality/oracle-api-quality
sudo systemctl start oracle@api-quality.service
```

Or skip the manual restore and use the auto-rollback flag on the next
upgrade attempt: `sudo ./scripts/upgrade.sh api-quality <path-to-known-good>
--auto-rollback`.
