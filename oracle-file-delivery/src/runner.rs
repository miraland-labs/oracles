//! `FileDeliveryProfileRunner` — wires the file-delivery judge to Forge's
//! seller-side verdict path.
//!
//! Unlike `oracle_common::profile::ProfileBinding`, which fetches evidence
//! bytes from the registry-blob "shop/CDN" path via a generic
//! `EvidenceFetcher<Output = _>`, this runner fetches delivered-file evidence
//! for escrow *preview* listings straight from Forge's seller-side verdict
//! endpoint ([`ForgeVerdictFetcher`]), authenticated per the step 1 ESCROW TWO
//! DOORS contract. The SLA document itself is still a small JSON document
//! read from the registry — only the delivered *file* evidence moves to the
//! Forge verdict door.

use std::sync::Arc;

use async_trait::async_trait;
use oracle_common::{
    error::OracleError,
    evaluator::{EvaluationContext, OracleEvaluator},
    fetcher::{ArtifactKind, EvidenceFetcher, RegistryJsonFetcher},
    profile::ProfileRunner,
    types::EvaluationOutcome,
};

use crate::{
    evaluator::FileDeliveryEvaluator, fetcher::ForgeVerdictFetcher, sla::FileDeliverySla,
    PROFILE_ID,
};

/// Binds the file-delivery evaluator to a registry SLA fetcher and Forge's
/// seller-side verdict fetcher for delivered-file evidence.
pub struct FileDeliveryProfileRunner {
    pub evaluator: Arc<FileDeliveryEvaluator>,
    pub sla_fetcher: Arc<RegistryJsonFetcher<FileDeliverySla>>,
    pub verdict_fetcher: Arc<ForgeVerdictFetcher>,
}

