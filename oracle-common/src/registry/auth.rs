//! Seller wallet registration + bearer-token middleware.
//!
//! Two-step registration (matching the existing pr402 onboard challenge flow):
//! 1. `GET /v1/registry/seller/challenge?wallet=<pubkey>` returns `{challenge, expires_at}`.
//! 2. Seller signs `challenge` with their wallet keypair, calls
//!    `POST /v1/registry/seller/register` with `{wallet, signature, challenge}`.
//! 3. Registry verifies the Ed25519 signature, inserts a row in `oracle_seller_keys`
//!    with `bearer_sha256 = SHA256(token)`, and returns the raw `token` exactly once.
//!
//! Subsequent uploads include `Authorization: Bearer <token>`. The middleware
//! extracts the bearer, hashes it, looks up the row, rejects revoked rows with `401`,
//! and updates `last_used_at` on success.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{sync::Mutex, time::timeout};
use tokio_postgres::types::Json;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid wallet: {0}")]
    InvalidWallet(String),
    #[error("invalid signature")]
    InvalidSignature,
    #[error("challenge expired or unknown")]
    UnknownChallenge,
    #[error("missing or malformed bearer token")]
    BearerMissing,
    #[error("token revoked")]
    Revoked,
    #[error("token not recognized")]
    Unknown,
    #[error("database: {0}")]
    Db(String),
}

/// In-memory pending challenges keyed by wallet pubkey base58.
///
/// Production deployments may move this to Postgres or Redis if multi-replica
/// state-sharing matters; for v1 the registry runs in-process with the oracle so
/// in-memory is sufficient.
#[derive(Default)]
pub struct ChallengeStore {
    inner: Mutex<Vec<ChallengeRecord>>,
    ttl: ChallengeTtl,
}

#[derive(Clone, Copy)]
pub struct ChallengeTtl(pub Duration);

impl Default for ChallengeTtl {
    fn default() -> Self {
        Self(Duration::from_secs(5 * 60))
    }
}

#[derive(Clone)]
struct ChallengeRecord {
    wallet: String,
    challenge: String,
    issued_at: Instant,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChallengeResponse {
    pub challenge: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterRequest {
    pub wallet: String,
    pub signature: String,
    pub challenge: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegisterResponse {
    pub id: i64,
    /// Raw bearer token. Returned once, never logged. Only `SHA256(token)` is stored.
    pub token: String,
}

impl ChallengeStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
            ttl: ChallengeTtl(ttl),
        }
    }

    /// Issue a fresh challenge for `wallet`. Returns `(response, ttl_seconds)`.
    pub async fn issue(&self, wallet: &str) -> ChallengeResponse {
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        let challenge = bs58::encode(buf).into_string();
        let issued_at = Instant::now();

        let mut g = self.inner.lock().await;
        // Garbage-collect expired entries on every issue.
        g.retain(|r| r.issued_at.elapsed() < self.ttl.0);
        g.push(ChallengeRecord {
            wallet: wallet.to_string(),
            challenge: challenge.clone(),
            issued_at,
        });

        ChallengeResponse {
            challenge,
            expires_at: Utc::now()
                + chrono::Duration::from_std(self.ttl.0).unwrap_or(chrono::Duration::seconds(300)),
        }
    }

    /// Validate that `(wallet, challenge)` was issued recently. Returns true and
    /// removes the entry on success; returns false otherwise.
    pub async fn consume(&self, wallet: &str, challenge: &str) -> bool {
        let mut g = self.inner.lock().await;
        let now = Instant::now();
        g.retain(|r| now.duration_since(r.issued_at) < self.ttl.0);
        if let Some(idx) = g
            .iter()
            .position(|r| r.wallet == wallet && r.challenge == challenge)
        {
            g.swap_remove(idx);
            true
        } else {
            false
        }
    }
}

/// Verify the Ed25519 signature `sig_b58` over `challenge` under `wallet_b58`.
pub fn verify_signature(wallet_b58: &str, challenge: &str, sig_b58: &str) -> Result<(), AuthError> {
    let wallet_bytes = bs58::decode(wallet_b58)
        .into_vec()
        .map_err(|e| AuthError::InvalidWallet(e.to_string()))?;
    if wallet_bytes.len() != 32 {
        return Err(AuthError::InvalidWallet("must be 32 bytes".into()));
    }
    let mut wb = [0u8; 32];
    wb.copy_from_slice(&wallet_bytes);
    let vk = VerifyingKey::from_bytes(&wb).map_err(|e| AuthError::InvalidWallet(e.to_string()))?;

    let sig_bytes = bs58::decode(sig_b58)
        .into_vec()
        .map_err(|_| AuthError::InvalidSignature)?;
    if sig_bytes.len() != 64 {
        return Err(AuthError::InvalidSignature);
    }
    let mut sb = [0u8; 64];
    sb.copy_from_slice(&sig_bytes);
    let sig = Signature::from_bytes(&sb);

    vk.verify(challenge.as_bytes(), &sig)
        .map_err(|_| AuthError::InvalidSignature)
}

/// Generate a fresh raw token (URL-safe base58, 32 bytes of entropy).
pub fn generate_bearer() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    bs58::encode(buf).into_string()
}

