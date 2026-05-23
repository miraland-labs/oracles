//! Generic evaluation pipeline.
//!
//! Reads `profile_id` from the SLA bytes that the chain monitor pre-fetched, looks
//! the runner up in the [`crate::profile::ProfileRegistry`], and runs the typed
//! evaluator-and-fetcher pair. Non-existent profile ids are refused at dispatch with
//! `OracleError::UnknownProfile`. Settlement is invoked separately by the worker so
//! the pipeline returns a settle-ready outcome (verdict + 32-byte resolution hash);
//! see `worker::run_worker` in `lib.rs` for the full lifecycle.

use bytes::Bytes;
use tracing::info;

use crate::{
    error::OracleError,
    evaluator::EvaluationContext,
    profile::ProfileRegistry,
    types::{EvaluationOutcome, SlaEnvelope},
};

/// Outcome of one pipeline run. The signature is filled in by the worker after
/// `settler::settle` succeeds — the pipeline itself doesn't touch the chain.
pub type PipelineOutcome = EvaluationOutcome;

/// Run one job through the dispatch + evaluator + resolution-hash pipeline.
///
/// Inputs:
/// * `profiles` — the profile registry the binary built at startup.
/// * `ctx` — borrowed cross-cutting concerns (RPC, HTTP, the job, strict flag).
/// * `prefetched_sla` — the bytes the chain monitor attached to `EvaluationJob`.
///   The dispatcher reads `profile_id` from these bytes; the family runner is
///   responsible for parsing the full SLA via its own fetcher.
pub async fn run_pipeline(
    profiles: &ProfileRegistry,
    ctx: &EvaluationContext<'_>,
    prefetched_sla: Option<&Bytes>,
) -> Result<PipelineOutcome, OracleError> {
    let uid_hex = hex::encode(ctx.job.payment_uid);
    info!(payment_uid = %uid_hex, "pipeline started");

    // Dispatch: read `profile_id` from the small envelope.
    let envelope = match prefetched_sla {
        Some(b) => parse_envelope(b)?,
        None => {
            return Err(OracleError::SlaParse(
                "no SLA bytes attached to job; pipeline requires the chain monitor or manual evaluate to provide them"
                    .into(),
            ));
        }
    };

    let runner = profiles
        .resolve(&envelope.profile_id)
        .ok_or_else(|| OracleError::UnknownProfile(envelope.profile_id.clone()))?;
    info!(profile_id = %envelope.profile_id, "dispatched");

    runner.run(ctx).await
}

