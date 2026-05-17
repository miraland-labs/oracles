//! Evidence-fetch abstraction.
//!
//! The `EvidenceFetcher` trait lets each oracle family bring its own way of pulling
//! SLA / evidence bytes from the registry mirrors. Two concrete implementations ship
//! with `oracle-common`:
//!
//! * [`RegistryJsonFetcher<T>`] — fetches a single JSON document, hash-verifies the
//!   raw bytes, then parses into `T`. Used by the api-quality and onchain-transfer
//!   families.
//! * [`RegistryStreamingFetcher`] — streams the body, computes SHA-256 incrementally,
//!   sniffs MIME from the leading window. Returns [`StreamedBlobOutcome`] without
//!   parsing. Used by the file-delivery family.
//!
//! Both implementations honour the design's hash-verify-before-parse rule (see
//! design.md §C5 and properties P-HASH-1..P-HASH-3). On hash mismatch they return
//! `OracleError::EvidenceNotFound` with the *computed* digest in the error message
//! so operators can see exactly why the verify failed.

use std::{marker::PhantomData, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, AUTHORIZATION};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::error::OracleError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    Sla,
    Delivery,
    Blob,
}

/// What an `EvidenceFetcher` produces. Concrete output type is associated so a JSON
/// fetcher returns a parsed `T`, while a streaming blob fetcher returns metadata.
#[async_trait]
pub trait EvidenceFetcher: Send + Sync {
    type Output: Send + Sync;

    /// Fetch and hash-verify the artifact identified by `hash`. Implementations MUST
    /// compute SHA-256 over the raw bytes BEFORE producing `Output`.
    async fn fetch(&self, hash: &[u8; 32], kind: ArtifactKind)
        -> Result<Self::Output, OracleError>;
}

/// Configuration for both fetchers.
#[derive(Clone)]
pub struct FetcherConfig {
    /// Ordered registry mirror base URLs; the first one that returns a hash-valid
    /// body wins.
    pub mirrors: Vec<String>,
    /// Optional `Authorization` header value (e.g. `"Bearer ..."`) sent on every GET.
    pub auth_header: Option<String>,
    /// Per-URL retry count for transient 5xx / network errors.
    pub max_retries: u32,
    /// Base delay between retries; doubled on each successive attempt.
    pub retry_base: Duration,
}

// =====================================================================
// JSON fetcher
// =====================================================================

/// Fetches a JSON document and parses it into `T` after hash verification.
pub struct RegistryJsonFetcher<T> {
    client: reqwest::Client,
    cfg: Arc<FetcherConfig>,
    _t: PhantomData<fn() -> T>,
}

impl<T> RegistryJsonFetcher<T> {
    pub fn new(client: reqwest::Client, cfg: Arc<FetcherConfig>) -> Self {
        Self {
            client,
            cfg,
            _t: PhantomData,
        }
    }
}

#[async_trait]
impl<T: DeserializeOwned + Send + Sync + 'static> EvidenceFetcher for RegistryJsonFetcher<T> {
    type Output = T;

    async fn fetch(
        &self,
        hash: &[u8; 32],
        _kind: ArtifactKind,
    ) -> Result<Self::Output, OracleError> {
        let bytes = fetch_with_verify(&self.client, &self.cfg, hash).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| OracleError::SlaParse(format!("JSON parse failed: {e}")))
    }
}

// =====================================================================
// Streaming blob fetcher
// =====================================================================

/// What the streaming fetcher returns: the verified hash, total size, and a sniffed
/// MIME type from the leading window. The body bytes are NOT retained in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamedBlobOutcome {
    pub hash_hex: String,
    pub size_bytes: u64,
    pub sniffed_mime: Option<String>,
}

/// Streams the body, computes SHA-256 incrementally, sniffs MIME from the leading
/// window. Memory ceiling is bounded by the read-buffer size and the sniff window.
pub struct RegistryStreamingFetcher {
    client: reqwest::Client,
    cfg: Arc<FetcherConfig>,
}

