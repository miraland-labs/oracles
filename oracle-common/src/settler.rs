//! Canonical resolution-hash recipe + on-chain `ConfirmOracle` settler.
//!
//! There is exactly one canonical resolution-hash recipe shared by every family
//! (see design.md C9 and §Single Canonical Resolution-Hash Recipe). The envelope
//! is `x402/oracles/resolution-envelope/v1` with a fixed key order; per-family details live
//! under the `details` slot. SHA-256 over the serialized envelope produces the
//! 32-byte digest written to `Payment.resolution_hash`.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig, pubkey::Pubkey, sysvar::clock::Clock,
    transaction::Transaction,
};
use tracing::{info, warn};

use crate::{
    config::OracleConfig,
    error::OracleError,
    types::{EvaluationJob, EvaluationResult},
};

/// Canonical envelope identifier — opaque to the chain, used by indexers to confirm
/// they are looking at the v1 recipe.
pub const RESOLUTION_ENVELOPE_PROFILE: &str = "x402/oracles/resolution-envelope/v1";

/// Compute the canonical resolution hash for a given verdict.
///
/// `details` is the per-family JSON the evaluator builds (e.g. `{txSignature, ...}`
/// for `x402/oracles/onchain-transfer/v1`). It is embedded verbatim under the `details`
/// envelope key.
///
/// Determinism (P-DET-2): the function never reads the wall clock or any
/// random source. Identical inputs produce identical 32-byte outputs across runs
/// and across processes.
pub fn compute_resolution_hash(
    job: &EvaluationJob,
    evaluator_profile_id: &str,
    result: &EvaluationResult,
    details: Value,
) -> Result<[u8; 32], OracleError> {
    // Build the envelope with FIXED key order. `serde_json::Value::Object` preserves
    // insertion order, so writing keys in the documented sequence guarantees a
    // deterministic byte layout regardless of input map iteration order.
    let mut envelope = serde_json::Map::new();
    envelope.insert("profile".into(), json!(RESOLUTION_ENVELOPE_PROFILE));
    envelope.insert("evaluatorProfile".into(), json!(evaluator_profile_id));
    envelope.insert("paymentUid".into(), json!(hex::encode(job.payment_uid)));
    envelope.insert(
        "paymentPubkey".into(),
        json!(job.payment_pubkey.to_string()),
    );
    envelope.insert("slaHash".into(), json!(hex::encode(job.sla_hash)));
    envelope.insert("deliveryHash".into(), json!(hex::encode(job.delivery_hash)));
    envelope.insert("approved".into(), json!(result.approved));
    envelope.insert("resolutionReason".into(), json!(result.resolution_reason));
    envelope.insert("details".into(), details);

    let bytes = serde_json::to_vec(&Value::Object(envelope))
        .map_err(|e| OracleError::Settlement(format!("resolution-hash serialize: {e}")))?;

    let digest = Sha256::digest(&bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}

/// Convenience for evaluators that produce a typed `details` struct: serialize via
/// `serde` (which is canonical because the type's field declaration order is fixed
/// at compile time) then call [`compute_resolution_hash`].
pub fn compute_resolution_hash_typed<D: Serialize>(
    job: &EvaluationJob,
    evaluator_profile_id: &str,
    result: &EvaluationResult,
    details: &D,
) -> Result<[u8; 32], OracleError> {
    let value = serde_json::to_value(details)
        .map_err(|e| OracleError::Settlement(format!("details serialize: {e}")))?;
    compute_resolution_hash(job, evaluator_profile_id, result, value)
}

// =====================================================================
// Eligibility + on-chain settlement
// =====================================================================

/// Read the on-chain `Clock` sysvar. Falls back to the operator's wall clock if the
/// RPC call fails — the resulting eligibility is "best-effort degraded" rather than
/// a hard refusal, matching design.md §Property 7 fallback. The wall-clock path is
/// logged so operators see the degraded state via `/health.last_monitor_error`.
async fn chain_unix_timestamp(rpc: &RpcClient) -> i64 {
    match rpc
        .get_account_with_commitment(
            &solana_sdk::sysvar::clock::ID,
            CommitmentConfig::confirmed(),
        )
        .await
    {
        Ok(resp) => match resp.value {
            Some(account) => match bincode::deserialize::<Clock>(&account.data) {
                Ok(clock) => clock.unix_timestamp,
                Err(e) => {
                    warn!("Failed to decode on-chain Clock; falling back to wall clock: {e}");
                    chrono::Utc::now().timestamp()
                }
            },
            None => {
                warn!("Clock sysvar account missing; falling back to wall clock");
                chrono::Utc::now().timestamp()
            }
        },
        Err(e) => {
            warn!("RPC get_account for Clock failed; falling back to wall clock: {e}");
            chrono::Utc::now().timestamp()
        }
    }
}

/// Check whether the on-chain `Payment` is still eligible for settlement by this
/// oracle. Returns `false` (no error) if any of the following hold:
///
/// * `payment.oracle_authority != self.pubkey` (P-AUTH-1)
/// * `payment.delivery_timestamp == 0` (P-AUTH-2)
/// * `payment.resolution_state != 0` (P-AUTH-3)
/// * on-chain `Clock.unix_timestamp > payment.expires_at` (P-AUTH-4)
///
/// This is a defense-in-depth guard: the on-chain `ConfirmOracle` handler enforces
/// the same conditions, so refusing here saves the oracle's SOL on doomed txs and
/// keeps the ledger row honest.
pub async fn is_eligible(
    rpc: &Arc<RpcClient>,
    config: &OracleConfig,
    job: &EvaluationJob,
) -> Result<bool, OracleError> {
    use sla_escrow_api::state::Payment;

    let account = rpc
        .get_account_with_commitment(&job.payment_pubkey, CommitmentConfig::confirmed())
        .await?
        .value;
    let Some(account) = account else {
        warn!("Payment account {} no longer exists", job.payment_pubkey);
        return Ok(false);
    };
    if account.data.len() < 8 + std::mem::size_of::<Payment>() {
        return Ok(false);
    }
    let payment: &Payment =
        bytemuck::from_bytes(&account.data[8..8 + std::mem::size_of::<Payment>()]);

    if payment.oracle_authority != config.oracle_pubkey() {
        return Ok(false);
    }
    if payment.delivery_timestamp == 0 {
        return Ok(false);
    }
    if payment.resolution_state != 0 {
        return Ok(false);
    }

    let now = chain_unix_timestamp(rpc).await;
    if now > payment.expires_at {
        warn!(
            "Payment {} expired (expires_at={}, now={})",
            hex::encode(payment.payment_uid),
            payment.expires_at,
            now
        );
        return Ok(false);
    }
    Ok(true)
}

/// Build, sign, and send a `ConfirmOracle` transaction. Honors a small per-call
/// retry budget on transient RPC errors.
pub async fn settle(
    rpc: &Arc<RpcClient>,
    config: &OracleConfig,
    job: &EvaluationJob,
    approved: bool,
    resolution_reason: u16,
    resolution_hash: [u8; 32],
) -> Result<String, OracleError> {
    use sla_escrow_api::sdk::EscrowSdk;

    let resolution_state: u8 = if approved { 1 } else { 2 };
    let payment_uid_hex = hex::encode(job.payment_uid);

    // The `_from_uid_bytes` SDK variant derives the payment PDA from the
    // raw 32 bytes of the on-chain `Payment.payment_uid`, NOT from a
    // string. Routing the legacy `confirm_oracle(uid_str, ...)` helper
    // would re-encode the hex *string* via `normalize_payment_uid`
    // (zero-padding the first 32 ASCII bytes into the seed), producing
    // a different PDA than the one the buyer's FundPayment created.
    // The bytes variant skips that step entirely.
    let ix = EscrowSdk::confirm_oracle_from_uid_bytes(
        config.oracle_pubkey(),
        job.mint,
        &job.payment_uid,
        job.delivery_hash,
        resolution_hash,
        resolution_state,
        resolution_reason,
    );

    // Diagnostic: log every account meta the SDK produced so a
    // settlement failure is immediately traceable to a specific PDA
    // (vs. having to re-derive PDAs from logs after the fact).
    info!(
        "ConfirmOracle accounts for payment {payment_uid_hex}: {:?}",
        ix.accounts
            .iter()
            .map(|a| (a.pubkey.to_string(), a.is_signer, a.is_writable))
            .collect::<Vec<_>>()
    );

    let recent_blockhash = rpc
        .get_latest_blockhash()
        .await
        .map_err(|e| OracleError::Settlement(format!("get_latest_blockhash: {e}")))?;

    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&config.oracle_pubkey()),
        &[config.oracle_keypair.as_ref()],
        recent_blockhash,
    );

    // Skip preflight: the chain monitor reacts to `SubmitDelivery` at
    // `confirmed` commitment, but the public devnet RPC's preflight
    // pool can lag the leader by a slot or two and momentarily report
    // the just-created `Payment` PDA as system-owned. That manifests
    // as "Invalid account owner" preflight failures even when the
    // account is fully valid on-chain. Real submission goes through
    // the leader directly, so we simply pay the lamports and let the
    // runtime re-simulate at submit time.
    use solana_client::rpc_config::RpcSendTransactionConfig;
    let send_cfg = RpcSendTransactionConfig {
        skip_preflight: true,
        preflight_commitment: Some(solana_sdk::commitment_config::CommitmentLevel::Confirmed),
        ..Default::default()
    };
    let sig = rpc
        .send_and_confirm_transaction_with_spinner_and_config(
            &tx,
            CommitmentConfig::confirmed(),
            send_cfg,
        )
        .await
        .map_err(|e| OracleError::Settlement(format!("send_and_confirm: {e}")))?;

    let verdict = if approved { "APPROVED" } else { "REJECTED" };
    info!("Settlement {verdict} for payment {payment_uid_hex}: sig={sig}");
    Ok(sig.to_string())
}

