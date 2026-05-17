//! `StreamingBlobFetcher` — wraps `RegistryStreamingFetcher` and converts the
//! `StreamedBlobOutcome` into `FileDeliveryEvidence`.
//!
//! The on-chain `delivery_hash` for `x402/oracles/file-delivery/attestation/v1` commits
//! directly to the blob bytes (not to a JSON envelope), so the fetcher's job is
//! to:
//!
//! 1. Stream the body from the registry mirrors.
//! 2. Compute SHA-256 incrementally (memory bounded by [`READ_BUFFER`]).
//! 3. Sniff MIME from the leading [`SNIFF_WINDOW`] bytes.
//! 4. Produce a [`FileDeliveryEvidence`] that captures the verified hash, size,
//!    and MIME without retaining the bytes.
//!
//! Hash mismatch surfaces as `OracleError::EvidenceNotFound` from the underlying
//! `RegistryStreamingFetcher`, which the worker converts into a `failed` ledger
//! row (no settlement attempt — see Property P-HASH-3).

use std::sync::Arc;

use async_trait::async_trait;
pub use oracle_common::fetcher::RegistryStreamingFetcher as InnerStreamingFetcher;
use oracle_common::{
    error::OracleError,
    fetcher::{ArtifactKind, EvidenceFetcher, FetcherConfig, RegistryStreamingFetcher},
};

use crate::evidence::FileDeliveryEvidence;

/// Streaming fetcher specialised for the file-delivery family. Returns
/// [`FileDeliveryEvidence`] (the streaming-fetch outcome) so the evaluator never
/// touches the raw bytes.
pub struct StreamingBlobFetcher {
    inner: RegistryStreamingFetcher,
}

impl StreamingBlobFetcher {
    pub fn new(client: reqwest::Client, cfg: Arc<FetcherConfig>) -> Self {
        Self {
            inner: RegistryStreamingFetcher::new(client, cfg),
        }
    }
}

#[async_trait]
impl EvidenceFetcher for StreamingBlobFetcher {
    type Output = FileDeliveryEvidence;

    async fn fetch(
        &self,
        hash: &[u8; 32],
        kind: ArtifactKind,
    ) -> Result<Self::Output, OracleError> {
        // The kind we receive is whatever the runner passes; we always stream from
        // the registry path so the kind is informational here.
        let outcome = self.inner.fetch(hash, kind).await?;
        Ok(FileDeliveryEvidence {
            size_bytes: outcome.size_bytes,
            sniffed_mime: outcome.sniffed_mime,
            blob_sha256_hex: outcome.hash_hex,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::{routing::get, Router};
    use sha2::{Digest, Sha256};
    use tokio::sync::Mutex;

    use super::*;

    /// Spawn a tiny axum server that returns `body` for any path.
    async fn spawn_server(body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(Mutex::new(body));
        let app = Router::new().route(
            "/{hash}",
            get(move || {
                let body = body.clone();
                async move {
                    let body = body.lock().await.clone();
                    body
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn cfg(base: String) -> Arc<FetcherConfig> {
        Arc::new(FetcherConfig {
            mirrors: vec![base],
            auth_header: None,
            max_retries: 1,
            retry_base: Duration::from_millis(1),
        })
    }

    #[tokio::test]
    async fn streams_and_returns_evidence() {
        // PNG header → infer should report image/png.
        let mut body = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        body.extend_from_slice(&[0u8; 64]);
        let digest = Sha256::digest(&body);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&digest);

        let url = spawn_server(body.clone()).await;
        let client = reqwest::Client::builder().build().unwrap();
        let f = StreamingBlobFetcher::new(client, cfg(url));

        let evidence = f.fetch(&hash, ArtifactKind::Blob).await.unwrap();
        assert_eq!(evidence.size_bytes, body.len() as u64);
        assert_eq!(evidence.sniffed_mime.as_deref(), Some("image/png"));
        assert_eq!(evidence.blob_sha256_hex, hex::encode(hash));
    }

    #[tokio::test]
    async fn fails_closed_on_hash_mismatch() {
        let body = vec![1u8; 100];
        let url = spawn_server(body).await;
        let client = reqwest::Client::builder().build().unwrap();
        let f = StreamingBlobFetcher::new(client, cfg(url));

        let tampered = [9u8; 32];
        let err = f.fetch(&tampered, ArtifactKind::Blob).await.unwrap_err();
        match err {
            OracleError::EvidenceNotFound(msg) => {
                assert!(msg.contains("Hash mismatch"), "got: {msg}");
            }
            other => panic!("unexpected: {other}"),
        }
    }
}