impl RegistryStreamingFetcher {
    /// 64 KiB read buffer per chunk, matching design.md §Performance Considerations.
    pub const READ_BUFFER: usize = 64 * 1024;
    /// 512-byte MIME-sniff window — enough for `infer` to identify common formats.
    pub const SNIFF_WINDOW: usize = 512;

    pub fn new(client: reqwest::Client, cfg: Arc<FetcherConfig>) -> Self {
        Self { client, cfg }
    }
}

#[async_trait]
impl EvidenceFetcher for RegistryStreamingFetcher {
    type Output = StreamedBlobOutcome;

    async fn fetch(
        &self,
        hash: &[u8; 32],
        _kind: ArtifactKind,
    ) -> Result<Self::Output, OracleError> {
        let hash_hex = hex::encode(hash);
        let auth_headers = build_auth_headers(self.cfg.auth_header.as_deref())?;

        let mut last_err = String::new();
        for base in &self.cfg.mirrors {
            let url = format!("{}/{}", base.trim_end_matches('/'), hash_hex);
            for attempt in 0..self.cfg.max_retries.max(1) {
                let req = self.client.get(&url).headers(auth_headers.clone());
                match req.send().await {
                    Ok(response) => {
                        let status = response.status();
                        if !status.is_success() {
                            last_err = format!("{} returned {}", url, status);
                            if status.is_server_error() && attempt + 1 < self.cfg.max_retries.max(1)
                            {
                                tokio::time::sleep(backoff(self.cfg.retry_base, attempt)).await;
                                continue;
                            }
                            break;
                        }

                        let mut hasher = Sha256::new();
                        let mut size: u64 = 0;
                        let mut sniff_buf: Vec<u8> = Vec::new();
                        let mut stream = response.bytes_stream();

                        while let Some(chunk) = stream.next().await {
                            let chunk = chunk.map_err(|e| {
                                OracleError::EvidenceNotFound(format!("transport: {e}"))
                            })?;
                            if sniff_buf.len() < Self::SNIFF_WINDOW {
                                let to_take = Self::SNIFF_WINDOW.saturating_sub(sniff_buf.len());
                                let head = &chunk[..chunk.len().min(to_take)];
                                sniff_buf.extend_from_slice(head);
                            }
                            hasher.update(&chunk);
                            size += chunk.len() as u64;
                        }

                        let computed = hasher.finalize();
                        let computed_hex = hex::encode(computed);
                        if computed.as_slice() != hash {
                            return Err(OracleError::EvidenceNotFound(format!(
                                "Hash mismatch for {hash_hex}: computed {computed_hex}"
                            )));
                        }

                        let sniffed_mime =
                            infer::get(&sniff_buf).map(|t| t.mime_type().to_string());

                        return Ok(StreamedBlobOutcome {
                            hash_hex: computed_hex,
                            size_bytes: size,
                            sniffed_mime,
                        });
                    }
                    Err(e) => {
                        last_err = e.to_string();
                        if attempt + 1 < self.cfg.max_retries.max(1) {
                            tokio::time::sleep(backoff(self.cfg.retry_base, attempt)).await;
                        }
                    }
                }
            }
        }

        Err(OracleError::EvidenceNotFound(format!(
            "{hash_hex} (streaming, tried {} mirror(s)): {last_err}",
            self.cfg.mirrors.len()
        )))
    }
}

// =====================================================================
// Shared helpers
// =====================================================================

fn build_auth_headers(value: Option<&str>) -> Result<HeaderMap, OracleError> {
    let mut headers = HeaderMap::new();
    if let Some(v) = value {
        let parsed = v
            .parse()
            .map_err(|e: reqwest::header::InvalidHeaderValue| {
                OracleError::EvidenceNotFound(format!("invalid AUTH header: {e}"))
            })?;
        headers.insert(AUTHORIZATION, parsed);
    }
    Ok(headers)
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    base.saturating_mul(1u32 << attempt.min(6))
}

