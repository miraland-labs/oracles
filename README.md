# ⚖️ Oracles Workspace

This workspace houses the off-chain oracle framework for the `sla-escrow` protocol. It includes a shared verification library (`oracle-common`) and three reference oracle implementations (sibling binaries) that publish binding verdicts (`ConfirmOracle`) on Solana.

---

## 📦 Oracle Crate Directory

| Crate Name | Profile ID | Default Port | Purpose |
| :--- | :--- | :--- | :--- |
| **`oracle-api-quality`** | `x402/oracles/api-quality/v1` | `4020` | Evaluates JSON HTTP response status, latencies, and schemas. |
| **`oracle-onchain-transfer`** | `x402/oracles/onchain-transfer/v1` | `4021` | Verifies SPL token transfers by inspecting Solana transactions. |
| **`oracle-file-delivery`** | `x402/oracles/file-delivery/attestation/v1` | `4022` | Attests to binary/large file delivery sizes and MIME types. |

---

## 📖 Developer & Integrator Guides

* **[SLA-Escrow Overview](docs/SLA_ESCROW_PROTOCOL.md):** High-level cross-actor protocol flow.
* **[Oracle Developer Guide](docs/ORACLE_DEVELOPER_GUIDE.md):** Step-by-step walkthrough to build your own custom oracle.
* **[Seller Integration Guide](docs/SELLER_GUIDE.md):** Actionable recipes for upload, evidence committing, and delivery submission.
* **[Buyer Integration Guide](docs/BUYER_GUIDE.md):** Three-step recipe to pick an oracle and fund the escrow.
* **[Deployment & Operations](docs/DEPLOYMENT.md):** Server bring-up, daemon management, and systemd specs.

---

## 🚀 Building & Testing

### Compilation
```bash
# Compile all crates in the workspace
cargo build --workspace --release
```

### Running Tests
```bash
# Run all workspace unit and integration tests
cargo test --workspace
```
*(Requires Rust 1.92+ as defined in `rust-toolchain.toml`)*
