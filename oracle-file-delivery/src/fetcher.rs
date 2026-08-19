//! `ForgeVerdictFetcher` — the file-delivery judge's evidence source on preview.
//!
//! The oracle-side judge acts as a "verdict door": it does not host or mirror
//! downloads itself. Instead it calls Forge's seller-side verdict endpoint —
//!
//! `GET {FORGE_VERDICT_BASE_URL}/api/v1/oracle/listings/{listing_id}/artifact`
//!
//! — authenticated with `payment_uid`, a `timestamp`, and an oracle Ed25519
//! signature over those fields, carried as request headers rather than
//! query-string parameters (query strings are logged by intermediary proxies
//! and CDNs, which would leak the signature).
//!
//! NOTE ON PROVENANCE: the header names (`HEADER_PAYMENT_UID`,
//! `HEADER_TIMESTAMP`, `HEADER_SIGNATURE`) and the signature domain
//! (`VERDICT_SIGNATURE_DOMAIN`) below are this codebase's own choices, not a
//! verbatim copy of Forge's externally published seller-side verdict
//! contract — no in-repo spec documents that contract, and this environment
//! could not reach Forge's published contract to source the exact field
//! names and signature format from it. Do not treat these constants as
//! confirmed to match what Forge actually expects; they must be reconciled
//! against Forge's published contract before this fetcher is pointed at a
//! real Forge deployment.
//!
//! The response stream is treated exactly like the
//! payment door treats it: SHA-256 is computed incrementally over the raw bytes
//! (never buffered in full) and compared against the digest the seller promised
//! on-chain. A match is evidence for approval; a mismatch surfaces as
//! `OracleError::EvidenceNotFound` so the judge rejects — it never partially
//! approves or serves the bytes onward.

use std::{sync::Arc, time::SystemTime};

use ed25519_dalek::{Signer, SigningKey};
use futures_util::StreamExt;
use oracle_common::error::OracleError;
use sha2::{Digest, Sha256};

use crate::evidence::FileDeliveryEvidence;

/// Domain separator for the verdict-request signature, so a signature made for
/// this purpose can never be replayed as a signature for another message shape.
const VERDICT_SIGNATURE_DOMAIN: &[u8] = b"x402/forge/verdict/v1";

/// Header names carrying the auth fields. Headers, not query-string
/// parameters: query strings are routinely logged by reverse proxies, CDNs,
/// and access logs, which would leak `payment_uid` and the oracle's signature
/// to any system in the request path. UNCONFIRMED against Forge's published
/// contract — see the provenance note at the top of this module.
const HEADER_PAYMENT_UID: &str = "x-oracle-payment-uid";
const HEADER_TIMESTAMP: &str = "x-oracle-timestamp";
const HEADER_SIGNATURE: &str = "x-oracle-signature";

/// 512-byte MIME-sniff window — enough for `infer` to identify common formats,
/// matching the payment door's sniffing behaviour.
const SNIFF_WINDOW: usize = 512;

/// Configuration for [`ForgeVerdictFetcher`]. `verdict_base_url` MUST be the
/// preview host — this initiative does not configure a production base URL.
#[derive(Clone, Debug)]
pub struct ForgeVerdictConfig {
    /// Value of `FORGE_VERDICT_BASE_URL`.
    pub verdict_base_url: String,
    /// Oracle's Ed25519 signing key, used to authenticate the verdict request.
    pub oracle_signing_key: Arc<SigningKey>,
}

impl ForgeVerdictConfig {
    /// Build config by reading `FORGE_VERDICT_BASE_URL` from the process
    /// environment. Returns an error if unset — the judge must not fall back
    /// to any other source for delivery inspection.
    pub fn from_env(oracle_signing_key: Arc<SigningKey>) -> Result<Self, OracleError> {
        let verdict_base_url = std::env::var("FORGE_VERDICT_BASE_URL").map_err(|_| {
            OracleError::EvidenceNotFound(
                "FORGE_VERDICT_BASE_URL is not set; the file-delivery judge only reads \
                 deliveries from Forge's seller-side verdict endpoint"
                    .into(),
            )
        })?;
        Ok(Self {
            verdict_base_url,
            oracle_signing_key,
        })
    }
}

/// Fetches the delivered file from Forge's seller-side verdict endpoint and
/// self-verifies it with SHA-256 against the promised digest. Produces
/// [`FileDeliveryEvidence`] — the raw bytes are never retained or served.
pub struct ForgeVerdictFetcher {
    client: reqwest::Client,
    cfg: ForgeVerdictConfig,
}

impl ForgeVerdictFetcher {
    pub fn new(client: reqwest::Client, cfg: ForgeVerdictConfig) -> Self {
        Self { client, cfg }
    }

