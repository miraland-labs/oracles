//! `ForgeVerdictProfileRunner` — binds the file-delivery evaluator to Forge's
//! seller-side verdict path instead of the generic registry-mirror
//! `EvidenceFetcher` binding (`oracle_common::profile::ProfileBinding`).
//!
//! `ProfileBinding` only ever passes `ctx.job.delivery_hash` into the evidence
//! fetcher, which is enough for a registry blob (content-addressed by that
//! same hash) but not enough for Forge's verdict endpoint, which additionally
//! needs the `listing_id` carried in the SLA and the payment's `payment_uid`.
//! This runner fetches the SLA first (still via the registry mirror — it's a
//! small JSON document, not the delivered file), then uses
//! `sla.listing_id` + `hex(ctx.job.payment_uid)` to call
//! [`crate::fetcher::ForgeVerdictFetcher`] for the delivered-file evidence.

use std::sync::Arc;

use async_trait::async_trait;
use oracle_common::{
    error::OracleError,
    evaluator::OracleEvaluator,
    fetcher::{ArtifactKind, EvidenceFetcher, RegistryJsonFetcher},
    evaluator::EvaluationContext,
    profile::ProfileRunner,
    types::EvaluationOutcome,
};

use crate::{
    evaluator::FileDeliveryEvaluator, fetcher::ForgeVerdictFetcher, sla::FileDeliverySla,
    PROFILE_ID,
};

/// [`ProfileRunner`] for the file-delivery judge, wired to Forge's
/// seller-side verdict path for delivered-file evidence.
pub struct ForgeVerdictProfileRunner {
    pub evaluator: Arc<FileDeliveryEvaluator>,
    pub sla_fetcher: Arc<RegistryJsonFetcher<FileDeliverySla>>,
    pub verdict_fetcher: Arc<ForgeVerdictFetcher>,
}