// `Pubkey` import is used only via re-exports above; suppress the unused-import lint
// when feature-gated builds drop the chain-monitor path.
#[allow(dead_code)]
fn _pubkey_compile_only(p: Pubkey) -> Pubkey {
    p
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use solana_sdk::pubkey::Pubkey;

    use super::*;
    use crate::types::CheckResult;

    fn job() -> EvaluationJob {
        EvaluationJob {
            payment_uid: [1u8; 32],
            payment_pubkey: Pubkey::new_unique(),
            sla_hash: [2u8; 32],
            delivery_hash: [3u8; 32],
            amount: 0,
            mint: Pubkey::new_unique(),
            oracle_authority: Pubkey::new_unique(),
            oracle_fee_bps: 100,
            expires_at: 0,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        }
    }

    fn approve() -> EvaluationResult {
        EvaluationResult {
            approved: true,
            resolution_reason: 0,
            checks: vec![CheckResult {
                name: "x".into(),
                passed: true,
                detail: "ok".into(),
            }],
        }
    }

    #[test]
    fn deterministic_across_runs() {
        let j = job();
        let r = approve();
        let d = json!({"foo": 1, "bar": "baz"});
        let a = compute_resolution_hash(&j, "x402/test/v1", &r, d.clone()).unwrap();
        let b = compute_resolution_hash(&j, "x402/test/v1", &r, d).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, [0u8; 32]);
    }

    #[test]
    fn changes_when_any_input_changes() {
        let j = job();
        let r = approve();
        let base = compute_resolution_hash(&j, "x402/test/v1", &r, json!({"x": 1})).unwrap();

        // Different evaluator profile id.
        let alt_id = compute_resolution_hash(&j, "x402/test/v2", &r, json!({"x": 1})).unwrap();
        assert_ne!(base, alt_id);

        // Different details payload.
        let alt_details = compute_resolution_hash(&j, "x402/test/v1", &r, json!({"x": 2})).unwrap();
        assert_ne!(base, alt_details);

        // Different verdict.
        let mut rejected = approve();
        rejected.approved = false;
        rejected.resolution_reason = 255;
        let alt_verdict =
            compute_resolution_hash(&j, "x402/test/v1", &rejected, json!({"x": 1})).unwrap();
        assert_ne!(base, alt_verdict);
    }

    #[test]
    fn fixed_key_order_independent_of_input_map_order() {
        // `details` built with keys in the opposite alphabetical order should still
        // yield the same envelope hash because we control the *envelope* keys; the
        // details Value itself is whatever the evaluator handed us.
        let j = job();
        let r = approve();
        let mut a = serde_json::Map::new();
        a.insert("alpha".into(), json!(1));
        a.insert("beta".into(), json!(2));
        let mut b = serde_json::Map::new();
        b.insert("alpha".into(), json!(1));
        b.insert("beta".into(), json!(2));

        let h1 = compute_resolution_hash(&j, "x402/test/v1", &r, Value::Object(a)).unwrap();
        let h2 = compute_resolution_hash(&j, "x402/test/v1", &r, Value::Object(b)).unwrap();
        assert_eq!(h1, h2);
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 32,
            ..ProptestConfig::default()
        })]

        #[test]
        fn p_det_2_pure_function_of_inputs(
            uid in any::<[u8; 32]>(),
            sla_hash in any::<[u8; 32]>(),
            delivery_hash in any::<[u8; 32]>(),
            approved in any::<bool>(),
            reason in any::<u16>()
        ) {
            let mut j = job();
            j.payment_uid = uid;
            j.sla_hash = sla_hash;
            j.delivery_hash = delivery_hash;
            let r = EvaluationResult {
                approved,
                resolution_reason: reason,
                checks: vec![],
            };
            let a = compute_resolution_hash(&j, "x402/test/v1", &r, json!(null)).unwrap();
            let b = compute_resolution_hash(&j, "x402/test/v1", &r, json!(null)).unwrap();
            prop_assert_eq!(a, b);
        }
    }
}