fn parse_envelope(bytes: &Bytes) -> Result<SlaEnvelope, OracleError> {
    serde_json::from_slice(bytes)
        .map_err(|e| OracleError::SlaParse(format!("envelope (profile_id) parse failed: {e}")))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde::{Deserialize, Serialize};
    use solana_client::nonblocking::rpc_client::RpcClient;
    use solana_sdk::pubkey::Pubkey;

    use super::*;
    use crate::{
        evaluator::OracleEvaluator,
        profile::{ProfileBinding, RegisteredProfile},
        types::{CheckResult, EvaluationJob, EvaluationResult},
    };

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestSla {
        profile_id: String,
        ok: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestEvidence {
        ok: bool,
    }

    struct StubEvaluator;

    #[async_trait]
    impl OracleEvaluator for StubEvaluator {
        type Sla = TestSla;
        type Evidence = TestEvidence;
        fn profile_id(&self) -> &'static str {
            "x402/test/v1"
        }
        async fn evaluate(
            &self,
            _ctx: &EvaluationContext<'_>,
            sla: &TestSla,
            evidence: &TestEvidence,
        ) -> Result<EvaluationResult, OracleError> {
            let approved = sla.ok && evidence.ok;
            Ok(EvaluationResult {
                approved,
                resolution_reason: if approved { 0 } else { 255 },
                checks: vec![CheckResult {
                    name: "stub".into(),
                    passed: approved,
                    detail: format!("sla.ok={} evidence.ok={}", sla.ok, evidence.ok),
                }],
                resolution_details: None,
            })
        }
    }

    /// In-memory fetcher that returns a pre-computed value.
    struct FixedFetcher<T: Clone + Send + Sync> {
        value: T,
    }

    #[async_trait]
    impl<T: Clone + Send + Sync + 'static> crate::fetcher::EvidenceFetcher for FixedFetcher<T> {
        type Output = T;
        async fn fetch(
            &self,
            _hash: &[u8; 32],
            _kind: crate::fetcher::ArtifactKind,
        ) -> Result<Self::Output, OracleError> {
            Ok(self.value.clone())
        }
    }

    fn make_job(sla_bytes: Option<Bytes>) -> EvaluationJob {
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
            sla_bytes,
            retry_count: 0,
        }
    }

    fn dummy_ctx<'a>(
        job: &'a EvaluationJob,
        rpc: &'a Arc<RpcClient>,
        http: &'a reqwest::Client,
    ) -> EvaluationContext<'a> {
        EvaluationContext {
            rpc,
            http,
            job,
            strict: true,
            ledger: None,
        }
    }

    #[tokio::test]
    async fn dispatches_to_registered_profile() {
        let evaluator = Arc::new(StubEvaluator);
        let sla_fetcher = Arc::new(FixedFetcher {
            value: TestSla {
                profile_id: "x402/test/v1".into(),
                ok: true,
            },
        });
        let evidence_fetcher = Arc::new(FixedFetcher {
            value: TestEvidence { ok: true },
        });

        let mut reg = ProfileRegistry::new();
        reg.register(RegisteredProfile {
            profile_id: "x402/test/v1",
            run: Arc::new(ProfileBinding {
                evaluator,
                sla_fetcher,
                evidence_fetcher,
            }),
        });

        let sla_bytes = Bytes::from_static(br#"{"profile_id":"x402/test/v1"}"#);
        let job = make_job(Some(sla_bytes.clone()));
        let rpc = Arc::new(RpcClient::new("http://127.0.0.1:8899".into()));
        let http = reqwest::Client::builder().build().unwrap();
        let ctx = dummy_ctx(&job, &rpc, &http);

        let outcome = run_pipeline(&reg, &ctx, Some(&sla_bytes)).await.unwrap();
        assert!(outcome.result.approved);
        assert_eq!(outcome.result.resolution_reason, 0);
        assert_eq!(outcome.signature, None);
        assert_ne!(outcome.resolution_hash, [0u8; 32]);
    }

    #[tokio::test]
    async fn refuses_unknown_profile_id() {
        let evaluator = Arc::new(StubEvaluator);
        let sla_fetcher = Arc::new(FixedFetcher {
            value: TestSla {
                profile_id: "x402/test/v1".into(),
                ok: true,
            },
        });
        let evidence_fetcher = Arc::new(FixedFetcher {
            value: TestEvidence { ok: true },
        });
        let mut reg = ProfileRegistry::new();
        reg.register(RegisteredProfile {
            profile_id: "x402/test/v1",
            run: Arc::new(ProfileBinding {
                evaluator,
                sla_fetcher,
                evidence_fetcher,
            }),
        });

        // Bytes declare a different profile id.
        let sla_bytes = Bytes::from_static(br#"{"profile_id":"x402/somethingelse/v1"}"#);
        let job = make_job(Some(sla_bytes.clone()));
        let rpc = Arc::new(RpcClient::new("http://127.0.0.1:8899".into()));
        let http = reqwest::Client::builder().build().unwrap();
        let ctx = dummy_ctx(&job, &rpc, &http);

        let res = run_pipeline(&reg, &ctx, Some(&sla_bytes)).await;
        match res {
            Err(OracleError::UnknownProfile(s)) => assert_eq!(s, "x402/somethingelse/v1"),
            Err(other) => panic!("unexpected: {other}"),
            Ok(_) => panic!("expected refusal"),
        }
    }

    #[tokio::test]
    async fn requires_prefetched_sla_bytes() {
        let reg = ProfileRegistry::new();
        let job = make_job(None);
        let rpc = Arc::new(RpcClient::new("http://127.0.0.1:8899".into()));
        let http = reqwest::Client::builder().build().unwrap();
        let ctx = dummy_ctx(&job, &rpc, &http);

        let res = run_pipeline(&reg, &ctx, None).await;
        assert!(matches!(res, Err(OracleError::SlaParse(_))));
    }
}
