# oracle-api-quality

JSON-shaped SLA quality oracle for the x402/pr402 ecosystem. Implements the
[`x402/oracle/api-quality/v1`](spec/api-quality-v1/NORMATIVE.md) profile: status code,
latency, required-fields, JSON Schema, and body-length checks against a JSON
delivery payload.

> **Are you a seller integrating with this oracle?** This README is the
> operator-facing doc (run, deploy, observe). Sellers should read
> [`oracles/docs/SELLER_GUIDE.md`](../docs/SELLER_GUIDE.md) instead — three
> copy-paste recipes, one per delivery scenario.
>
> **Are you a buyer paying for an SLA-escrow service?** Read
> [`oracles/docs/BUYER_GUIDE.md`](../docs/BUYER_GUIDE.md) — how to pick an
> oracle and fund the escrow via pr402.

This is one of three sibling oracles in the `oracles/` workspace. It shares
chain-monitoring, registration, settlement, and Postgres ledger code with
`oracle-onchain-transfer` and `oracle-file-delivery` via the `oracle-common`
library. The three binaries are independently deployable; each holds its own
oracle keypair and Postgres database for blast-radius isolation.

## Quick start (development)

```bash
cd oracles
cp oracle-api-quality/.env.example /tmp/oracle-api-quality.env
# edit /tmp/oracle-api-quality.env: ESCROW_PROGRAM_ID, ORACLE_KEYPAIR_PATH,
# DATABASE_URL, EVIDENCE_REGISTRY_URL, ORACLE_REGISTRY_BACKEND.
psql "$DATABASE_URL" < oracle-common/migrations/init.sql
env $(grep -v '^#' /tmp/oracle-api-quality.env | xargs) \
    cargo run --release -p oracle-api-quality
```

The binary listens on `BIND_ADDR` (default `0.0.0.0:4020`) and exposes:

| Path                            | Purpose                                            |
| ------------------------------- | -------------------------------------------------- |
| `GET /`                         | Service banner                                     |
| `GET /health`                   | Health probe (`200`/`503` based on chain + ws)     |
| `GET /stats`                    | JSON counters (`OracleStats`)                      |
| `GET /metrics`                  | Prometheus text exposition                         |
| `POST /evaluate`                | Operator-only manual evaluation (bearer + rate)    |
| `POST /v1/registry/sla`         | Seller upload — SLA JSON                           |
| `POST /v1/registry/delivery`    | Seller upload — delivery JSON                      |
| `POST /v1/registry/blob`        | Seller upload — opaque blob (streamed)             |
| `GET  /v1/registry/{sha256}`    | Hash-verified fetch                                |
| `HEAD /v1/registry/{sha256}`    | Stat                                               |

## Production install (Ubuntu 24.04)

```bash
sudo bash oracles/scripts/install.sh \
    api-quality \
    https://github.com/miraland-labs/x402/releases/download/oracle-api-quality-vX/oracle-api-quality \
    /tmp/oracle-api-quality.env
sudo systemctl status oracle@api-quality
```

The installer creates the `oracle` system user, copies the binary to
`/usr/local/bin/oracle-api-quality`, drops the env file at
`/etc/oracle/api-quality.env` (mode 0600), and enables
`oracle@api-quality.service`. See [`oracles/scripts/README.md`](../scripts/README.md)
for the full smoke-test runbook.

## Devnet runbook

End-to-end Devnet validation lives at
[`tests/devnet/api_quality_v1.sh`](tests/devnet/api_quality_v1.sh). It exercises
the full path: seller registers, uploads SLA + delivery, buyer funds the
escrow, seller submits delivery, oracle evaluates and settles, ledger row
transitions to `settled`.

### Prerequisites

- `solana-keygen` / `solana` CLI 2.x with a keypair funded with Devnet SOL
- `jq`, `curl`, `shasum` available
- `DATABASE_URL` pointing at a Postgres reachable by the runbook host with
  `oracle-common/migrations/init.sql` already applied
- The `oracle-api-quality` binary running locally or via systemd (default port
  `:4020`)
- A buyer + seller wallet with Devnet USDC; the seller registered against
  `POST /v1/registry/seller/register`
- `ORACLE_HOST` (default `http://127.0.0.1:4020`), `SELLER_TOKEN`, `PAYMENT_UID`
  exported

### Operational notes

- Running two binaries with the same oracle keypair is unsupported: the
  on-chain program accepts exactly one settlement per `payment_uid`; the second
  request loses the race.
- `ORACLE_STRICT_PROFILE=true` (default) makes the evaluator validate the
  parsed SLA against the JSON Schema — a malformed SLA fails the job in the
  evaluator rather than at deserialize time, surfacing a clearer error in
  `oracle_jobs.last_error`.
- Watch logs with `journalctl -u oracle@api-quality -f` while the runbook
  drives traffic; key events have `target=oracle_api_quality` or
  `target=oracle_common::*`.

## Specification

- [`spec/api-quality-v1/NORMATIVE.md`](spec/api-quality-v1/NORMATIVE.md)
- [`spec/api-quality-v1/schema/sla-document.schema.json`](spec/api-quality-v1/schema/sla-document.schema.json)
- [`spec/api-quality-v1/schema/delivery-evidence.schema.json`](spec/api-quality-v1/schema/delivery-evidence.schema.json)
- [`spec/api-quality-v1/examples/`](spec/api-quality-v1/examples/) — three
  example pairs (approve, status-rejected, schema-rejected) used by the
  workspace `spec_lint` test.

## Resolution-reason codes

This binary emits codes in `[0, 5]` and `255`:

| Code | Meaning                       |
| ---- | ----------------------------- |
| 0    | Approved                      |
| 1    | StatusCode                    |
| 2    | Latency                       |
| 3    | MissingRequiredFields         |
| 4    | SchemaValidation              |
| 5    | BodyLength                    |
| 255  | Unspecified / generic failure |

Custom-code ranges `[256, 319]` and `[320, 383]` are reserved for the
onchain-transfer and file-delivery families respectively.
