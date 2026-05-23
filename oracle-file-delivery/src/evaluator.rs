//! `FileDeliveryEvaluator` — attestation-only file-delivery checks.
//!
//! Three checks in fixed order:
//!
//! 1. size in `[expected_size_bytes_min, expected_size_bytes_max]` → reject `Custom(320)`
//! 2. (if `expected_mime` set) sniffed MIME equals `expected_mime` (case-insensitive) → reject `Custom(321)`
//! 3. (if `attestor_pubkey` set) optional Ed25519 signature verifies → reject `Custom(322)`
//!
//! Truncation / partial fetch is surfaced as `Custom(323)` from the fetcher path
//! (P-FD-* series).

use async_trait::async_trait;
use oracle_common::{
    error::OracleError,
    evaluator::{EvaluationContext, OracleEvaluator},
    resolution_codes::file_delivery,
    types::{CheckResult, EvaluationResult, EvidenceKey},
};

use crate::{evidence::FileDeliveryEvidence, sla::FileDeliverySla, PROFILE_ID};

#[derive(Default, Clone, Copy)]
pub struct FileDeliveryEvaluator;

impl FileDeliveryEvaluator {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl OracleEvaluator for FileDeliveryEvaluator {
    type Sla = FileDeliverySla;
    type Evidence = FileDeliveryEvidence;

    fn profile_id(&self) -> &'static str {
        PROFILE_ID
    }

    async fn evaluate(
        &self,
        ctx: &EvaluationContext<'_>,
        sla: &Self::Sla,
        evidence: &Self::Evidence,
    ) -> Result<EvaluationResult, OracleError> {
        let mut checks = Vec::new();

        // 1. size bounds (P-FD-1)
        let size_ok = evidence.size_bytes >= sla.expected_size_bytes_min
            && evidence.size_bytes <= sla.expected_size_bytes_max;
        checks.push(CheckResult {
            name: "size".into(),
            passed: size_ok,
            detail: format!(
                "{} bytes (min {}, max {})",
                evidence.size_bytes, sla.expected_size_bytes_min, sla.expected_size_bytes_max
            ),
        });

        // 2. MIME match (P-FD-2)
        if let Some(want_mime) = sla.expected_mime.as_deref() {
            let got = evidence.sniffed_mime.as_deref().unwrap_or("");
            let mime_ok = got.eq_ignore_ascii_case(want_mime) || got.starts_with(want_mime);
            checks.push(CheckResult {
                name: "mime".into(),
                passed: mime_ok,
                detail: format!("sniffed='{got}' expected='{want_mime}'"),
            });
        }

        // 3. attestor signature is OPTIONAL in v1; the SLA indicates it's expected
        //    but the streaming evidence does not yet carry one. The check is recorded
        //    as failed iff `attestor_pubkey` is set (per design.md §Property 23).
        if sla.attestor_pubkey.is_some() {
            checks.push(CheckResult {
                name: "attestor_signature".into(),
                passed: false,
                detail: "attestor_pubkey set but the v1 streaming-evidence path does not carry signatures; use a future signed-delivery profile".into(),
            });
        }

        let approved = checks.iter().all(|c| c.passed);
        let resolution_reason: u16 = if approved {
            0
        } else {
            checks
                .iter()
                .find(|c| !c.passed)
                .map(|c| match c.name.as_str() {
                    "size" => file_delivery::BLOB_SIZE_OUT_OF_RANGE,
                    "mime" => file_delivery::BLOB_MIME_MISMATCH,
                    "attestor_signature" => file_delivery::BLOB_ATTESTOR_SIGNATURE_INVALID,
                    "delivery_hash_replay" => file_delivery::BLOB_DELIVERY_HASH_REUSED,
                    _ => 255,
                })
                .unwrap_or(255)
        };

        // Wave A §1.3 cross-payment replay refusal: if all attestation checks
        // pass, probe the ledger for any *other* `payment_uid` that already
        // settled against the same `delivery_hash`. Detection means a seller is
        // reusing a single uploaded blob to settle multiple payments — refuse
        // with `BLOB_DELIVERY_HASH_REUSED`.
        if approved {
            if let Some(ledger) = ctx.ledger {
                match ledger
                    .evidence_key_settled_for_other_payment(
                        &ctx.job.payment_uid,
                        "delivery_hash",
                        &evidence.blob_sha256_hex,
                    )
                    .await
                {
                    Ok(true) => {
                        checks.push(CheckResult {
                            name: "delivery_hash_replay".into(),
                            passed: false,
                            detail: format!(
                                "delivery_hash {} was already settled for a different payment_uid; cross-payment replay refused",
                                evidence.blob_sha256_hex
                            ),
                        });
                        return Ok(EvaluationResult {
                            approved: false,
                            resolution_reason: file_delivery::BLOB_DELIVERY_HASH_REUSED,
                            checks,
                            resolution_details: None,
                        });
                    }
                    Ok(false) => {}
                    Err(e) => {
                        // Ledger unreachable: surface as worker-level error so
                        // the job retries rather than approving without the
                        // check.
                        return Err(e);
                    }
                }
            }
        }

        Ok(EvaluationResult {
            approved,
            resolution_reason,
            checks,
            resolution_details: None,
        })
    }

