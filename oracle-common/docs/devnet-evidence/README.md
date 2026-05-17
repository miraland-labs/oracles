# Devnet Evidence

This directory holds artifacts captured during the Phase D Final Integration
Milestone (Task 23 in the
[multi-category-oracle-architecture spec](../../../.kiro/specs/multi-category-oracle-architecture/tasks.md)).
Each subdirectory corresponds to one end-to-end run on a fresh Ubuntu 24.04
VPS with all three sibling oracles installed via `oracles/scripts/install.sh`
and a MinIO instance bootstrapped via `oracles/scripts/bootstrap-minio.sh`.

## Required artifacts per run

The runbooks at `oracle-*/tests/devnet/*.sh` produce the following evidence;
copy them into a dated subdirectory here for the project log:

| Filename                    | Source                                           |
| --------------------------- | ------------------------------------------------ |
| `00-host-info.txt`          | `uname -a; lsb_release -a; rustc -V; psql -V`    |
| `01-install.log`            | `journalctl -u oracle@*` during install          |
| `02-bootstrap-minio.log`    | `bash bootstrap-minio.sh 2>&1 \| tee ...`        |
| `03-api-quality-flow.log`   | terminal transcript of `api_quality_v1.sh`       |
| `04-onchain-transfer-flow.log` | terminal transcript of `transfer_v1.sh`       |
| `05-file-delivery-flow.log` | terminal transcript of `file_v1.sh`              |
| `06-oracle-jobs.tsv`        | `psql ... -c "SELECT * FROM oracle_jobs ..."`    |
| `07-oracle-verdicts.tsv`    | `psql ... -c "SELECT * FROM oracle_verdicts..."` |
| `08-target-status.txt`      | `systemctl status oracle.target`                 |
| `09-health.json`            | `curl -fsS http://127.0.0.1:4020/health \| jq .` |

## Layout

```
devnet-evidence/
├── README.md                 ← this file
├── 2026-05-DD-fresh-install/
│   ├── 00-host-info.txt
│   ├── 01-install.log
│   ├── ...
│   └── 09-health.json
└── ...
```

## Status

Evidence directories are populated when each milestone-23 run is performed
on real hardware. The directory is empty in the source tree so contributors
can reproduce the runbooks without conflicting with prior runs.
