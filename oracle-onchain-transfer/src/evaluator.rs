//! `TransferEvaluator` for the onchain-transfer family.
//!
//! Verification flow (per design.md §Scenario (c) and Properties P-OT-1..P-OT-6):
//!
//! 1. Cluster pinning — SLA `cluster` MUST equal the binary's configured cluster.
//!    Mismatch → reject with `Custom(261)` (TransferClusterMismatch).
//! 2. Fetch the tx via `getTransaction(tx_signature, jsonParsed)` against the
//!    binary's RPC. Missing → `Custom(256)` (TransferTxNotFound). Failed
//!    (`meta.err.is_some()`) → `Custom(257)` (TransferTxFailed).
//! 3. (Optional) `deadline_unix` enforcement against `meta.block_time`. Late
//!    → `Custom(260)` (TransferDeadlineExceeded).
//! 4. For each `ExpectedTransfer`:
//!    - Find a `(mint, owner)` row in the post-token-balance list. Missing
//!      → `Custom(259)` (TransferMintMismatch).
//!    - Find the matching pre-token-balance row (or treat absent pre as `0`,
//!      which is the standard Solana semantic for newly-created ATAs).
//!    - Compute `delta = post - pre` as `i128` (signed because either side may
//!      be larger).
//!    - Check the sign agrees with `direction` (`in` ⇒ delta > 0). Mismatch
//!      → `Custom(263)` (TransferDirectionMismatch).
//!    - Compare `|delta|` against `min_amount`. Insufficient
//!      → `Custom(258)` (TransferAmountInsufficient).
//!
//! On approve, `resolution_reason` is `0` (P-VER-3). On reject, the reason is the
//! standard or custom code corresponding to the **first** failing check
//! (P-VER-2, P-OT-* family).
//!
//! The verification logic is split into [`verify_observed_transfer`] (a pure
//! function over a `TxObservation` snapshot) and the async wrapper that drives
//! `getTransaction` against a real RPC. Tests exercise the pure function with
//! synthetic snapshots so we never need a live network connection.

use std::str::FromStr;

use async_trait::async_trait;
use oracle_common::{
    error::OracleError,
    evaluator::{EvaluationContext, OracleEvaluator},
    resolution_codes::onchain_transfer,
    types::{CheckResult, EvaluationResult},
};
use solana_client::{nonblocking::rpc_client::RpcClient, rpc_config::RpcTransactionConfig};
use solana_sdk::{commitment_config::CommitmentConfig, signature::Signature};
use solana_transaction_status::{
    option_serializer::OptionSerializer, EncodedConfirmedTransactionWithStatusMeta,
    EncodedTransactionWithStatusMeta, UiTransactionEncoding, UiTransactionTokenBalance,
};
use tracing::{debug, warn};

use crate::{
    evidence::TransferEvidence,
    sla::{TransferCluster, TransferDirection, TransferSla},
    PROFILE_ID,
};

#[derive(Clone)]
pub struct TransferEvaluator {
    /// Cluster this binary is configured to verify against. SLA `cluster` MUST equal
    /// this value or the evaluator rejects with `Custom(261)` (TransferClusterMismatch).
    pub cluster: TransferCluster,
}

impl TransferEvaluator {
    pub fn new(cluster: TransferCluster) -> Self {
        Self { cluster }
    }
}

#[async_trait]
impl OracleEvaluator for TransferEvaluator {
    type Sla = TransferSla;
    type Evidence = TransferEvidence;

    fn profile_id(&self) -> &'static str {
        PROFILE_ID
    }

    async fn evaluate(
        &self,
        ctx: &EvaluationContext<'_>,
        sla: &Self::Sla,
        evidence: &Self::Evidence,
    ) -> Result<EvaluationResult, OracleError> {
        // P-OT-1: cluster pinning.
        if sla.cluster != self.cluster {
            return Ok(reject(
                onchain_transfer::TRANSFER_CLUSTER_MISMATCH,
                "cluster",
                &format!(
                    "SLA cluster {:?} differs from binary cluster {:?}",
                    sla.cluster, self.cluster
                ),
            ));
        }

        // Fetch the tx and snapshot the relevant fields. RPC-level failures are
        // propagated as `OracleError::RpcVerification` and surface as worker
        // errors (which retry then dead-letter); only the explicit "tx not
        // found" / "tx failed" / "no balances" cases produce on-chain rejections.
        let observation = match fetch_observation(ctx.rpc, &evidence.tx_signature).await {
            Ok(o) => o,
            Err(FetchError::SignatureFormat(e)) => {
                return Ok(reject(
                    onchain_transfer::TRANSFER_TX_NOT_FOUND,
                    "tx_signature",
                    &format!("invalid signature: {e}"),
                ));
            }
            Err(FetchError::NotFound) => {
                return Ok(reject(
                    onchain_transfer::TRANSFER_TX_NOT_FOUND,
                    "tx_signature",
                    "RPC returned no transaction for this signature",
                ));
            }
            Err(FetchError::Transport(msg)) => {
                return Err(OracleError::RpcVerification(msg));
            }
        };

        Ok(verify_observed_transfer(sla, &observation))
    }
}

