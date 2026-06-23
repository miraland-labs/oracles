# 🛠️ Oracle Developer Guide

This guide walks you through building a custom oracle family for the x402 SLA-Escrow ecosystem. 

Your oracle will watch the Solana blockchain for `SubmitDelivery` events, fetch the SLA and evidence from a registry, evaluate them according to your custom rules, and post a binding on-chain verdict (`ConfirmOracle`).

---

## 🏗️ Architecture Overview

The `oracle-common` crate handles the boring stuff (blockchain monitoring, registry servers, transaction signing). You only need to write the **domain evaluation logic**.

```
Your Oracle Binary
├── oracle-common (Shared Library)
│   ├── Chain Monitor & Event Listener
│   ├── Active Guardian (automatic safety reject if seller withholds proof)
│   └── Settler (ConfirmOracle signer)
└── Your Custom Crate (~200 lines)
    ├── SLA & Evidence Structs (JSON schemas)
    ├── OracleEvaluator Implementation (custom logic)
    └── main.rs (wiring)
```

---

## 🚀 Step-by-Step Implementation

### Step 1: Define Profile ID & JSON Schemas
Your profile is a versioned string identifying your rule family. E.g., `x402/oracles/gpu-inference/v1`.

Your **SLA** and **Evidence** must extend the default envelope:
```json
// SLA Schema (sla.json)
{
  "version": 1,
  "profile_id": "x402/oracles/gpu-inference/v1",
  "payment_uid": "<64-character-hex-string>",
  "buyer_nonce": "<optional-nonce>",
  "model": "llama-3-70b",
  "max_latency_ms": 5000
}
```

---

### Step 2: Implement the `OracleEvaluator` Trait
Create a Rust module with your SLA/Evidence structs and implement `OracleEvaluator`:

```rust
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use oracle_common::{
    evaluator::{EvaluationContext, OracleEvaluator},
    error::OracleError,
    types::{EvaluationResult, CheckResult},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceSla {
    pub version: u32,
    pub profile_id: String,
    pub payment_uid: String,
    pub max_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceEvidence {
    pub version: u32,
    pub profile_id: String,
    pub payment_uid: String,
    pub latency_ms: u64,
}

pub struct InferenceEvaluator;

#[async_trait]
impl OracleEvaluator for InferenceEvaluator {
    type Sla = InferenceSla;
    type Evidence = InferenceEvidence;

    fn profile_id(&self) -> &'static str {
        "x402/oracles/gpu-inference/v1"
    }

    async fn evaluate(
        &self,
        ctx: &EvaluationContext<'_>,
        sla: &Self::Sla,
        evidence: &Self::Evidence,
    ) -> Result<EvaluationResult, OracleError> {
        let mut checks = Vec::new();
        let mut approved = true;

        // 1. Validate payment_uid match (replay protection)
        let uid_ok = sla.payment_uid == evidence.payment_uid;
        checks.push(CheckResult { name: "payment_uid_match".into(), passed: uid_ok, detail: "".into() });
        approved &= uid_ok;

        // 2. Evaluate performance bounds
        let latency_ok = evidence.latency_ms <= sla.max_latency_ms;
        checks.push(CheckResult { name: "latency".into(), passed: latency_ok, detail: format!("got={}, max={}", evidence.latency_ms, sla.max_latency_ms) });
        approved &= latency_ok;

        Ok(EvaluationResult {
            approved,
            resolution_reason: if approved { 0 } else { 1 },
            checks,
        })
    }
}
```

---

### Step 3: Wire the Evaluator in `main.rs`
Instantiate the helper structs from `oracle-common` and register your profile:

```rust
use std::sync::Arc;
use oracle_common::{
    fetcher::RegistryJsonFetcher,
    profile::{ProfileBinding, ProfileRegistry, RegisteredProfile},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let http = reqwest::Client::new();
    let evaluator = Arc::new(InferenceEvaluator);

    let sla_fetcher = Arc::new(RegistryJsonFetcher::<InferenceSla>::new(http.clone(), fetcher_cfg.clone()));
    let evidence_fetcher = Arc::new(RegistryJsonFetcher::<InferenceEvidence>::new(http.clone(), fetcher_cfg.clone()));

    let mut profiles = ProfileRegistry::new();
    profiles.register(RegisteredProfile {
        profile_id: "x402/oracles/gpu-inference/v1",
        run: Arc::new(ProfileBinding {
            evaluator,
            sla_fetcher,
            evidence_fetcher,
        }),
    });

    // Start the oracle daemon
    oracle_common::bootstrap(profiles).await?;
    Ok(())
}
```

---

### Step 4: Configure and Run
Run your compiled binary locally or inside a Docker container using these environment variables:

```bash
# Required environment keys
ORACLE_KEYPAIR_PATH=/path/to/oracle-keypair.json
ORACLE_REGISTRY_BACKEND=local # 'local', 'postgres', or 's3'
SOLANA_RPC_URL=https://api.devnet.solana.com
SOLANA_WS_URL=wss://api.devnet.solana.com
```

Test the liveness of your daemon:
```bash
curl -s http://localhost:4020/health | jq .
curl -s http://localhost:4020/v1/policy | jq .
```

---

### Step 5: Onboard with pr402
1. Share your **Oracle Public Key** and **Normative Specification URL** with the pr402 operator.
2. The operator lists your oracle profile under `Capabilities.slaEscrowOracleProfiles[]` so buyers can discover and select it.
