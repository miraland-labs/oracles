# oracle-onchain-transfer

On-chain transfer / swap quality oracle for the x402/pr402 ecosystem.
Implements the [`x402/oracles/onchain-transfer/v1`](spec/onchain-transfer-v1/NORMATIVE.md)
profile: re-derives token deltas from `getTransaction(jsonParsed)` and
approves only when every `expected_transfers[]` entry is satisfied with
`direction`, `mint`, `recipient_owner`, and `min_amount` matching.

> **Are you a seller integrating with this oracle?** This README is the
> operator-facing doc (run, deploy, observe). Sellers should read
> [`oracles/docs/SELLER_GUIDE.md`](../docs/SELLER_GUIDE.md) — see §4.B
> for the on-chain-transfer recipe.
>
> **Are you a buyer paying for an SLA-escrow service?** Read
> [`oracles/docs/BUYER_GUIDE.md`](../docs/BUYER_GUIDE.md) — how to pick an
> oracle and fund the escrow via pr402.

This is one of three sibling oracles in the `oracles/` workspace; see the
top-level [`oracles/README`](../) (or any of the other crates' READMEs) for
shared context.

## Quick start (development)

```bash
cd oracles
cp oracle-onchain-transfer/.env.example /tmp/oracle-onchain-transfer.env
# edit /tmp/oracle-onchain-transfer.env: ESCROW_PROGRAM_ID, ORACLE_KEYPAIR_PATH,
# DATABASE_URL, TRANSFER_CLUSTER (mainnet|devnet|testnet|custom).
psql "$DATABASE_URL" < oracle-common/migrations/init.sql
env $(grep -v '^#' /tmp/oracle-onchain-transfer.env | xargs) \
    cargo run --release -p oracle-onchain-transfer
```

Default port: `:4021`. HTTP surface is identical to the api-quality binary
(see its README); the per-family difference is the evaluator wired in
`main.rs`.

## Production install (Ubuntu 24.04)

```bash
sudo bash oracles/scripts/install.sh \
    onchain-transfer \
    https://github.com/miraland-labs/oracles/releases/download/oracle-onchain-transfer-vX/oracle-onchain-transfer \
    /tmp/oracle-onchain-transfer.env
sudo systemctl status oracle@onchain-transfer
```

## Devnet runbook

[`tests/devnet/transfer_v1.sh`](tests/devnet/transfer_v1.sh) drives the
full path: seller broadcasts a real `TransferChecked` of Devnet USDC, posts
the resulting signature in the evidence JSON, buyer funds the escrow, seller
submits delivery, oracle calls `getTransaction(sig, jsonParsed)`, re-derives
deltas, settles.

### Prerequisites

- `solana-keygen` / `solana` CLI 2.x; an oracle keypair funded with Devnet SOL
- `jq`, `curl`, `shasum`
- `DATABASE_URL` Postgres with migrations applied
- The `oracle-onchain-transfer` binary running (default port `:4021`)
- A buyer wallet with at least one ATA created for the expected mint
- A seller wallet that holds the tokens being transferred and is registered
  against the registry
- Exported: `ORACLE_HOST`, `BUYER_PUBKEY`, `TX_SIGNATURE` (set by the seller
  step), `DEVNET_USDC_MINT` (default
  `Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB`)
- `TRANSFER_CLUSTER=devnet` in the env file (the evaluator refuses to verify
  signatures from a different cluster)

### Operational notes

- Per [design.md C7](../../.kiro/specs/multi-category-oracle-architecture/design.md),
  one `oracle-onchain-transfer` binary serves exactly one cluster. To
  evaluate transfers on multiple clusters, run multiple binaries with
  different `TRANSFER_CLUSTER` and different oracle keypairs.
- Running two binaries with the same oracle keypair is unsupported (race).
- The evaluator is fully pure — `verify_observed_transfer(sla, observation)`
  is unit-tested without an RPC. Production failures usually trace to one of
  the eight `Custom(256..=263)` codes; check `oracle_verdicts.resolution_reason`
  for the diagnosis.
- `getTransaction` retries: transient RPC errors trigger retry per
  `EVIDENCE_FETCH_MAX_RETRIES`; persistent failures dead-letter the job after
  `ORACLE_DEAD_LETTER_MAX_ATTEMPTS`.

## Specification

- [`spec/onchain-transfer-v1/NORMATIVE.md`](spec/onchain-transfer-v1/NORMATIVE.md)
- [`spec/onchain-transfer-v1/schema/sla-document.schema.json`](spec/onchain-transfer-v1/schema/sla-document.schema.json)
- [`spec/onchain-transfer-v1/schema/delivery-evidence.schema.json`](spec/onchain-transfer-v1/schema/delivery-evidence.schema.json)
- [`spec/onchain-transfer-v1/examples/`](spec/onchain-transfer-v1/examples/) —
  approve, mint-mismatch, amount-insufficient, deadline-exceeded.

## Resolution-reason codes

This binary emits codes in `[256, 263]` and `0` / `255`:

| Code        | Meaning                                |
| ----------- | -------------------------------------- |
| 0           | Approved                               |
| 256         | TxNotFound                             |
| 257         | TxFailed                               |
| 258         | ClusterMismatch                        |
| 259         | DeadlineExceeded                       |
| 260         | MintMismatch                           |
| 261         | RecipientMismatch                      |
| 262         | AmountInsufficient                     |
| 263         | DirectionMismatch                      |
| 255         | Unspecified / generic failure          |

These codes are stable across releases; downstream consumers may switch on
the numeric reason.
