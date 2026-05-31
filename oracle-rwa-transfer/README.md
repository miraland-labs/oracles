# oracle-rwa-transfer

RWA Token-2022 primary delivery oracle for the x402/pr402 ecosystem.
Implements [`x402/oracles/rwa-transfer/v1`](../../spec/rwa-transfer/v1/NORMATIVE.md):
re-derives token deltas from `getTransaction(jsonParsed)`, pins Token-2022
`token_program`, and approves when delivery satisfies the SLA.

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
cp oracle-rwa-transfer/.env.example /tmp/oracle-rwa-transfer.env
# edit /tmp/oracle-rwa-transfer.env: ESCROW_PROGRAM_ID, ORACLE_KEYPAIR_PATH,
# DATABASE_URL, TRANSFER_CLUSTER (mainnet|devnet|testnet|custom).
psql "$DATABASE_URL" < oracle-common/migrations/init.sql
env $(grep -v '^#' /tmp/oracle-rwa-transfer.env | xargs) \
    cargo run --release -p oracle-rwa-transfer
```

Default port: `:4021`. HTTP surface is identical to the api-quality binary
(see its README); the per-family difference is the evaluator wired in
`main.rs`.

## Production install (Ubuntu 24.04)

```bash
sudo bash oracles/scripts/install.sh \
    rwa-transfer \
    https://github.com/miraland-labs/oracles/releases/download/oracle-rwa-transfer-vX/oracle-rwa-transfer \
    /tmp/oracle-rwa-transfer.env
sudo systemctl status oracle@rwa-transfer
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
- The `oracle-rwa-transfer` binary running (default port `:4021`)
- A buyer wallet with at least one ATA created for the expected mint
- A seller wallet that holds the tokens being transferred and is registered
  against the registry
- Exported: `ORACLE_HOST`, `BUYER_PUBKEY`, `TX_SIGNATURE` (set by the seller
  step), `DEVNET_USDC_MINT` (default
  `Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB`)
- `TRANSFER_CLUSTER=devnet` in the env file (the evaluator refuses to verify
  signatures from a different cluster)

### Operational notes

- One `oracle-rwa-transfer` binary serves exactly one cluster. To
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

- **Authoritative normative spec:**
  [`../../spec/rwa-transfer/v1/NORMATIVE.md`](../../spec/rwa-transfer/v1/NORMATIVE.md)
  (the crate-local [`spec/rwa-transfer-v1/NORMATIVE.md`](spec/rwa-transfer-v1/NORMATIVE.md)
  is a pointer to it).
- [`spec/rwa-transfer-v1/schema/sla-document.schema.json`](spec/rwa-transfer-v1/schema/sla-document.schema.json)
- [`spec/rwa-transfer-v1/schema/delivery-evidence.schema.json`](spec/rwa-transfer-v1/schema/delivery-evidence.schema.json)
- [`spec/rwa-transfer-v1/examples/`](spec/rwa-transfer-v1/examples/) —
  `sla.approve`, `sla.amount-insufficient`, `sla.with-sender-binding`,
  `delivery.approve`.

## Resolution-reason codes

This binary emits `0` (approved) plus the `rwa-transfer/v1` window `[448, 479]`
(NORMATIVE §8). These differ from the sibling `oracle-onchain-transfer`
(`256–319`); downstream consumers MUST switch on the numbers below.

| Code | Constant                          | Meaning                                   |
| ---- | --------------------------------- | ----------------------------------------- |
| 0    | —                                 | Approved                                  |
| 448  | RwaTokenProgramMismatch           | Mint owner ≠ SLA `token_program`          |
| 449  | RwaTransferHookMismatch           | Mint hook extension vs SLA mismatch       |
| 450  | RwaTransferTxNotFound             | RPC has no tx for the signature           |
| 451  | RwaTransferTxFailed               | `meta.err` set (includes hook revert)     |
| 452  | RwaTransferAmountInsufficient     | Delta below `min_amount`                  |
| 453  | RwaTransferMintMismatch           | No matching `(mint, owner)` balance row   |
| 454  | RwaTransferDeadlineExceeded       | Past `deadline_unix`                      |
| 455  | RwaTransferClusterMismatch        | SLA `cluster` ≠ oracle config             |
| 456  | RwaTransferDirectionMismatch      | Wrong delta sign for `direction`          |
| 457  | RwaTransferSenderMismatch         | `sender_owner` pin failed                 |
| 458  | RwaTransferEvidencePredatesPayment| `block_time` < `Payment.created_at`       |
| 459  | RwaTransferTxSignatureReused      | Replay across payments                    |
| 460  | RwaTransferPaymentUidMismatch     | `payment_uid` binding failure             |
| 461  | RwaTransferBuyerNonceMismatch     | `buyer_nonce` echo failure                |
| 462  | RwaTransferBlockTimeMissing       | `block_time` absent when freshness/deadline required |
| 463  | RwaTransferRecipientNotResolvable | Reserved (destination ATA never derived)  |

These codes are stable across releases; downstream consumers may switch on
the numeric reason.

