# Operations Guide

Day-2 runbook for the oracles workspace. Two paths:

- **Daily routine + incident triage** (the cheat sheet) — §1, §2, §3.
- **Reference** (rotations, backup, audit, failover, capacity, upgrades) — §4 onward.

For initial deployment see [`DEPLOYMENT.md`](DEPLOYMENT.md).

---

## 1. Daily routine

A 5-minute morning check:

```bash
systemctl status oracle.target              # all units active?
curl -fsS http://127.0.0.1:4020/health \    # repeat per family port
    | jq '{status, chain_connected, websocket_connected, queue_depth, oracle_balance_lamports}'
journalctl -u oracle@*.service --since '1 day ago' | grep -E '(WARN|ERROR)' | tail
```

If those three return clean output, you're done. If anything looks off,
go to §3.

---

## 2. Monitoring

### What to scrape

`GET /metrics` (Prometheus text format):

| Metric | Type | Meaning |
| --- | --- | --- |
| `oracle_total_evaluated` | counter | Cumulative evaluations completed. |
| `oracle_total_approved` / `_rejected` | counter | Cumulative verdict counts. |
| `oracle_total_errors` | counter | Pipeline errors before settlement. |
| `oracle_total_dead_letter` | counter | Jobs that exhausted retry budget. |
| `oracle_total_evidence_fetch_failures` | counter | Hash-bound fetch failures (post-retry). |
| `oracle_queue_depth` | gauge | Current monitor → worker channel depth. |
| `oracle_websocket_connected` | gauge | 1 = connected, 0 = disconnected. |
| `oracle_deliveries_observed` | counter | Accepted delivery events since process start. |
| `oracle_last_seen_slot` | gauge | Highest slot observed. |
| `oracle_uptime_seconds` | counter | Seconds since process start. |

`oracle_balance_lamports` lives on `/health` (RPC-sampled, not in
`/metrics`). Scrape `/health` separately if you want to alert on
balance.

### Alert thresholds

| Alert | Severity | Condition |
| --- | --- | --- |
| `oracle_websocket_connected == 0` | critical | for ≥ 5 min |
| `oracle_balance_lamports < 0.2 SOL` | warning | sustained |
| `oracle_balance_lamports < 0.05 SOL` | critical | sustained |
| `oracle_total_dead_letter` increase ≥ 1 | critical | per hour |
| `oracle_total_errors` rate > 5/min | warning | for ≥ 15 min |
| `oracle_queue_depth > 50` | warning | for ≥ 5 min |
| `oracle_queue_depth > 200` | critical | for ≥ 5 min |
| `up{job="oracle"} == 0` | critical | scrape failed |
| `last_websocket_message_at` stale > 60 s | warning | from `/health` |
| `chain_connected == false` | critical | from `/health` |

### Health probe shape

```json
{
  "status": "healthy",
  "oracle_pubkey": "...",
  "program_id": "...",
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

Returns HTTP 503 when degraded — wire your load-balancer health check
accordingly.

---

## 3. Incident triage

Look up the symptom; do the action. Most rows are oracle-specific. For
generic Solana / Postgres / RPC issues, your team's playbooks already
cover them.

| Symptom | Likely cause | Action |
| --- | --- | --- |
| `oracle_websocket_connected = 0`, `deliveries_observed` flat | RPC dropped subscription | Confirm RPC reachable, swap `SOLANA_WS_URL`, restart. Backfill recovers missed events on boot. |
| `oracle_total_evidence_fetch_failures` rising; `/health.registry_reachable=false` | Registry mirror down | Add a mirror to `EVIDENCE_REGISTRY_URLS` (comma-list); add a CDN in front; the bytes are content-addressed, perfect for caching. |
| `oracle_total_dead_letter` increased | Jobs gave up after retries | See §3.1 dead-letter recovery. |
| `oracle_balance_lamports` low | Settlement fees exhausted SOL | `solana transfer $ORACLE_PUBKEY ...`. Settler retries on `InsufficientFunds`; pending settles catch up automatically. |
| MinIO POST returns 500 (`507 Insufficient Storage`) | Disk full | Free disk or extend volume; upload resumes without restart. Add an S3 lifecycle policy to expire blobs older than longest escrow window. |
| `pool: timed out waiting for object` | Postgres pool exhausted | `systemctl restart oracle@<family>.service` releases connections; raise Postgres `max_connections` if you front the oracle with a high-volume registration HTTP layer. |
| Same `payment_uid` cycling `detected → failed` | Malformed SLA bytes; seller error | By design — dead-letters after `ORACLE_DEAD_LETTER_MAX_ATTEMPTS`. Operator does not intervene. |
| `oracle_total_errors` rate climbing, journal shows `429 Too Many Requests` | RPC rate-limit | Move to a paid RPC tier. `oracle-onchain-transfer` is the most sensitive (one `getTransaction(jsonParsed)` per delivery). |

### 3.1 Dead-letter recovery

Group by error to find root cause:

```sql
SELECT last_error, count(*)
  FROM oracle_jobs
 WHERE status = 'dead_letter'
 GROUP BY last_error;