#[async_trait]
impl ProfileRunner for ForgeVerdictProfileRunner {
    fn profile_id(&self) -> &'static str {
        PROFILE_ID
    }

    async fn run(&self, ctx: &EvaluationContext<'_>) -> Result<EvaluationOutcome, OracleError> {
        let sla = self
            .sla_fetcher
            .fetch(&ctx.job.sla_hash, ArtifactKind::Sla)
            .await?;

        let payment_uid_hex = hex::encode(ctx.job.payment_uid);
        let evidence = self
            .verdict_fetcher
            .fetch_and_verify(&sla.listing_id, &payment_uid_hex, &ctx.job.delivery_hash)
            .await?;

        let result = self.evaluator.evaluate(ctx, &sla, &evidence).await?;
        let evidence_keys = self.evaluator.evidence_keys(&sla, &evidence);
        let details = result
            .resolution_details
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        let resolution_hash = oracle_common::settler::compute_resolution_hash(
            ctx.job,
            self.evaluator.profile_id(),
            &result,
            details,
        )?;
        Ok(EvaluationOutcome {
            result,
            resolution_hash,
            signature: None,
            evidence_keys,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::{extract::Path, routing::get, Router};
    use ed25519_dalek::SigningKey;
    use oracle_common::fetcher::FetcherConfig;
    use sha2::{Digest, Sha256};
    use solana_client::nonblocking::rpc_client::RpcClient;
    use solana_sdk::pubkey::Pubkey;

    use super::*;
    use crate::fetcher::ForgeVerdictConfig;

    /// Spawn a mock registry mirror that returns `sla_bytes` for any hash
    /// path (only the SLA is fetched from the registry in this wiring).
    async fn spawn_registry(sla_bytes: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(sla_bytes);
        let app = Router::new().route(
            "/{hash}",
            get(move || {
                let body = body.clone();
                async move { body.as_ref().clone() }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// Spawn a mock Forge verdict endpoint that returns `artifact_body` for
    /// any listing id.
    async fn spawn_verdict(artifact_body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(artifact_body);
        let app = Router::new().route(
            "/api/v1/oracle/listings/{listing_id}/artifact",
            get(move |Path(_listing_id): Path<String>| {
                let body = body.clone();
                async move { body.as_ref().clone() }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn build_runner(registry_url: String, verdict_url: String) -> ForgeVerdictProfileRunner {
        let http = reqwest::Client::builder().build().unwrap();
        let fetcher_cfg = Arc::new(FetcherConfig {
            mirrors: vec![registry_url],
            auth_header: None,
            max_retries: 1,
            retry_base: Duration::from_millis(1),
        });
        let sla_fetcher = Arc::new(RegistryJsonFetcher::new(http.clone(), fetcher_cfg));
        let verdict_cfg = ForgeVerdictConfig {
            verdict_base_url: verdict_url,
            oracle_signing_key: Arc::new(SigningKey::from_bytes(&[3u8; 32])),
        };
        let verdict_fetcher = Arc::new(ForgeVerdictFetcher::new(http, verdict_cfg));
        ForgeVerdictProfileRunner {
            evaluator: Arc::new(FileDeliveryEvaluator::new()),
            sla_fetcher,
            verdict_fetcher,
        }
    }

    fn job(sla_hash: [u8; 32], delivery_hash: [u8; 32]) -> oracle_common::types::EvaluationJob {
        oracle_common::types::EvaluationJob {
            payment_uid: [5u8; 32],
            payment_pubkey: Pubkey::new_unique(),
            sla_hash,
            delivery_hash,
            amount: 1,
            mint: Pubkey::new_unique(),
            oracle_authority: Pubkey::new_unique(),
            oracle_fee_bps: 100,
            expires_at: i64::MAX,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        }
    }

    /// Startup-wiring + Forge auth/verdict flow: a delivered file whose
    /// SHA-256 matches the on-chain `delivery_hash` drives the runner to a
    /// real APPROVE, proving the evidence came from the Forge verdict path
    /// end to end (SLA fetched from the registry mirror, artifact fetched
    /// and self-verified against Forge's mock verdict endpoint).
    #[tokio::test]
    async fn real_approve_when_forge_verdict_artifact_matches_delivery_hash() {
        let artifact = vec![7u8; 4096];
        let digest = Sha256::digest(&artifact);
        let mut delivery_hash = [0u8; 32];
        delivery_hash.copy_from_slice(&digest);

        let sla = FileDeliverySla {
            version: 1,
            profile_id: PROFILE_ID.into(),
            payment_uid: hex::encode([5u8; 32]),
            listing_id: "listing-approve".into(),
            buyer_nonce: None,
            expected_size_bytes_min: 1,
            expected_size_bytes_max: 1_000_000,
            expected_mime: None,
            expected_extension: None,
            attestor_pubkey: None,
        };
        let sla_bytes = serde_json::to_vec(&sla).unwrap();
        let sla_hash: [u8; 32] = Sha256::digest(&sla_bytes).into();

        let registry_url = spawn_registry(sla_bytes).await;
        let verdict_url = spawn_verdict(artifact).await;
        let runner = build_runner(registry_url, verdict_url);

        let job = job(sla_hash, delivery_hash);
        let rpc = Arc::new(RpcClient::new("http://127.0.0.1:8899".into()));
        let http = reqwest::Client::new();
        let ctx = EvaluationContext {
            rpc: &rpc,
            http: &http,
            job: &job,
            strict: true,
            ledger: None,
        };

        let outcome = runner.run(&ctx).await.expect("run");
        assert!(outcome.result.approved, "checks: {:?}", outcome.result.checks);
        assert_eq!(outcome.result.resolution_reason, 0);
    }

    /// Deliberate reject: the artifact Forge's verdict endpoint returns does
    /// not hash to the on-chain `delivery_hash` the buyer funded escrow
    /// against (a wrong/tampered delivery). The runner must surface this as
    /// an error (fail-closed at the fetch boundary), never a false approve.
    #[tokio::test]
    async fn deliberate_reject_when_forge_verdict_artifact_does_not_match_delivery_hash() {
        let wrong_artifact = vec![9u8; 2048];
        // Promise a digest that does NOT match `wrong_artifact`.
        let delivery_hash = [1u8; 32];

        let sla = FileDeliverySla {
            version: 1,
            profile_id: PROFILE_ID.into(),
            payment_uid: hex::encode([5u8; 32]),
            listing_id: "listing-reject".into(),
            buyer_nonce: None,
            expected_size_bytes_min: 1,
            expected_size_bytes_max: 1_000_000,
            expected_mime: None,
            expected_extension: None,
            attestor_pubkey: None,
        };
        let sla_bytes = serde_json::to_vec(&sla).unwrap();
        let sla_hash: [u8; 32] = Sha256::digest(&sla_bytes).into();

        let registry_url = spawn_registry(sla_bytes).await;
        let verdict_url = spawn_verdict(wrong_artifact).await;
        let runner = build_runner(registry_url, verdict_url);

        let job = job(sla_hash, delivery_hash);
        let rpc = Arc::new(RpcClient::new("http://127.0.0.1:8899".into()));
        let http = reqwest::Client::new();
        let ctx = EvaluationContext {
            rpc: &rpc,
            http: &http,
            job: &job,
            strict: true,
            ledger: None,
        };

        let err = runner.run(&ctx).await.unwrap_err();
        match err {
            OracleError::EvidenceNotFound(msg) => {
                assert!(msg.contains("Hash mismatch"), "got: {msg}");
            }
            other => panic!("unexpected: {other}"),
        }
    }

    #[test]
    fn runner_reports_the_canonical_profile_id() {
        // Cheap startup-wiring assertion: the runner (as registered into
        // `ProfileRegistry` at boot) must dispatch under the exact profile id
        // the SLA declares, per P-DISP-1 (no aliases).
        let http = reqwest::Client::builder().build().unwrap();
        let fetcher_cfg = Arc::new(FetcherConfig {
            mirrors: vec!["http://127.0.0.1:0".into()],
            auth_header: None,
            max_retries: 1,
            retry_base: Duration::from_millis(1),
        });
        let sla_fetcher = Arc::new(RegistryJsonFetcher::new(http.clone(), fetcher_cfg));
        let verdict_cfg = ForgeVerdictConfig {
            verdict_base_url: "http://127.0.0.1:0".into(),
            oracle_signing_key: Arc::new(SigningKey::from_bytes(&[3u8; 32])),
        };
        let verdict_fetcher = Arc::new(ForgeVerdictFetcher::new(http, verdict_cfg));
        let runner = ForgeVerdictProfileRunner {
            evaluator: Arc::new(FileDeliveryEvaluator::new()),
            sla_fetcher,
            verdict_fetcher,
        };
        assert_eq!(runner.profile_id(), PROFILE_ID);
    }
}