    fn evidence_keys(&self, _sla: &Self::Sla, evidence: &Self::Evidence) -> Vec<EvidenceKey> {
        // The blob's content-addressed hash is the unique key for cross-payment
        // replay protection.
        vec![EvidenceKey {
            kind: "delivery_hash".into(),
            value: evidence.blob_sha256_hex.clone(),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sla_basic() -> FileDeliverySla {
        FileDeliverySla {
            version: 1,
            profile_id: PROFILE_ID.into(),
            payment_uid: "00".repeat(32),
            buyer_nonce: None,
            expected_size_bytes_min: 1_000,
            expected_size_bytes_max: 1_000_000,
            expected_mime: Some("video/mp4".into()),
            expected_extension: None,
            attestor_pubkey: None,
        }
    }

    fn evidence_with(size: u64, mime: Option<&str>) -> FileDeliveryEvidence {
        FileDeliveryEvidence {
            size_bytes: size,
            sniffed_mime: mime.map(|s| s.to_string()),
            blob_sha256_hex: "00".repeat(32),
        }
    }

    fn rpc() -> std::sync::Arc<solana_client::nonblocking::rpc_client::RpcClient> {
        std::sync::Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(
            "http://127.0.0.1:8899".into(),
        ))
    }

    #[tokio::test]
    async fn approves_within_size_and_mime() {
        let rpc = rpc();
        let http = reqwest::Client::builder().build().unwrap();
        let job = oracle_common::types::EvaluationJob {
            payment_uid: [0u8; 32],
            payment_pubkey: solana_sdk::pubkey::Pubkey::new_unique(),
            sla_hash: [0u8; 32],
            delivery_hash: [0u8; 32],
            amount: 0,
            mint: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_authority: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_fee_bps: 100,
            expires_at: 0,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        };
        let ctx = EvaluationContext {
            rpc: &rpc,
            http: &http,
            job: &job,
            strict: true,
            ledger: None,
        };

        let r = FileDeliveryEvaluator::new()
            .evaluate(
                &ctx,
                &sla_basic(),
                &evidence_with(50_000, Some("video/mp4")),
            )
            .await
            .unwrap();
        assert!(r.approved);
        assert_eq!(r.resolution_reason, 0);
    }

    #[tokio::test]
    async fn rejects_size_out_of_range() {
        let rpc = rpc();
        let http = reqwest::Client::builder().build().unwrap();
        let job = oracle_common::types::EvaluationJob {
            payment_uid: [0u8; 32],
            payment_pubkey: solana_sdk::pubkey::Pubkey::new_unique(),
            sla_hash: [0u8; 32],
            delivery_hash: [0u8; 32],
            amount: 0,
            mint: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_authority: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_fee_bps: 100,
            expires_at: 0,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        };
        let ctx = EvaluationContext {
            rpc: &rpc,
            http: &http,
            job: &job,
            strict: true,
            ledger: None,
        };

        let r = FileDeliveryEvaluator::new()
            .evaluate(&ctx, &sla_basic(), &evidence_with(50, Some("video/mp4")))
            .await
            .unwrap();
        assert!(!r.approved);
        assert_eq!(r.resolution_reason, file_delivery::BLOB_SIZE_OUT_OF_RANGE);
    }

    #[tokio::test]
    async fn rejects_mime_mismatch() {
        let rpc = rpc();
        let http = reqwest::Client::builder().build().unwrap();
        let job = oracle_common::types::EvaluationJob {
            payment_uid: [0u8; 32],
            payment_pubkey: solana_sdk::pubkey::Pubkey::new_unique(),
            sla_hash: [0u8; 32],
            delivery_hash: [0u8; 32],
            amount: 0,
            mint: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_authority: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_fee_bps: 100,
            expires_at: 0,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        };
        let ctx = EvaluationContext {
            rpc: &rpc,
            http: &http,
            job: &job,
            strict: true,
            ledger: None,
        };

        let r = FileDeliveryEvaluator::new()
            .evaluate(
                &ctx,
                &sla_basic(),
                &evidence_with(50_000, Some("text/plain")),
            )
            .await
            .unwrap();
        assert!(!r.approved);
        assert_eq!(r.resolution_reason, file_delivery::BLOB_MIME_MISMATCH);
    }

    // -------------------------------------------------------------------------
    // Wave A §1.3 — delivery-hash uniqueness (cross-payment replay refusal)
    // -------------------------------------------------------------------------

    /// Stub `LedgerProbe` that returns a fixed answer.
    struct StubLedger {
        already_settled: bool,
    }

    #[async_trait::async_trait]
    impl oracle_common::evaluator::LedgerProbe for StubLedger {
        async fn evidence_key_settled_for_other_payment(
            &self,
            _current_uid: &[u8; 32],
            _key_kind: &str,
            _key_value: &str,
        ) -> Result<bool, oracle_common::error::OracleError> {
            Ok(self.already_settled)
        }
    }

    #[tokio::test]
    async fn rejects_when_delivery_hash_already_settled_for_other_payment() {
        let rpc = rpc();
        let http = reqwest::Client::builder().build().unwrap();
        let job = oracle_common::types::EvaluationJob {
            payment_uid: [0u8; 32],
            payment_pubkey: solana_sdk::pubkey::Pubkey::new_unique(),
            sla_hash: [0u8; 32],
            delivery_hash: [0u8; 32],
            amount: 0,
            mint: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_authority: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_fee_bps: 100,
            expires_at: 0,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        };
        let ledger: std::sync::Arc<dyn oracle_common::evaluator::LedgerProbe> =
            std::sync::Arc::new(StubLedger {
                already_settled: true,
            });
        let ctx = EvaluationContext {
            rpc: &rpc,
            http: &http,
            job: &job,
            strict: true,
            ledger: Some(&ledger),
        };
        let r = FileDeliveryEvaluator::new()
            .evaluate(
                &ctx,
                &sla_basic(),
                &evidence_with(50_000, Some("video/mp4")),
            )
            .await
            .unwrap();
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            file_delivery::BLOB_DELIVERY_HASH_REUSED
        );
    }

    #[tokio::test]
    async fn approves_when_delivery_hash_is_fresh() {
        let rpc = rpc();
        let http = reqwest::Client::builder().build().unwrap();
        let job = oracle_common::types::EvaluationJob {
            payment_uid: [0u8; 32],
            payment_pubkey: solana_sdk::pubkey::Pubkey::new_unique(),
            sla_hash: [0u8; 32],
            delivery_hash: [0u8; 32],
            amount: 0,
            mint: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_authority: solana_sdk::pubkey::Pubkey::new_unique(),
            oracle_fee_bps: 100,
            expires_at: 0,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        };
        let ledger: std::sync::Arc<dyn oracle_common::evaluator::LedgerProbe> =
            std::sync::Arc::new(StubLedger {
                already_settled: false,
            });
        let ctx = EvaluationContext {
            rpc: &rpc,
            http: &http,
            job: &job,
            strict: true,
            ledger: Some(&ledger),
        };
        let r = FileDeliveryEvaluator::new()
            .evaluate(
                &ctx,
                &sla_basic(),
                &evidence_with(50_000, Some("video/mp4")),
            )
            .await
            .unwrap();
        assert!(r.approved);
    }

    #[test]
    fn evidence_keys_returns_delivery_hash() {
        let evaluator = FileDeliveryEvaluator::new();
        let evidence = evidence_with(1234, Some("video/mp4"));
        let keys = OracleEvaluator::evidence_keys(&evaluator, &sla_basic(), &evidence);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].kind, "delivery_hash");
        assert_eq!(keys[0].value, "00".repeat(32));
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 64,
            ..proptest::test_runner::Config::default()
        })]

        /// P-FD-1: for any blob byte size and SLA `[min, max]` range, the size
        /// check approves iff `min ≤ size ≤ max`.
        #[test]
        fn p_fd_1_size_gating(
            min in 1u64..1_000_000,
            max in 1u64..2_000_000,
            size in 0u64..3_000_000,
        ) {
            use proptest::prop_assert_eq;

            let min = min.min(max);
            let max = max.max(min);

            let mut sla = sla_basic();
            sla.expected_size_bytes_min = min;
            sla.expected_size_bytes_max = max;
            sla.expected_mime = None; // remove MIME from the universe

            let evidence = evidence_with(size, None);

            let rpc = rpc();
            let http = reqwest::Client::builder().build().unwrap();
            let job = oracle_common::types::EvaluationJob {
                payment_uid: [0u8; 32],
                payment_pubkey: solana_sdk::pubkey::Pubkey::new_unique(),
                sla_hash: [0u8; 32],
                delivery_hash: [0u8; 32],
                amount: 0,
                mint: solana_sdk::pubkey::Pubkey::new_unique(),
                oracle_authority: solana_sdk::pubkey::Pubkey::new_unique(),
                oracle_fee_bps: 100,
                expires_at: 0,
                created_at: 0,
                delivery_cutoff_seconds: 0,
                sla_bytes: None,
            retry_count: 0,
            };
            let ctx = EvaluationContext { rpc: &rpc, http: &http, job: &job, strict: true, ledger: None };

            // Synchronous-ish: the evaluator's body is a sync computation; we still
            // need a runtime for the `async` trait method.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let r = rt.block_on(async {
                FileDeliveryEvaluator::new()
                    .evaluate(&ctx, &sla, &evidence)
                    .await
                    .unwrap()
            });

            let in_range = size >= min && size <= max;
            prop_assert_eq!(r.approved, in_range);
            if !in_range {
                prop_assert_eq!(r.resolution_reason, file_delivery::BLOB_SIZE_OUT_OF_RANGE);
            }
        }
    }
}
