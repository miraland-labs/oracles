//! Live `evaluate()` integration test against the deployed devnet `rwa-kyc-hook`
//! mint (closes review finding T2 — the async path was previously only covered
//! by "constant is in range" assertions).
//!
//! This drives the REAL `TransferEvaluator::evaluate` against devnet RPC: it
//! fetches the transfer via `getTransaction(jsonParsed)`, re-derives pre/post
//! token deltas, and applies the RWA token-program + Transfer Hook pins against
//! the on-chain Token-2022 mint configured by the `rwa-kyc-hook` e2e.
//!
//! It is **opt-in** (requires a real, confirmed transfer signature) so CI stays
//! hermetic. Produce the inputs with the kyc-hook e2e, then run:
//!
//! ```bash
//! RWA_ORACLE_DEVNET_E2E=1 \
//! RWA_E2E_TX_SIGNATURE=<sig of a verified seller->buyer Token-2022 transfer> \
//! RWA_E2E_MINT=<mint created with --transfer-hook ky1Nv5Sh...> \
//! RWA_E2E_RECIPIENT_OWNER=<buyer wallet pubkey> \
//! RWA_E2E_MIN_AMOUNT=<raw units expected, e.g. 10000000> \
//! RWA_E2E_HOOK_PROGRAM=ky1Nv5ShhkJ9oxVfPSZmCGEHMXSE8ibkdKYFQC1woo6 \
//! RWA_E2E_RPC_URL=https://api.devnet.solana.com \
//! cargo test -p oracle-rwa-transfer --test evaluate_devnet -- --nocapture --ignored
//! ```
//!
//! Optional: RWA_E2E_SENDER_OWNER (seller wallet) to also exercise sender pinning.

use std::sync::Arc;