    /// Sign `listing_id || payment_uid || timestamp` (domain-separated) with
    /// the oracle's Ed25519 key using this module's own (unconfirmed) auth
    /// shape — see the provenance note at the top of this module. Returns the unix
    /// timestamp used and the base58-encoded signature.
    fn sign_request(&self, listing_id: &str, payment_uid: &str) -> (u64, String) {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut message = Vec::new();
        message.extend_from_slice(VERDICT_SIGNATURE_DOMAIN);
        message.push(0);
        message.extend_from_slice(listing_id.as_bytes());
        message.push(0);
        message.extend_from_slice(payment_uid.as_bytes());
        message.push(0);
        message.extend_from_slice(timestamp.to_string().as_bytes());

        let signature = self.cfg.oracle_signing_key.sign(&message);
        (timestamp, bs58::encode(signature.to_bytes()).into_string())
    }

    /// Fetch the artifact for `listing_id` / `payment_uid` from the verdict
    /// endpoint, stream it while computing SHA-256 incrementally, and compare
    /// the result against `promised_sha256`. Ok(..) means the delivery
    /// verified and the judge should approve; Err(..) means it should reject
    /// (wrong file, transport failure, or non-2xx from the verdict endpoint).
    pub async fn fetch_and_verify(
        &self,
        listing_id: &str,
        payment_uid: &str,
        promised_sha256: &[u8; 32],
    ) -> Result<FileDeliveryEvidence, OracleError> {
        let (timestamp, signature) = self.sign_request(listing_id, payment_uid);
        let timestamp_str = timestamp.to_string();

        let url = format!(
            "{}/api/v1/oracle/listings/{listing_id}/artifact",
            self.cfg.verdict_base_url.trim_end_matches('/')
        );

        let response = self
            .client
            .get(&url)
            .header(HEADER_PAYMENT_UID, payment_uid)
            .header(HEADER_TIMESTAMP, timestamp_str.as_str())
            .header(HEADER_SIGNATURE, signature.as_str())
            .send()
            .await
            .map_err(|e| {
                OracleError::EvidenceNotFound(format!("verdict endpoint transport: {e}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(OracleError::EvidenceNotFound(format!(
                "verdict endpoint {url} returned {status}"
            )));
        }

        let mut hasher = Sha256::new();
        let mut size: u64 = 0;
        let mut sniff_buf: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                OracleError::EvidenceNotFound(format!("verdict endpoint stream: {e}"))
            })?;
            if sniff_buf.len() < SNIFF_WINDOW {
                let to_take = SNIFF_WINDOW.saturating_sub(sniff_buf.len());
                let head = &chunk[..chunk.len().min(to_take)];
                sniff_buf.extend_from_slice(head);
            }
            hasher.update(&chunk);
            size += chunk.len() as u64;
        }

        let computed = hasher.finalize();
        let computed_hex = hex::encode(computed);
        if computed.as_slice() != promised_sha256 {
            return Err(OracleError::EvidenceNotFound(format!(
                "Hash mismatch for {}: computed {computed_hex}",
                hex::encode(promised_sha256)
            )));
        }

        let sniffed_mime = infer::get(&sniff_buf).map(|t| t.mime_type().to_string());

        Ok(FileDeliveryEvidence {
            size_bytes: size,
            sniffed_mime,
            blob_sha256_hex: computed_hex,
        })
    }
}

#[cfg(test)]
mod tests {
    use axum::{extract::Path, http::HeaderMap, http::Uri, routing::get, Router};
    use ed25519_dalek::{Verifier, VerifyingKey};
    use tokio::sync::Mutex;

    use super::*;

    /// What the mock verdict server observed for the last request: the path
    /// segment, the raw request headers, and the original URI (so tests can
    /// positively assert the query string carries no auth material).
    #[derive(Clone)]
    struct SeenRequest {
        listing_id: String,
        headers: HeaderMap,
        raw_query: Option<String>,
    }

    fn signing_key() -> Arc<SigningKey> {
        Arc::new(SigningKey::from_bytes(&[7u8; 32]))
    }