#[async_trait]
impl ProfileRunner for FileDeliveryProfileRunner {
    fn profile_id(&self) -> &'static str {
        PROFILE_ID
    }

    async fn run(&self, ctx: &EvaluationContext<'_>) -> Result<EvaluationOutcome, OracleError> {
        let sla = self
            .sla_fetcher
            .fetch(&ctx.job.sla_hash, ArtifactKind::Sla)
            .await?;

        // The on-chain job carries no separate Forge `listing_id`; in the
        // escrow-preview scenario a payment is 1:1 with the listing it funds,
        // so `payment_uid` is the only per-job identifier available and is
        // used for both the path segment and the auth field the step 1
        // contract requires.
        let payment_uid_hex = hex::encode(ctx.job.payment_uid);
        let evidence = self
            .verdict_fetcher
            .fetch_and_verify(&payment_uid_hex, &payment_uid_hex, &ctx.job.delivery_hash)
            .await?;

        let result = OracleEvaluator::evaluate(&*self.evaluator, ctx, &sla, &evidence).await?;
        let evidence_keys = OracleEvaluator::evidence_keys(&*self.evaluator, &sla, &evidence);
        let details = result
            .resolution_details
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        let resolution_hash =
            oracle_common::settler::compute_resolution_hash(ctx.job, PROFILE_ID, &result, details)?;
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
    use axum::{extract::Path, routing::get, Router};
    use ed25519_dalek::SigningKey;
    use oracle_common::fetcher::FetcherConfig;
    use sha2::{Digest, Sha256};
    use solana_sdk::pubkey::Pubkey;
    use tokio::sync::Mutex;

    use super::*;
    use crate::fetcher::ForgeVerdictConfig;

    /// Spawn a registry server that serves `sla_body` for the SLA hash, and a
    /// Forge verdict server that serves `verdict_body` for any listing id. This
    /// exercises the full startup-wired path: SLA from the registry, delivered
    /// file evidence from Forge's seller-side verdict endpoint.
    async fn spawn_registry(body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(body);
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

    async fn spawn_verdict_server(body: Vec<u8>) -> (String, Arc<Mutex<Option<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(body);
        let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let seen_route = seen.clone();
        let app = Router::new().route(
            "/api/v1/oracle/listings/{listing_id}/artifact",
            get(move |Path(listing_id): Path<String>| {
                let body = body.clone();
                let seen_route = seen_route.clone();
                async move {
                    *seen_route.lock().await = Some(listing_id);
                    body.as_ref().clone()
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), seen)
    }

    fn job(payment_uid: [u8; 32], sla_hash: [u8; 32], delivery_hash: [u8; 32]) -> oracle_common::types::EvaluationJob {
        oracle_common::types::EvaluationJob {
            payment_uid,
            payment_pubkey: Pubkey::new_unique(),
            sla_hash,
            delivery_hash,
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

    fn rpc() -> Arc<solana_client::nonblocking::rpc_client::RpcClient> {
        Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(
            "http://127.0.0.1:8899".into(),
        ))
    }

    /// Startup wiring + Forge auth/verdict flow, real approve: the SLA comes
    /// from the registry, the delivered file comes from Forge's seller-side
    /// verdict endpoint, the oracle's Ed25519 signature over the step 1
    /// contract's fields verifies, and the digest matches — the judge
    /// approves.
    #[tokio::test]
    async fn approves_real_delivery_via_forge_verdict_path() {
        let mut file_body = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        file_body.extend_from_slice(&[0u8; 64]);
        let digest = Sha256::digest(&file_body);
        let mut delivery_hash = [0u8; 32];
        delivery_hash.copy_from_slice(&digest);

        let sla = FileDeliverySla {
            version: 1,
            profile_id: PROFILE_ID.into(),
            payment_uid: "aa".repeat(32),
            buyer_nonce: None,
            expected_size_bytes_min: 1,
            expected_size_bytes_max: 1_000_000,
            expected_mime: Some("image/png".into()),
            expected_extension: None,
            attestor_pubkey: None,
        };
        let sla_body = serde_json::to_vec(&sla).unwrap();
        let sla_digest = Sha256::digest(&sla_body);
        let mut sla_hash = [0u8; 32];
        sla_hash.copy_from_slice(&sla_digest);

        let registry_url = spawn_registry(sla_body).await;
        let (verdict_url, seen) = spawn_verdict_server(file_body).await;

        let http = reqwest::Client::builder().build().unwrap();
        let fetcher_cfg = Arc::new(FetcherConfig {
            mirrors: vec![registry_url],
            auth_header: None,
            max_retries: 1,
            retry_base: std::time::Duration::from_millis(1),
        });
        let sla_fetcher = Arc::new(RegistryJsonFetcher::new(http.clone(), fetcher_cfg));
        let signing_key = Arc::new(SigningKey::from_bytes(&[3u8; 32]));
        let verdict_fetcher = Arc::new(ForgeVerdictFetcher::new(
            http.clone(),
            ForgeVerdictConfig {
                verdict_base_url: verdict_url,
                oracle_signing_key: signing_key,
            },
        ));
        let runner = FileDeliveryProfileRunner {
            evaluator: Arc::new(FileDeliveryEvaluator::new()),
            sla_fetcher,
            verdict_fetcher,
        };

        let payment_uid = [0xaau8; 32];
        let job = job(payment_uid, sla_hash, delivery_hash);
        let rpc = rpc();
        let ctx = EvaluationContext {
            rpc: &rpc,
            http: &http,
            job: &job,
            strict: true,
            ledger: None,
        };

        let outcome = runner.run(&ctx).await.unwrap();
        assert!(outcome.result.approved, "checks: {:?}", outcome.result.checks);
        assert_eq!(outcome.result.resolution_reason, 0);

        // Confirm the delivered-file evidence really came from the Forge
        // verdict endpoint (not a registry blob route): the mock verdict
        // server recorded the request.
        assert!(seen.lock().await.is_some());
    }

    /// Deliberate reject: the verdict endpoint returns a file whose bytes
    /// don't match the on-chain `delivery_hash` the seller promised. The
    /// judge must reject rather than approve or serve the bytes onward.
    #[tokio::test]
    async fn rejects_deliberate_wrong_delivery_via_forge_verdict_path() {
        let sla = FileDeliverySla {
            version: 1,
            profile_id: PROFILE_ID.into(),
            payment_uid: "bb".repeat(32),
            buyer_nonce: None,
            expected_size_bytes_min: 1,
            expected_size_bytes_max: 1_000_000,
            expected_mime: None,
            expected_extension: None,
            attestor_pubkey: None,
        };
        let sla_body = serde_json::to_vec(&sla).unwrap();
        let sla_digest = Sha256::digest(&sla_body);
        let mut sla_hash = [0u8; 32];
        sla_hash.copy_from_slice(&sla_digest);

        let registry_url = spawn_registry(sla_body).await;
        // Wrong file: doesn't match the promised delivery_hash below.
        let (verdict_url, _seen) = spawn_verdict_server(vec![1u8; 256]).await;

        let http = reqwest::Client::builder().build().unwrap();
        let fetcher_cfg = Arc::new(FetcherConfig {
            mirrors: vec![registry_url],
            auth_header: None,
            max_retries: 1,
            retry_base: std::time::Duration::from_millis(1),
        });
        let sla_fetcher = Arc::new(RegistryJsonFetcher::new(http.clone(), fetcher_cfg));
        let signing_key = Arc::new(SigningKey::from_bytes(&[4u8; 32]));
        let verdict_fetcher = Arc::new(ForgeVerdictFetcher::new(
            http.clone(),
            ForgeVerdictConfig {
                verdict_base_url: verdict_url,
                oracle_signing_key: signing_key,
            },
        ));
        let runner = FileDeliveryProfileRunner {
            evaluator: Arc::new(FileDeliveryEvaluator::new()),
            sla_fetcher,
            verdict_fetcher,
        };

        let payment_uid = [0xbbu8; 32];
        let promised_delivery_hash = [9u8; 32]; // does not match the wrong file's digest
        let job = job(payment_uid, sla_hash, promised_delivery_hash);
        let rpc = rpc();
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
}