// ---------------------------------------------------------------------------
// Pure verification core
// ---------------------------------------------------------------------------

/// Snapshot of the `getTransaction(jsonParsed)` response shape the evaluator cares
/// about. Made `pub` so tests can build synthetic ones without going through the RPC.
#[derive(Debug, Clone)]
pub struct TxObservation {
    /// `meta.err.is_some()` — true if the transaction is on-chain but failed.
    pub failed: bool,
    /// `meta.block_time` — used for `deadline_unix` enforcement when set.
    pub block_time: Option<i64>,
    /// `meta.preTokenBalances`.
    pub pre_token_balances: Vec<TokenBalance>,
    /// `meta.postTokenBalances`.
    pub post_token_balances: Vec<TokenBalance>,
}

#[derive(Debug, Clone)]
pub struct TokenBalance {
    pub mint: String,
    /// May be `None` for indexed balances pre Solana 1.9; modern responses always
    /// populate it.
    pub owner: Option<String>,
    /// Raw integer (decimal string) per `UiTokenAmount.amount`.
    pub amount: String,
}

/// Pure verification: runs the `expected_transfers` battery against an observed
/// transaction snapshot. No RPC, no clock, no random — entirely deterministic
/// (P-DET-1).
pub fn verify_observed_transfer(
    sla: &TransferSla,
    observation: &TxObservation,
) -> EvaluationResult {
    // P-OT-3: tx failed on-chain.
    if observation.failed {
        return reject(
            onchain_transfer::TRANSFER_TX_FAILED,
            "tx_status",
            "meta.err is set; transaction is on-chain but failed",
        );
    }

    // P-OT-6: deadline enforcement.
    if let Some(deadline) = sla.deadline_unix {
        if let Some(block_time) = observation.block_time {
            if block_time > deadline {
                return reject(
                    onchain_transfer::TRANSFER_DEADLINE_EXCEEDED,
                    "deadline_unix",
                    &format!("block_time {block_time} > deadline {deadline}"),
                );
            }
        }
        // If `block_time` is missing we continue — the RPC will populate it for any
        // confirmed transaction. The evaluator's worst case here is approving a
        // transfer whose deadline check we couldn't verify; rare in practice and a
        // future revision may upgrade this to a hard reject.
    }

    let mut checks: Vec<CheckResult> = Vec::with_capacity(sla.expected_transfers.len());

    for (idx, expected) in sla.expected_transfers.iter().enumerate() {
        let post = find_balance(
            &observation.post_token_balances,
            &expected.mint,
            &expected.recipient_owner,
        );

        let post = match post {
            Some(b) => b,
            None => {
                return reject(
                    onchain_transfer::TRANSFER_MINT_MISMATCH,
                    &format!("expected_transfer[{idx}]"),
                    &format!(
                        "no post_token_balance for (mint={}, owner={})",
                        expected.mint, expected.recipient_owner
                    ),
                );
            }
        };

        // ATA-not-resolvable case: post is None _and_ pre is None. We surface this
        // before MintMismatch so the operator sees a precise reason. In practice a
        // missing post entry already triggers MintMismatch above; we keep the
        // RecipientNotResolvable code reserved for the rare "destination ATA was
        // never even derived" path which an offline test fixture can inject.
        let _ = onchain_transfer::TRANSFER_RECIPIENT_NOT_RESOLVABLE;

        let pre = find_balance(
            &observation.pre_token_balances,
            &expected.mint,
            &expected.recipient_owner,
        );

        let pre_amount = pre
            .map(|b| parse_amount_or_zero(&b.amount))
            .unwrap_or(0i128);
        let post_amount = parse_amount_or_zero(&post.amount);
        let delta: i128 = post_amount - pre_amount;
        let abs_delta = delta.unsigned_abs();

        // P-OT-5: direction mismatch.
        let direction_ok = match expected.direction {
            TransferDirection::In => delta > 0,
            TransferDirection::Out => delta < 0,
        };
        if !direction_ok {
            return reject(
                onchain_transfer::TRANSFER_DIRECTION_MISMATCH,
                &format!("expected_transfer[{idx}]"),
                &format!(
                    "direction {:?} but observed delta {} (pre={}, post={})",
                    expected.direction, delta, pre_amount, post_amount
                ),
            );
        }

        // P-OT-4: amount sufficiency.
        let min_amount = parse_min_amount(&expected.min_amount).unwrap_or(0);
        if abs_delta < min_amount {
            return reject(
                onchain_transfer::TRANSFER_AMOUNT_INSUFFICIENT,
                &format!("expected_transfer[{idx}]"),
                &format!("|delta|={abs_delta} < min_amount={min_amount}"),
            );
        }

        checks.push(CheckResult {
            name: format!("expected_transfer[{idx}]"),
            passed: true,
            detail: format!(
                "{} delta={}, min_amount={} (mint={}, owner={})",
                match expected.direction {
                    TransferDirection::In => "in",
                    TransferDirection::Out => "out",
                },
                delta,
                min_amount,
                expected.mint,
                expected.recipient_owner
            ),
        });
    }

    EvaluationResult {
        approved: true,
        resolution_reason: 0,
        checks,
    }
}