    /// Spawn a mock verdict endpoint that returns `body` for any listing id,
    /// and records the last request's headers, path, and raw query string for
    /// assertions.
    async fn spawn_verdict_server(
        body: Vec<u8>,
    ) -> (String, Arc<Mutex<Option<SeenRequest>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(body);
        let seen: Arc<Mutex<Option<SeenRequest>>> = Arc::new(Mutex::new(None));
        let seen_route = seen.clone();
        let app = Router::new().route(
            "/api/v1/oracle/listings/{listing_id}/artifact",
            get(
                move |Path(listing_id): Path<String>, uri: Uri, headers: HeaderMap| {
                    let body = body.clone();
                    let seen_route = seen_route.clone();
                    async move {
                        *seen_route.lock().await = Some(SeenRequest {
                            listing_id,
                            headers,
                            raw_query: uri.query().map(|q| q.to_string()),
                        });
                        body.as_ref().clone()
                    }
                },
            ),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), seen)
    }

    #[tokio::test]
    async fn approves_when_stream_matches_promised_digest() {
        let mut body = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        body.extend_from_slice(&[0u8; 64]);
        let digest = Sha256::digest(&body);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&digest);

        let (base_url, seen) = spawn_verdict_server(body.clone()).await;
        let key = signing_key();
        let cfg = ForgeVerdictConfig {
            verdict_base_url: base_url,
            oracle_signing_key: key.clone(),
        };
        let client = reqwest::Client::builder().build().unwrap();
        let fetcher = ForgeVerdictFetcher::new(client, cfg);

        let evidence = fetcher
            .fetch_and_verify("listing-123", "payment-abc", &hash)
            .await
            .unwrap();

        assert_eq!(evidence.size_bytes, body.len() as u64);
        assert_eq!(evidence.sniffed_mime.as_deref(), Some("image/png"));
        assert_eq!(evidence.blob_sha256_hex, hex::encode(hash));

        // Confirm the judge hit the verdict endpoint using this module's auth shape:
        // correct path, auth fields present as headers, a verifiable oracle
        // signature, and — critically — no auth material leaked into the
        // query string.
        let seen = seen.lock().await.clone().unwrap();
        assert_eq!(seen.listing_id, "listing-123");
        assert!(
            seen.raw_query.is_none(),
            "auth must not be carried in the query string, got: {:?}",
            seen.raw_query
        );
        assert_eq!(
            seen.headers.get(HEADER_PAYMENT_UID).unwrap(),
            "payment-abc"
        );
        let timestamp = seen
            .headers
            .get(HEADER_TIMESTAMP)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let signature_b58 = seen
            .headers
            .get(HEADER_SIGNATURE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let mut message = Vec::new();
        message.extend_from_slice(VERDICT_SIGNATURE_DOMAIN);
        message.push(0);
        message.extend_from_slice(b"listing-123");
        message.push(0);
        message.extend_from_slice(b"payment-abc");
        message.push(0);
        message.extend_from_slice(timestamp.as_bytes());

        let sig_bytes = bs58::decode(signature_b58).into_vec().unwrap();
        let mut sb = [0u8; 64];
        sb.copy_from_slice(&sig_bytes);
        let sig = ed25519_dalek::Signature::from_bytes(&sb);
        let vk: VerifyingKey = key.verifying_key();
        assert!(vk.verify(&message, &sig).is_ok());
    }

    #[tokio::test]
    async fn rejects_when_delivered_file_does_not_match_promised_digest() {
        // The seller promised one digest but the verdict endpoint returns a
        // different file — a deliberate wrong delivery.
        let wrong_body = vec![1u8; 128];
        let (base_url, _seen) = spawn_verdict_server(wrong_body).await;
        let cfg = ForgeVerdictConfig {
            verdict_base_url: base_url,
            oracle_signing_key: signing_key(),
        };
        let client = reqwest::Client::builder().build().unwrap();
        let fetcher = ForgeVerdictFetcher::new(client, cfg);

        let promised = [9u8; 32];
        let err = fetcher
            .fetch_and_verify("listing-999", "payment-xyz", &promised)
            .await
            .unwrap_err();

        match err {
            OracleError::EvidenceNotFound(msg) => {
                assert!(msg.contains("Hash mismatch"), "got: {msg}");
            }
            other => panic!("unexpected: {other}"),
        }
    }

    #[tokio::test]
    async fn rejects_on_non_success_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/api/v1/oracle/listings/{listing_id}/artifact",
            get(|| async { axum::http::StatusCode::NOT_FOUND }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let cfg = ForgeVerdictConfig {
            verdict_base_url: format!("http://{addr}"),
            oracle_signing_key: signing_key(),
        };
        let client = reqwest::Client::builder().build().unwrap();
        let fetcher = ForgeVerdictFetcher::new(client, cfg);

        let err = fetcher
            .fetch_and_verify("missing-listing", "payment-abc", &[0u8; 32])
            .await
            .unwrap_err();

        match err {
            OracleError::EvidenceNotFound(msg) => {
                assert!(msg.contains("404"), "got: {msg}");
            }
            other => panic!("unexpected: {other}"),
        }
    }

    #[tokio::test]
    async fn from_env_reads_forge_verdict_base_url_and_errors_when_unset() {
        // Both assertions live in one test (rather than two) because they
        // mutate the same process-wide env var and cargo test runs tests in
        // parallel threads by default; splitting them would race.
        std::env::remove_var("FORGE_VERDICT_BASE_URL");
        let err = ForgeVerdictConfig::from_env(signing_key()).unwrap_err();
        match err {
            OracleError::EvidenceNotFound(msg) => {
                assert!(msg.contains("FORGE_VERDICT_BASE_URL"), "got: {msg}");
            }
            other => panic!("unexpected: {other}"),
        }

        std::env::set_var("FORGE_VERDICT_BASE_URL", "https://preview.forge.example/api");
        let cfg = ForgeVerdictConfig::from_env(signing_key()).unwrap();
        assert_eq!(cfg.verdict_base_url, "https://preview.forge.example/api");
        std::env::remove_var("FORGE_VERDICT_BASE_URL");
    }
}