/// Internal: fetch + verify for the JSON path. Walks mirrors in order, retries on
/// 5xx / transport errors, and returns `EvidenceNotFound` (with the computed digest
/// in the message) on hash mismatch.
async fn fetch_with_verify(
    client: &reqwest::Client,
    cfg: &FetcherConfig,
    hash: &[u8; 32],
) -> Result<bytes::Bytes, OracleError> {
    let hash_hex = hex::encode(hash);
    let auth_headers = build_auth_headers(cfg.auth_header.as_deref())?;

    let mut last_err = String::new();
    for base in &cfg.mirrors {
        let url = format!("{}/{}", base.trim_end_matches('/'), hash_hex);
        for attempt in 0..cfg.max_retries.max(1) {
            let req = client.get(&url).headers(auth_headers.clone());
            match req.send().await {
                Ok(response) => {
                    let status = response.status();
                    if !status.is_success() {
                        last_err = format!("{} returned {}", url, status);
                        if status.is_server_error() && attempt + 1 < cfg.max_retries.max(1) {
                            tokio::time::sleep(backoff(cfg.retry_base, attempt)).await;
                            continue;
                        }
                        break;
                    }
                    match response.bytes().await {
                        Ok(raw) => {
                            let computed = Sha256::digest(&raw);
                            let computed_hex = hex::encode(computed);
                            if computed.as_slice() != hash {
                                return Err(OracleError::EvidenceNotFound(format!(
                                    "Hash mismatch for {hash_hex}: computed {computed_hex}"
                                )));
                            }
                            return Ok(raw);
                        }
                        Err(e) => {
                            last_err = format!("read body: {e}");
                        }
                    }
                }
                Err(e) => {
                    last_err = e.to_string();
                }
            }
            if attempt + 1 < cfg.max_retries.max(1) {
                tokio::time::sleep(backoff(cfg.retry_base, attempt)).await;
            }
        }
    }

    Err(OracleError::EvidenceNotFound(format!(
        "{hash_hex} (tried {} mirror(s), up to {} retries each): {last_err}",
        cfg.mirrors.len(),
        cfg.max_retries.max(1)
    )))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{routing::get, Router};
    use proptest::prelude::*;
    use serde::{Deserialize, Serialize};
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Tiny {
        version: u32,
        profile_id: String,
    }

    fn cfg(base: String) -> Arc<FetcherConfig> {
        Arc::new(FetcherConfig {
            mirrors: vec![base],
            auth_header: None,
            max_retries: 1,
            retry_base: Duration::from_millis(1),
        })
    }

    /// Spawn a tiny axum server that returns `body` for any path.
    async fn spawn_server(body: Vec<u8>) -> String {
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

    /// Spawn a server that returns 500 the first `n` times, then succeeds.
    async fn spawn_flaky_server(body: Vec<u8>, fail_first: u32) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let counter = Arc::new(Mutex::new(0u32));
        let body = Arc::new(body);
        let app = Router::new().route(
            "/{hash}",
            get(move || {
                let counter = counter.clone();
                let body = body.clone();
                async move {
                    let mut c = counter.lock().await;
                    *c += 1;
                    if *c <= fail_first {
                        Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
                    } else {
                        Ok::<_, axum::http::StatusCode>(body.as_ref().clone())
                    }
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn json_fetcher_accepts_hash_match() {
        let body = serde_json::to_vec(&Tiny {
            version: 1,
            profile_id: "x402/test/v1".into(),
        })
        .unwrap();
        let digest = Sha256::digest(&body);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&digest);
        let url = spawn_server(body).await;
        let client = reqwest::Client::builder().build().unwrap();
        let f: RegistryJsonFetcher<Tiny> = RegistryJsonFetcher::new(client, cfg(url));

        let parsed = f.fetch(&hash, ArtifactKind::Sla).await.unwrap();
        assert_eq!(parsed.profile_id, "x402/test/v1");
        assert_eq!(parsed.version, 1);
    }

    #[tokio::test]
    async fn json_fetcher_fails_closed_on_hash_mismatch() {
        let body = serde_json::to_vec(&serde_json::json!({"version": 1})).unwrap();
        let url = spawn_server(body).await;
        let client = reqwest::Client::builder().build().unwrap();
        let f: RegistryJsonFetcher<serde_json::Value> = RegistryJsonFetcher::new(client, cfg(url));

        // Tampered hash — server will return the body but it won't hash to this.
        let tampered = [9u8; 32];
        let err = f.fetch(&tampered, ArtifactKind::Sla).await.unwrap_err();
        match err {
            OracleError::EvidenceNotFound(msg) => {
                assert!(msg.contains("Hash mismatch"), "got: {msg}");
                // Computed digest should appear in the message so operators can compare.
                assert!(msg.contains("computed "), "got: {msg}");
            }
            other => panic!("unexpected: {other}"),
        }
    }

    #[tokio::test]
    async fn json_fetcher_retries_5xx_then_succeeds() {
        let body = serde_json::to_vec(&Tiny {
            version: 1,
            profile_id: "x402/test/v1".into(),
        })
        .unwrap();
        let digest = Sha256::digest(&body);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&digest);
        let url = spawn_flaky_server(body, 2).await;
        let client = reqwest::Client::builder().build().unwrap();
        let mut c = (*cfg(url)).clone();
        c.max_retries = 5;
        c.retry_base = Duration::from_millis(1);
        let f: RegistryJsonFetcher<Tiny> = RegistryJsonFetcher::new(client, Arc::new(c));

        let parsed = f.fetch(&hash, ArtifactKind::Sla).await.unwrap();
        assert_eq!(parsed.profile_id, "x402/test/v1");
    }

    #[tokio::test]
    async fn streaming_fetcher_accepts_hash_match_and_sniffs_mime() {
        // PNG header for a tiny IHDR-only blob. `infer` should report image/png.
        let mut body = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        body.extend_from_slice(&[0u8; 16]);
        let digest = Sha256::digest(&body);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&digest);
        let url = spawn_server(body).await;
        let client = reqwest::Client::builder().build().unwrap();
        let f = RegistryStreamingFetcher::new(client, cfg(url));

        let outcome = f.fetch(&hash, ArtifactKind::Blob).await.unwrap();
        assert_eq!(outcome.hash_hex, hex::encode(hash));
        assert_eq!(outcome.size_bytes, 24);
        assert_eq!(outcome.sniffed_mime.as_deref(), Some("image/png"));
    }

    #[tokio::test]
    async fn streaming_fetcher_fails_closed_on_hash_mismatch() {
        let body = vec![1u8; 100];
        let url = spawn_server(body).await;
        let client = reqwest::Client::builder().build().unwrap();
        let f = RegistryStreamingFetcher::new(client, cfg(url));

        let tampered = [7u8; 32];
        let err = f.fetch(&tampered, ArtifactKind::Blob).await.unwrap_err();
        match err {
            OracleError::EvidenceNotFound(msg) => {
                assert!(msg.contains("Hash mismatch"), "got: {msg}");
            }
            other => panic!("unexpected: {other}"),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 16,
            max_shrink_iters: 32,
            ..ProptestConfig::default()
        })]

        /// P-HASH-1 / P-HASH-2: any tampered digest produces `EvidenceNotFound`.
        #[test]
        fn fetcher_rejects_any_tampered_digest(
            body in prop::collection::vec(any::<u8>(), 0..512),
            tamper_byte in any::<u8>(),
            tamper_index in 0u8..32
        ) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                // Compute the honest digest, then mutate one byte to ensure mismatch.
                let computed = Sha256::digest(&body);
                let mut tampered = [0u8; 32];
                tampered.copy_from_slice(&computed);
                let idx = tamper_index as usize;
                let mut new_byte = tamper_byte;
                if new_byte == tampered[idx] {
                    new_byte = new_byte.wrapping_add(1);
                }
                tampered[idx] = new_byte;

                let url = spawn_server(body).await;
                let client = reqwest::Client::builder().build().unwrap();
                let f: RegistryJsonFetcher<serde_json::Value> = RegistryJsonFetcher::new(
                    client.clone(),
                    cfg(url.clone()),
                );

                let res = f.fetch(&tampered, ArtifactKind::Sla).await;
                prop_assert!(matches!(res, Err(OracleError::EvidenceNotFound(_))),
                    "expected EvidenceNotFound, got {:?}", res.map(|_| "Ok"));

                Ok(())
            })?;
        }
    }
}