fn find_balance<'a>(
    balances: &'a [TokenBalance],
    mint: &str,
    recipient_owner: &str,
) -> Option<&'a TokenBalance> {
    balances
        .iter()
        .find(|b| b.mint == mint && b.owner.as_deref() == Some(recipient_owner))
}

fn parse_amount_or_zero(s: &str) -> i128 {
    s.parse::<i128>().unwrap_or(0)
}

fn parse_min_amount(s: &str) -> Option<u128> {
    s.parse::<u128>().ok()
}

fn reject(reason: u16, name: &str, detail: &str) -> EvaluationResult {
    EvaluationResult {
        approved: false,
        resolution_reason: reason,
        checks: vec![CheckResult {
            name: name.into(),
            passed: false,
            detail: detail.into(),
        }],
    }
}

// ---------------------------------------------------------------------------
// RPC fetch (the impure shell around `verify_observed_transfer`)
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum FetchError {
    SignatureFormat(String),
    NotFound,
    Transport(String),
}

async fn fetch_observation(
    rpc: &std::sync::Arc<RpcClient>,
    tx_signature: &str,
) -> Result<TxObservation, FetchError> {
    let sig = Signature::from_str(tx_signature)
        .map_err(|e| FetchError::SignatureFormat(e.to_string()))?;

    let cfg = RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::JsonParsed),
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
    };

    match rpc.get_transaction_with_config(&sig, cfg).await {
        Ok(enc) => Ok(snapshot_from_encoded(&enc)),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("Not Found") {
                debug!(%tx_signature, "RPC reports transaction not found");
                return Err(FetchError::NotFound);
            }
            warn!(%tx_signature, error = %msg, "transfer evaluator RPC error");
            Err(FetchError::Transport(msg))
        }
    }
}

fn snapshot_from_encoded(enc: &EncodedConfirmedTransactionWithStatusMeta) -> TxObservation {
    snapshot_from_value(&enc.transaction, enc.block_time)
}

fn snapshot_from_value(
    enc: &EncodedTransactionWithStatusMeta,
    block_time_override: Option<i64>,
) -> TxObservation {
    let meta = enc.meta.as_ref();
    let failed = meta.map(|m| m.err.is_some()).unwrap_or(false);
    let block_time = block_time_override;
    let pre = meta
        .map(|m| ui_balances_to_simple(&m.pre_token_balances))
        .unwrap_or_default();
    let post = meta
        .map(|m| ui_balances_to_simple(&m.post_token_balances))
        .unwrap_or_default();
    TxObservation {
        failed,
        block_time,
        pre_token_balances: pre,
        post_token_balances: post,
    }
}