```

Once root cause is fixed, kick failed jobs back into the queue:

```sql
UPDATE oracle_jobs
   SET status='detected', attempts=0, last_error=NULL,
       locked_at=NULL, started_at=NULL, completed_at=NULL,
       updated_at=NOW()
 WHERE status='dead_letter'
   AND payment_uid IN ('<uid1>', '<uid2>');
```

Or trigger a single payment via `POST /evaluate` (§4.3).

---

## 4. Reference

### 4.1 Rotations

**Bearer tokens (sellers)** — `POST /v1/registry/seller/rotate` with the
old bearer; the response carries the new one. The old is revoked
atomically.

**Operator token** — generate (`openssl rand -hex 32`), hash
(`sha256sum`), update `ORACLE_OPERATOR_TOKEN_SHA256` in the env file,
restart, distribute the raw token via your secret manager.

**Oracle keypair** (highest stakes; forward-only because in-flight
payments are bound to the old key on-chain) —

1. Generate + fund a new keypair.
2. Update pr402 advertisement to expose the new pubkey.
3. Wait for in-flight payments on the old key to drain (longest
   `expires_at` is your bound).
4. Stop the binary, swap `ORACLE_KEYPAIR_PATH`, restart.
5. Move the old key to cold backup. Don't delete.

The simplest live rollout: stand up a second binary on a new port with
the new key, drain by switching pr402 advertisement, decom the old
binary once its `oracle_jobs` reports zero in-flight.

**RPC endpoint** — edit `SOLANA_RPC_URL` / `SOLANA_WS_URL`, restart.
Backfill recovers events from the brief gap.

### 4.2 Backup & restore

**Postgres**:

```bash
PGPASSWORD='...' pg_dump -F c -f /backup/oracle_<family>_$(date +%F).dump <database>
PGPASSWORD='...' pg_restore --clean --if-exists -d <database> /backup/<file>.dump
```

After restore, restart the oracle binary so the in-memory dedupe set
re-syncs from `is_terminal()`. Cron nightly, retain ≥ 30 days.

**MinIO** — single-node: `mc mirror oracle/oracle-blobs s3-backup/...`.
Distributed: built-in `mc replicate` to a secondary cluster.

**Oracle keypair** — three copies (hot on disk, warm encrypted offline,
cold paper). Verify the offline copy quarterly with `solana-keygen
pubkey ...`.

**Configuration** — `/etc/oracle/*.env` holds secrets; back up to your
secrets manager (Vault / AWS Secrets Manager / sealed-secrets).

### 4.3 Manual `/evaluate`

For incident response (replay after fixing infra) or spot-checking a
specific payment. Production deployments must require
`ORACLE_OPERATOR_TOKEN_SHA256`.

```bash
curl -fsS -X POST "https://oracle-api.example.com/evaluate" \
    -H "Authorization: Bearer $OPERATOR_TOKEN" \
    -H "Content-Type: application/json" \
    -d '{"payment_pubkey":"..."}' | jq .
```

- 404 = payment not assigned to this oracle authority (correct
  fail-closed).
- 429 = rate-limited by `ORACLE_MANUAL_EVALUATE_RATE_LIMIT` per
  `..._WINDOW_MS` (default 30 / 60 000 ms).
- 200 = verdict JSON.

Every call is logged in `oracle_lifecycle_events` with
`event='manual_evaluate'`.

### 4.4 Audit & verification

The chain commits only `resolution_hash`. Anyone holding SLA + delivery
bytes plus the `oracle_jobs` row can independently recompute and verify
the verdict; the recipe lives in
[`SLA_ESCROW_PROTOCOL.md` §5](SLA_ESCROW_PROTOCOL.md#5-trust-boundaries).

For a regulator-style audit query, the three relevant tables are
`oracle_jobs` (job state), `oracle_verdicts` (the verdict + per-check
detail), and `oracle_lifecycle_events` (append-only audit log). Join on
`payment_uid`. See [the schema](../oracle-common/migrations/init.sql) for
column definitions.

Suggested retention: `oracle_jobs` and `oracle_verdicts` indefinitely.
`oracle_lifecycle_events` ≥ 1 year for ops; archive older to cold
storage if disk pressure. `oracle_artifacts` (Postgres backend only):
purge bytes for settled jobs older than your longest escrow expiry,
keeping rows for the audit.

### 4.5 Failover

Single-writer per family: exactly one binary holds the keypair and
writes settlements at any moment. Two binaries with the same key race
on-chain and one loses.

Pattern: active host runs the service; standby has the keypair file
present but mode 0000 (unreadable) and the unit stopped. Failover:

```bash
# active: stop
sudo systemctl stop oracle@<family>.service
# standby: enable + start
sudo chmod 600 /var/lib/oracle/<family>/oracle-keypair.json
sudo systemctl start oracle@<family>.service
```

Postgres is shared (single source of truth); the standby boots with the
same `oracle_jobs` view. Backfill recovers any deliveries that landed
during the cutover.

### 4.6 Capacity

The oracle is I/O-bound, not compute-bound. When to act:

| Signal | Action |
| --- | --- |
| `queue_depth` p95 > 50 | Investigate evaluator latency. |
| Settlement P50 > 5 s | Faster RPC tier. |
| Evidence-fetch P50 > 2 s | Add a CDN in front of the registry. |
| Postgres CPU > 70% sustained | Beefier instance, or split per-family. |
| MinIO ingress > 50% line-rate | Distribute MinIO; add CDN for read. |

Horizontal scaling means **multiple keypairs** (sellers advertise
several oracles, buyers pick one). Each oracle runs independently with
its own DB. This is the right pattern for high-volume profiles; vertical
scaling rarely is.

### 4.7 Upgrades

```bash
cargo build --release -p oracle-<family>
sudo ./scripts/upgrade.sh <family> ./target/release/oracle-<family>
```

The script captures the running binary, stages the new one,
atomic-renames into place, restarts, and probes `/health` 5×2 s. Healthy
prunes old `.bak.*` (keeping `KEEP_BACKUPS=5`); unhealthy leaves the
binary in place and exits non-zero with a manual rollback hint.

For unattended deploys: `--auto-rollback` (or `AUTO_ROLLBACK=1`)
restores the most recent backup and restarts on health failure.

Exit codes: 0 = ok, 1 = unhealthy (manual rollback), 2 = auto-rollback
done, 3 = auto-rollback failed.

Migration upgrades (schema changes) are out of scope in v1 — the schema
is frozen and `init.sql` is idempotent. Future breaking changes will
ship a versioned `migrations/<timestamp>__*.sql` companion.

**Pre-upgrade**: tests pass on source; ledger backed up in last 24h;
standby on same version.

**Post-upgrade**: `/health` healthy within 30 s; one successful
settlement (or a manual `/evaluate` smoke test); no new ERROR lines.

**Manual rollback**:

```bash
LATEST=$(ls -1t /opt/oracle/<family>/oracle-<family>.bak.* | head -n 1)
sudo systemctl stop oracle@<family>.service
sudo cp -p "$LATEST" /opt/oracle/<family>/oracle-<family>
sudo systemctl start oracle@<family>.service
```
