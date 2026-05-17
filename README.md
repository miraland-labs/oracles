# oracles/

Multi-category oracle workspace for the x402/pr402 ecosystem. One shared library
([`oracle-common`](oracle-common/)) plus three independently-deployable sibling
binaries — one per delivery category — that settle SLA-Escrow verdicts on
Solana via `ConfirmOracle`.

| Crate                      | Profile                                  | Default port | When to run                                 |
| -------------------------- | ---------------------------------------- | ------------ | ------------------------------------------- |
| `oracle-api-quality`       | `x402/oracles/api-quality/v1`                    | `:4020`      | JSON API quality (status / latency / schema) |
| `oracle-onchain-transfer`  | `x402/oracles/onchain-transfer/v1`               | `:4021`      | SPL transfer / swap delivery                 |
| `oracle-file-delivery`     | `x402/oracles/file-delivery/attestation/v1`      | `:4022`      | Large-file (video / binary) delivery        |

The on-chain `sla-escrow` program is **not** modified by anything in this
workspace; complexity lives off-chain.

## Documentation map

| File                                                              | Purpose                                                                |
| ----------------------------------------------------------------- | ---------------------------------------------------------------------- |
| [`docs/SELLER_GUIDE.md`](docs/SELLER_GUIDE.md)                    | **Sellers start here.** Three copy-paste integration recipes.          |
| [`docs/BUYER_GUIDE.md`](docs/BUYER_GUIDE.md)                      | **Buyers start here.** How to pick an oracle and fund via pr402.       |
| [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md)                        | Full bring-up from clean Ubuntu 24.04 to production-ready oracle.      |
| [`docs/OPERATIONS.md`](docs/OPERATIONS.md)                        | Day-2 ops: monitoring, incidents, rotations, backup, audit, failover. |
| [`docs/marketing/oracle-intro-article.md`](docs/marketing/oracle-intro-article.md) | Recruiting article for prospective oracle developers (~3 min read).    |
| [`docs/marketing/oracle-intro-video-script.md`](docs/marketing/oracle-intro-video-script.md) | Narration script for the 2-minute oracle introduction video.            |
| [`scripts/README.md`](scripts/README.md)                          | Smoke-test runbook for `install.sh` / `upgrade.sh` / `uninstall.sh`.   |
| [`oracle-common/docs/PR402_CONTRACT.md`](oracle-common/docs/PR402_CONTRACT.md) | Buyer ↔ seller ↔ oracle discovery contract for `pr402`.                |
| [`oracle-common/docs/devnet-evidence/README.md`](oracle-common/docs/devnet-evidence/README.md) | Evidence-capture layout for the Phase D final-integration milestone.   |
| `oracle-*/README.md`                                              | Per-family quick-start and resolution-code reference (operator-facing).|
| `oracle-*/spec/*/NORMATIVE.md`                                    | Per-profile normative specification (SLA + evidence shapes).           |
| [`.kiro/specs/multi-category-oracle-architecture/`](../.kiro/specs/multi-category-oracle-architecture/) | Architectural source of truth: 13 constraints + 33 properties + tasks. |

## Quick links

- **You're a seller integrating with this**: read
  [`docs/SELLER_GUIDE.md`](docs/SELLER_GUIDE.md) — five-minute scan.
- **You're a buyer paying for an SLA-escrow service**: read
  [`docs/BUYER_GUIDE.md`](docs/BUYER_GUIDE.md) — three steps to fund.
- **Trying it locally as an operator**: pick one family's README —
  `oracle-api-quality/README.md` is the simplest entry point.
- **Deploying to a real VPS**: read [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md).
- **Already running, need to debug something**: jump to
  [`docs/OPERATIONS.md`](docs/OPERATIONS.md).
- **Integrating with `pr402` for discovery / advertising**: read
  [`oracle-common/docs/PR402_CONTRACT.md`](oracle-common/docs/PR402_CONTRACT.md).

## Build

```bash
cargo build --workspace --release
cargo test --workspace
```

`rustc` 1.92+ is required (pinned in `rust-toolchain.toml`). The MinIO
integration test (`oracle-file-delivery/tests/minio_integration.rs`) is
`#[ignore]`-marked; run with `--ignored` once you've pointed
`ORACLE_TEST_MINIO_ENDPOINT` at a real bucket.

## Status

102 workspace tests passing, 3 ignored (MinIO integration; runs in CI against a
service container). `cargo clippy --workspace --all-targets -- -D warnings`
is clean. `cargo deny` config at [`deny.toml`](deny.toml).

## License

Apache-2.0