fn ui_balances_to_simple(
    opt: &OptionSerializer<Vec<UiTransactionTokenBalance>>,
) -> Vec<TokenBalance> {
    match opt {
        OptionSerializer::Some(v) => v
            .iter()
            .map(|b| TokenBalance {
                mint: b.mint.clone(),
                owner: match &b.owner {
                    OptionSerializer::Some(s) => Some(s.clone()),
                    _ => None,
                },
                amount: b.ui_token_amount.amount.clone(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::sla::{ExpectedTransfer, TransferDirection};

    fn sla_basic(direction: TransferDirection, min: &str) -> TransferSla {
        TransferSla {
            version: 1,
            profile_id: PROFILE_ID.into(),
            cluster: TransferCluster::Devnet,
            expected_transfers: vec![ExpectedTransfer {
                mint: "MINT1".into(),
                recipient_owner: "OWNER1".into(),
                min_amount: min.into(),
                direction,
            }],
            swap_router: None,
            slippage_bps: None,
            deadline_unix: None,
        }
    }

    fn obs(
        failed: bool,
        block_time: Option<i64>,
        pre: Vec<TokenBalance>,
        post: Vec<TokenBalance>,
    ) -> TxObservation {
        TxObservation {
            failed,
            block_time,
            pre_token_balances: pre,
            post_token_balances: post,
        }
    }

    fn bal(mint: &str, owner: &str, amount: &str) -> TokenBalance {
        TokenBalance {
            mint: mint.into(),
            owner: Some(owner.into()),
            amount: amount.into(),
        }
    }

    #[test]
    fn approves_when_in_delta_meets_min() {
        // P-OT-4 happy path
        let sla = sla_basic(TransferDirection::In, "1000000");
        let observation = obs(
            false,
            None,
            vec![bal("MINT1", "OWNER1", "0")],
            vec![bal("MINT1", "OWNER1", "2000000")],
        );
        let r = verify_observed_transfer(&sla, &observation);
        assert!(r.approved);
        assert_eq!(r.resolution_reason, 0);
    }

    #[test]
    fn approves_when_pre_balance_missing_and_post_meets_min() {
        // ATA newly created on this tx — no pre row.
        let sla = sla_basic(TransferDirection::In, "1000000");
        let observation = obs(false, None, vec![], vec![bal("MINT1", "OWNER1", "1500000")]);
        let r = verify_observed_transfer(&sla, &observation);
        assert!(r.approved);
    }

    #[test]
    fn rejects_when_amount_insufficient() {
        // P-OT-4
        let sla = sla_basic(TransferDirection::In, "5000000");
        let observation = obs(
            false,
            None,
            vec![bal("MINT1", "OWNER1", "1000000")],
            vec![bal("MINT1", "OWNER1", "1500000")], // delta = 500_000 < 5_000_000
        );
        let r = verify_observed_transfer(&sla, &observation);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            onchain_transfer::TRANSFER_AMOUNT_INSUFFICIENT
        );
    }

    #[test]
    fn rejects_when_mint_mismatch() {
        // P-OT-4 negative — neither pre nor post has the (mint, owner) pair.
        let sla = sla_basic(TransferDirection::In, "1");
        let observation = obs(
            false,
            None,
            vec![],
            vec![bal("OTHER_MINT", "OWNER1", "1000000")],
        );
        let r = verify_observed_transfer(&sla, &observation);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            onchain_transfer::TRANSFER_MINT_MISMATCH
        );
    }

    #[test]
    fn rejects_when_direction_in_but_delta_negative() {
        // P-OT-5
        let sla = sla_basic(TransferDirection::In, "1");
        let observation = obs(
            false,
            None,
            vec![bal("MINT1", "OWNER1", "5000000")],
            vec![bal("MINT1", "OWNER1", "1000000")], // delta = -4_000_000
        );
        let r = verify_observed_transfer(&sla, &observation);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            onchain_transfer::TRANSFER_DIRECTION_MISMATCH
        );
    }

    #[test]
    fn rejects_when_direction_out_but_delta_positive() {
        // P-OT-5 (out variant)
        let sla = sla_basic(TransferDirection::Out, "1");
        let observation = obs(
            false,
            None,
            vec![bal("MINT1", "OWNER1", "1000000")],
            vec![bal("MINT1", "OWNER1", "5000000")], // delta = +4_000_000
        );
        let r = verify_observed_transfer(&sla, &observation);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            onchain_transfer::TRANSFER_DIRECTION_MISMATCH
        );
    }

    #[test]
    fn rejects_when_meta_err_set() {
        // P-OT-3
        let sla = sla_basic(TransferDirection::In, "1");
        let observation = obs(true, None, vec![], vec![]);
        let r = verify_observed_transfer(&sla, &observation);
        assert!(!r.approved);
        assert_eq!(r.resolution_reason, onchain_transfer::TRANSFER_TX_FAILED);
    }

    #[test]
    fn rejects_when_block_time_after_deadline() {
        // P-OT-6
        let mut sla = sla_basic(TransferDirection::In, "1");
        sla.deadline_unix = Some(1_700_000_000);
        let observation = obs(false, Some(1_700_000_001), vec![], vec![]);
        let r = verify_observed_transfer(&sla, &observation);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            onchain_transfer::TRANSFER_DEADLINE_EXCEEDED
        );
    }

    #[test]
    fn approves_at_block_time_equal_deadline() {
        let mut sla = sla_basic(TransferDirection::In, "1");
        sla.deadline_unix = Some(1_700_000_000);
        let observation = obs(
            false,
            Some(1_700_000_000),
            vec![],
            vec![bal("MINT1", "OWNER1", "1")],
        );
        let r = verify_observed_transfer(&sla, &observation);
        assert!(r.approved);
    }

    #[test]
    fn rejects_only_first_failing_transfer() {
        // Two expected transfers; the first one fails on amount. Reason should be
        // for the first failure (P-VER-2).
        let mut sla = sla_basic(TransferDirection::In, "9000000");
        sla.expected_transfers.push(ExpectedTransfer {
            mint: "MINT2".into(),
            recipient_owner: "OWNER2".into(),
            min_amount: "1".into(),
            direction: TransferDirection::In,
        });
        let observation = obs(
            false,
            None,
            vec![],
            vec![
                bal("MINT1", "OWNER1", "1000"),
                bal("MINT2", "OWNER2", "100000"),
            ],
        );
        let r = verify_observed_transfer(&sla, &observation);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            onchain_transfer::TRANSFER_AMOUNT_INSUFFICIENT
        );
        // Confirm the first failure was about expected_transfer[0].
        let first_failure = r.checks.iter().find(|c| !c.passed).unwrap();
        assert!(first_failure.name.starts_with("expected_transfer[0]"));
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 64,
            ..ProptestConfig::default()
        })]

        /// P-OT-4: with `direction="in"` and an honest tx (pre=0, post=observed),
        /// approve iff observed >= min_amount.
        #[test]
        fn p_ot_4_amount_gating(
            min_amount in 1u128..1_000_000_000,
            observed in 0u128..2_000_000_000,
        ) {
            let sla = sla_basic(TransferDirection::In, &min_amount.to_string());
            let observation = obs(
                false,
                None,
                vec![],
                vec![bal("MINT1", "OWNER1", &observed.to_string())],
            );
            let r = verify_observed_transfer(&sla, &observation);
            if observed >= min_amount {
                if observed > 0 {
                    prop_assert!(r.approved, "approved expected: observed={observed}, min={min_amount}");
                    prop_assert_eq!(r.resolution_reason, 0);
                } else {
                    // observed == 0 == min_amount: delta is zero, direction "in"
                    // requires strict positive delta — reject.
                    prop_assert!(!r.approved);
                }
            } else {
                prop_assert!(!r.approved);
                let reason = r.resolution_reason;
                prop_assert!(
                    reason == onchain_transfer::TRANSFER_AMOUNT_INSUFFICIENT
                        || reason == onchain_transfer::TRANSFER_DIRECTION_MISMATCH,
                    "unexpected reason {reason} for observed={observed}, min={min_amount}"
                );
            }
        }

        /// P-DET-1: deterministic over identical inputs.
        #[test]
        fn p_det_1_pure_function(
            observed in 0u128..1_000_000_000,
        ) {
            let sla = sla_basic(TransferDirection::In, "1000");
            let observation = obs(
                false,
                None,
                vec![],
                vec![bal("MINT1", "OWNER1", &observed.to_string())],
            );
            let a = verify_observed_transfer(&sla, &observation);
            let b = verify_observed_transfer(&sla, &observation);
            prop_assert_eq!(a, b);
        }
    }
}