use oracle_common::{
    evaluator::{EvaluationContext, OracleEvaluator},
    types::EvaluationJob,
};
use oracle_rwa_transfer::{
    evaluator::TransferEvaluator,
    evidence::{AssertedTransfer, TransferEvidence},
    sla::{ExpectedTransfer, TransferCluster, TransferDirection, TransferSla},
    PROFILE_ID,
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;

const TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn enabled() -> bool {
    env("RWA_ORACLE_DEVNET_E2E").as_deref() == Some("1")
}

/// Build the SLA + evidence + job that bind a real devnet transfer.
/// All three carry the same `payment_uid` so the binding checks pass.
fn build_inputs() -> (TransferSla, TransferEvidence, [u8; 32]) {
    let tx_signature = env("RWA_E2E_TX_SIGNATURE").expect("RWA_E2E_TX_SIGNATURE");
    let mint = env("RWA_E2E_MINT").expect("RWA_E2E_MINT");
    let recipient_owner = env("RWA_E2E_RECIPIENT_OWNER").expect("RWA_E2E_RECIPIENT_OWNER");
    let min_amount = env("RWA_E2E_MIN_AMOUNT").unwrap_or_else(|| "1".into());
    let hook_program = env("RWA_E2E_HOOK_PROGRAM");
    let sender_owner = env("RWA_E2E_SENDER_OWNER");
    let token_program = env("RWA_E2E_TOKEN_PROGRAM").unwrap_or_else(|| TOKEN_2022.into());

    // Deterministic payment_uid for the test binding (hex-64).
    let payment_uid = [0xABu8; 32];
    let payment_uid_hex = hex::encode(payment_uid);

    let sla = TransferSla {
        version: 1,
        profile_id: PROFILE_ID.into(),
        payment_uid: payment_uid_hex.clone(),
        buyer_nonce: None,
        cluster: TransferCluster::Devnet,
        expected_transfers: vec![ExpectedTransfer {
            mint: mint.clone(),
            recipient_owner: recipient_owner.clone(),
            min_amount,
            direction: TransferDirection::In,
            sender_owner,
        }],
        swap_router: None,
        slippage_bps: None,
        deadline_unix: None,
        token_program,
        transfer_hook_program: hook_program,
        offering_id: Some("rwa-e2e".into()),
    };

    let evidence = TransferEvidence {
        version: 1,
        profile_id: PROFILE_ID.into(),
        tx_signature,
        asserted_transfers: vec![AssertedTransfer {
            mint,
            recipient_owner,
            claimed_delta: "0".into(),
        }],
        submitted_at: 0,
        payment_uid: payment_uid_hex,
        buyer_nonce: None,
    };

    (sla, evidence, payment_uid)
}

fn job_for(payment_uid: [u8; 32]) -> EvaluationJob {
    EvaluationJob {
        payment_uid,
        payment_pubkey: Pubkey::new_unique(),
        sla_hash: [0u8; 32],
        delivery_hash: [0u8; 32],
        amount: 0,
        mint: Pubkey::new_unique(),
        oracle_authority: Pubkey::new_unique(),
        oracle_fee_bps: 0,
        expires_at: 0,
        // created_at = 0 → freshness lower-bound check is skipped (we are not
        // asserting freshness here, only the RWA verification path).
        created_at: 0,
        delivery_cutoff_seconds: 0,
        sla_bytes: None,
        retry_count: 0,
    }
}

#[tokio::test]
#[ignore = "live devnet: requires RWA_ORACLE_DEVNET_E2E=1 + a real transfer signature"]
async fn evaluate_approves_real_devnet_kyc_hook_transfer() {
    if !enabled() {
        eprintln!("skipping: set RWA_ORACLE_DEVNET_E2E=1 and the RWA_E2E_* inputs");
        return;
    }

    let rpc_url = env("RWA_E2E_RPC_URL").unwrap_or_else(|| "https://api.devnet.solana.com".into());
    let rpc = Arc::new(RpcClient::new(rpc_url));
    let http = reqwest::Client::new();

    let (sla, evidence, payment_uid) = build_inputs();
    let job = job_for(payment_uid);
    let ctx = EvaluationContext {
        rpc: &rpc,
        http: &http,
        job: &job,
        strict: true,
        ledger: None,
    };

    let evaluator = TransferEvaluator::new(TransferCluster::Devnet);
    let result = evaluator
        .evaluate(&ctx, &sla, &evidence)
        .await
        .expect("evaluate() must not error at the RPC layer");

    eprintln!(
        "evaluate() => approved={} reason={} details={}",
        result.approved,
        result.resolution_reason,
        serde_json::to_string_pretty(&result.resolution_details).unwrap_or_default()
    );

    assert!(
        result.approved,
        "expected approval for a verified kyc-hook transfer; got reason {}",
        result.resolution_reason
    );
    assert_eq!(result.resolution_reason, 0);
}

/// Negative live check: the same transfer evaluated with a DIFFERENT cluster
/// pin must reject with the cluster-mismatch code (455), without any RPC.
#[tokio::test]
#[ignore = "live devnet: requires RWA_ORACLE_DEVNET_E2E=1 + a real transfer signature"]
async fn evaluate_rejects_on_cluster_mismatch() {
    if !enabled() {
        eprintln!("skipping: set RWA_ORACLE_DEVNET_E2E=1 and the RWA_E2E_* inputs");
        return;
    }

    let rpc = Arc::new(RpcClient::new(
        env("RWA_E2E_RPC_URL").unwrap_or_else(|| "https://api.devnet.solana.com".into()),
    ));
    let http = reqwest::Client::new();
    let (sla, evidence, payment_uid) = build_inputs();
    let job = job_for(payment_uid);
    let ctx = EvaluationContext {
        rpc: &rpc,
        http: &http,
        job: &job,
        strict: true,
        ledger: None,
    };

    // Evaluator pinned to mainnet-beta; SLA says devnet → reject 455 before any RPC.
    let evaluator = TransferEvaluator::new(TransferCluster::MainnetBeta);
    let result = evaluator.evaluate(&ctx, &sla, &evidence).await.unwrap();
    assert!(!result.approved);
    assert_eq!(
        result.resolution_reason,
        oracle_common::resolution_codes::rwa_transfer::TRANSFER_CLUSTER_MISMATCH
    );
}