/// SHA-256 of the raw token, stored in `oracle_seller_keys.bearer_sha256`.
pub fn token_digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Insert a freshly-registered seller. Returns the row id.
pub async fn insert_seller(
    pool: &Pool,
    wallet: &str,
    bearer_sha256: &str,
    label: Option<&str>,
) -> Result<i64, AuthError> {
    const SQL: &str = r#"
        INSERT INTO oracle_seller_keys (wallet_pubkey, bearer_sha256, label)
        VALUES ($1, $2, $3)
        RETURNING id
    "#;
    let client = pool.get().await.map_err(|e| AuthError::Db(e.to_string()))?;
    let row = timeout(
        Duration::from_secs(10),
        client.query_one(SQL, &[&wallet, &bearer_sha256, &label]),
    )
    .await
    .map_err(|_| AuthError::Db("timeout".into()))?
    .map_err(|e| AuthError::Db(e.to_string()))?;
    let id: i64 = row.try_get(0).map_err(|e| AuthError::Db(e.to_string()))?;
    Ok(id)
}

/// Verify a bearer token. Returns the row id on success, `AuthError::Revoked` /
/// `AuthError::Unknown` otherwise. Updates `last_used_at` on success.
pub async fn verify_bearer(pool: &Pool, raw_token: &str) -> Result<i64, AuthError> {
    let digest = token_digest(raw_token);
    const SQL_LOOKUP: &str = r#"
        SELECT id, revoked
          FROM oracle_seller_keys
         WHERE bearer_sha256 = $1
         LIMIT 1
    "#;
    let client = pool.get().await.map_err(|e| AuthError::Db(e.to_string()))?;
    let row = timeout(
        Duration::from_secs(10),
        client.query_opt(SQL_LOOKUP, &[&digest]),
    )
    .await
    .map_err(|_| AuthError::Db("timeout".into()))?
    .map_err(|e| AuthError::Db(e.to_string()))?
    .ok_or(AuthError::Unknown)?;

    let id: i64 = row.try_get(0).map_err(|e| AuthError::Db(e.to_string()))?;
    let revoked: bool = row.try_get(1).map_err(|e| AuthError::Db(e.to_string()))?;
    if revoked {
        return Err(AuthError::Revoked);
    }

    const SQL_TOUCH: &str = r#"UPDATE oracle_seller_keys SET last_used_at = NOW() WHERE id = $1"#;
    let _ = client.execute(SQL_TOUCH, &[&id]).await;
    Ok(id)
}

/// Mark a token revoked. Used by `POST /v1/registry/seller/rotate`.
pub async fn revoke(pool: &Pool, id: i64) -> Result<(), AuthError> {
    const SQL: &str = r#"
        UPDATE oracle_seller_keys SET revoked = TRUE WHERE id = $1
    "#;
    let client = pool.get().await.map_err(|e| AuthError::Db(e.to_string()))?;
    timeout(Duration::from_secs(10), client.execute(SQL, &[&id]))
        .await
        .map_err(|_| AuthError::Db("timeout".into()))?
        .map_err(|e| AuthError::Db(e.to_string()))?;
    Ok(())
}

// Suppress unused-import lint for Json which the auth module doesn't directly
// use yet — registry::api will pull this through.
#[allow(dead_code)]
fn _type_check_compile_only(v: serde_json::Value) -> Json<serde_json::Value> {
    Json(v)
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    #[tokio::test]
    async fn challenge_round_trip() {
        let store = ChallengeStore::new(Duration::from_secs(60));
        let wallet = "ABCD1234";
        let r = store.issue(wallet).await;
        assert!(!r.challenge.is_empty());
        assert!(store.consume(wallet, &r.challenge).await);
        // Consume only once.
        assert!(!store.consume(wallet, &r.challenge).await);
    }

    #[tokio::test]
    async fn challenge_wrong_wallet_does_not_match() {
        let store = ChallengeStore::new(Duration::from_secs(60));
        let r = store.issue("alice").await;
        assert!(!store.consume("bob", &r.challenge).await);
        // Original still works for the right wallet.
        assert!(store.consume("alice", &r.challenge).await);
    }

    #[tokio::test]
    async fn challenge_expires() {
        let store = ChallengeStore::new(Duration::from_millis(20));
        let r = store.issue("alice").await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!store.consume("alice", &r.challenge).await);
    }

    #[test]
    fn ed25519_round_trip() {
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let pk = sk.verifying_key();
        let challenge = "hello-challenge";
        let sig = sk.sign(challenge.as_bytes());

        let wallet_b58 = bs58::encode(pk.to_bytes()).into_string();
        let sig_b58 = bs58::encode(sig.to_bytes()).into_string();

        verify_signature(&wallet_b58, challenge, &sig_b58).unwrap();
    }

    #[test]
    fn ed25519_rejects_tampered_signature() {
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let pk = sk.verifying_key();
        let sig = sk.sign(b"hello");

        let wallet_b58 = bs58::encode(pk.to_bytes()).into_string();
        // Tamper one byte of the signature.
        let mut sig_bytes = sig.to_bytes();
        sig_bytes[0] = sig_bytes[0].wrapping_add(1);
        let sig_b58 = bs58::encode(sig_bytes).into_string();

        let res = verify_signature(&wallet_b58, "hello", &sig_b58);
        assert!(matches!(res, Err(AuthError::InvalidSignature)));
    }

    #[test]
    fn token_digest_is_deterministic() {
        let t = generate_bearer();
        let d1 = token_digest(&t);
        let d2 = token_digest(&t);
        assert_eq!(d1, d2);
        assert_eq!(d1.len(), 64); // 32 bytes hex
    }

    #[test]
    fn generate_bearer_produces_unique_tokens() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            let t = generate_bearer();
            assert!(seen.insert(t));
        }
    }
}
