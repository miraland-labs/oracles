//! Registration HTTP routes.
//!
//! Exposed under `/v1/registry/...` on the oracle binary's main Axum router. Three
//! upload routes (`POST /v1/registry/{sla,delivery,blob}`), a content-addressed
//! `GET`/`HEAD`, an `info` endpoint, and the seller registration / rotation flow
//! from [`super::auth`].
//!
//! Idempotency: every upload is content-addressed by SHA-256, and `oracle_deliveries`
//! has a `UNIQUE (sha256_hex, kind)` index, so duplicate uploads converge to one row
//! and the response reflects the canonical record.

use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::{timeout, Duration};

use super::{
    auth::{
        generate_bearer, insert_seller, revoke, token_digest, verify_bearer, verify_signature,
        AuthError, ChallengeStore, RegisterRequest, RegisterResponse,
    },
    storage::{StorageBackend, StorageError},
};

/// Shared state for the registry routes.
#[derive(Clone)]
pub struct RegistryState {
    pub pool: Pool,
    pub backend: Arc<dyn StorageBackend>,
    pub backend_kind: BackendKind,
    pub challenge_store: Arc<ChallengeStore>,
    pub max_bytea_bytes: u64,
    pub max_blob_bytes: u64,
    pub registered_profile_id: &'static str,
    /// Base58 oracle authority pubkey served by this binary. Used by
    /// `GET /v1/registry/info` so sellers and operators can read the on-chain
    /// identity directly without parsing logs.
    pub oracle_pubkey: String,
    /// URL of this profile's normative spec (NORMATIVE.md). Optional — the
    /// binary may omit if its spec is hosted at a non-canonical URL the
    /// operator does not want to advertise.
    pub normative_spec_url: Option<String>,
    /// Cluster this binary serves for cluster-pinned profiles
    /// (e.g. `oracle-onchain-transfer` with `TRANSFER_CLUSTER`). `None` for
    /// cluster-agnostic profiles. Sellers/buyers compare against pr402's
    /// `chainId` to catch cluster mismatches before funding.
    pub cluster: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Postgres,
    S3,
    Local,
}

impl From<crate::config::RegistryBackend> for BackendKind {
    fn from(b: crate::config::RegistryBackend) -> Self {
        match b {
            crate::config::RegistryBackend::Postgres => Self::Postgres,
            crate::config::RegistryBackend::S3 => Self::S3,
            crate::config::RegistryBackend::Local => Self::Local,
        }
    }
}

/// Build the registry sub-router. Mount on the binary's main router with
/// `Router::new().nest("/v1/registry", registry_router(state))`.
///
/// The blob route's max body size is set to `max_blob_bytes` from the registry
/// state via `DefaultBodyLimit`. The other JSON routes inherit Axum's default
/// (2 MiB) which is below `max_bytea_bytes`'s 4 MiB default — a per-route
/// `DefaultBodyLimit::max(max_bytea_bytes)` override is set so JSON uploads up to
/// the configured cap are accepted.
pub fn registry_router(state: RegistryState) -> Router {
    let max_blob = state.max_blob_bytes as usize;
    let max_bytea = state.max_bytea_bytes as usize;

    Router::new()
        .route(
            "/sla",
            post(post_sla).layer(DefaultBodyLimit::max(max_bytea)),
        )
        .route(
            "/delivery",
            post(post_delivery).layer(DefaultBodyLimit::max(max_bytea)),
        )
        .route(
            "/blob",
            post(post_blob).layer(DefaultBodyLimit::max(max_blob)),
        )
        .route("/{sha256_hex}", get(get_object).head(head_object))
        .route("/info", get(info))
        .route("/seller/challenge", get(seller_challenge))
        .route("/seller/register", post(seller_register))
        .route("/seller/rotate", post(seller_rotate))
        .with_state(state)
}

// =====================================================================
// Response shapes
// =====================================================================

#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub sha256: String,
    pub url: String,
    pub size_bytes: u64,
    pub kind: &'static str,
    pub stored_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InfoResponse {
    pub backend: BackendKind,
    pub max_bytea_bytes: u64,
    pub max_blob_bytes: u64,
    pub registered_profile_id: &'static str,
    /// Base58 oracle authority pubkey. Sellers paste this into their
    /// HTTP-402 `accepts[].extra.oracleProfiles[].operatorPubkey` so buyers
    /// fund the right `oracle_authority` on-chain.
    pub oracle_pubkey: String,
    /// URL of the profile's normative spec, when the binary advertises one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normative_spec_url: Option<String>,
    /// Cluster this binary serves (only set for cluster-pinned profiles).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
}

impl ApiError {
    fn new(s: impl Into<String>) -> Self {
        Self { error: s.into() }
    }
}

fn err_response(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiError::new(msg))).into_response()
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ").map(|s| s.trim().to_string())
}

async fn require_bearer(state: &RegistryState, headers: &HeaderMap) -> Result<i64, Response> {
    let token = extract_bearer(headers).ok_or_else(|| {
        err_response(
            StatusCode::UNAUTHORIZED,
            "missing or malformed bearer token",
        )
    })?;
    match verify_bearer(&state.pool, &token).await {
        Ok(id) => Ok(id),
        Err(AuthError::Revoked) => Err(err_response(
            StatusCode::UNAUTHORIZED,
            "bearer token revoked",
        )),
        Err(AuthError::Unknown) => Err(err_response(
            StatusCode::UNAUTHORIZED,
            "bearer token not recognized",
        )),
        Err(other) => Err(err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("auth: {other}"),
        )),
    }
}

// =====================================================================
// Upload handlers
// =====================================================================

/// `profile_id` envelope for sniffing the SLA's family — see [`crate::types::SlaEnvelope`].
#[derive(Debug, Deserialize)]
struct ProfileIdSniffer {
    profile_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sla,
    Delivery,
    Blob,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Sla => "sla",
            Kind::Delivery => "delivery",
            Kind::Blob => "blob",
        }
    }
}

async fn post_sla(State(state): State<RegistryState>, headers: HeaderMap, body: Bytes) -> Response {
    let seller_key_id = match require_bearer(&state, &headers).await {
        Ok(id) => Some(id),
        Err(resp) => return resp,
    };
    if body.len() as u64 > state.max_bytea_bytes {
        return err_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("body exceeds max_bytea_bytes ({})", state.max_bytea_bytes),
        );
    }
    let sniff: ProfileIdSniffer = match serde_json::from_slice(&body) {
        Ok(s) => s,
        Err(e) => {
            return err_response(
                StatusCode::BAD_REQUEST,
                format!("body is not valid JSON: {e}"),
            );
        }
    };
    let profile_id = match sniff.profile_id {
        Some(p) if !p.trim().is_empty() => Some(p),
        _ => {
            return err_response(
                StatusCode::BAD_REQUEST,
                "SLA JSON must contain a non-empty `profile_id` field",
            );
        }
    };

    upload_payload(&state, body, Kind::Sla, profile_id, seller_key_id, None).await
}

async fn post_delivery(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let seller_key_id = match require_bearer(&state, &headers).await {
        Ok(id) => Some(id),
        Err(resp) => return resp,
    };
    if body.len() as u64 > state.max_bytea_bytes {
        return err_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("body exceeds max_bytea_bytes ({})", state.max_bytea_bytes),
        );
    }
    // Delivery uploads MUST be valid JSON. Non-JSON binary content has a
    // designated home at `POST /v1/registry/blob`. `profile_id` itself is
    // OPTIONAL on this endpoint (some profiles, e.g. `file-delivery`, do
    // not carry it on the delivery side); we sniff it for catalog tagging
    // when present.
    let sniffed_profile = match serde_json::from_slice::<ProfileIdSniffer>(&body) {
        Ok(s) => s.profile_id,
        Err(e) => {
            return err_response(
                StatusCode::BAD_REQUEST,
                format!("body is not valid JSON: {e}"),
            );
        }
    };
    upload_payload(
        &state,
        body,
        Kind::Delivery,
        sniffed_profile,
        seller_key_id,
        None,
    )
    .await
}

async fn post_blob(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let seller_key_id = match require_bearer(&state, &headers).await {
        Ok(id) => Some(id),
        Err(resp) => return resp,
    };
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    upload_payload(&state, body, Kind::Blob, None, seller_key_id, content_type).await
}

/// Common upload path for the three kinds. Streams the body through the storage
/// backend (which computes the SHA-256 incrementally), enforces the per-kind size
/// cap, then upserts the catalog row.
async fn upload_payload(
    state: &RegistryState,
    body: Bytes,
    kind: Kind,
    profile_id: Option<String>,
    seller_key_id: Option<i64>,
    content_type: Option<String>,
) -> Response {
    use futures_util::stream;

    let max = match kind {
        Kind::Sla | Kind::Delivery => state.max_bytea_bytes,
        Kind::Blob => state.max_blob_bytes,
    };
    if body.len() as u64 > max {
        return err_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("body exceeds max bytes ({max})"),
        );
    }

    let body_len = body.len() as u64;
    let body_clone = body.clone();
    let stream: super::storage::ByteStream =
        Box::pin(stream::iter(vec![Ok::<_, std::io::Error>(body_clone)]));
    let stored = match state
        .backend
        .put_streaming(stream, max, content_type.clone())
        .await
    {
        Ok(s) => s,
        Err(StorageError::TooLarge { size, max }) => {
            return err_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("body too large ({size} > {max})"),
            );
        }
        Err(e) => {
            return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}"));
        }
    };

    // Sanity check the backend agreed with us on size.
    if stored.size_bytes != body_len {
        return err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "size mismatch between backend and body",
        );
    }

    // Insert the catalog row. ON CONFLICT (sha256_hex, kind) DO UPDATE keeps a
    // stable response shape across duplicate uploads.
    let stored_at = Utc::now();
    let storage_backend = match state.backend_kind {
        BackendKind::Postgres => "postgres",
        BackendKind::S3 => "s3",
        BackendKind::Local => "local",
    };
    let storage_key = match state.backend_kind {
        BackendKind::S3 => format!("oracle-blobs/{}", stored.hash_hex),
        _ => stored.hash_hex.clone(),
    };

    const SQL: &str = r#"
        INSERT INTO oracle_deliveries (
            sha256_hex, kind, size_bytes, content_type, seller_key_id,
            profile_id, storage_backend, storage_key
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (sha256_hex, kind) DO UPDATE SET
            size_bytes = EXCLUDED.size_bytes,
            content_type = COALESCE(EXCLUDED.content_type, oracle_deliveries.content_type),
            seller_key_id = COALESCE(EXCLUDED.seller_key_id, oracle_deliveries.seller_key_id),
            profile_id = COALESCE(EXCLUDED.profile_id, oracle_deliveries.profile_id)
    "#;

    let client = match state.pool.get().await {
        Ok(c) => c,
        Err(e) => {
            return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("db pool: {e}"));
        }
    };

    let size_i64 = i64::try_from(stored.size_bytes).unwrap_or(i64::MAX);
    let exec_res = timeout(
        Duration::from_secs(10),
        client.execute(
            SQL,
            &[
                &stored.hash_hex,
                &kind.label(),
                &size_i64,
                &content_type.as_deref(),
                &seller_key_id,
                &profile_id.as_deref(),
                &storage_backend,
                &storage_key,
            ],
        ),
    )
    .await;
    match exec_res {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            return err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db catalog upsert: {e}"),
            );
        }
        Err(_) => {
            return err_response(StatusCode::GATEWAY_TIMEOUT, "db timeout");
        }
    }

    let url = format!("/v1/registry/{}", stored.hash_hex);
    (
        StatusCode::OK,
        Json(UploadResponse {
            sha256: stored.hash_hex,
            url,
            size_bytes: stored.size_bytes,
            kind: kind.label(),
            stored_at,
            content_type: stored.content_type,
        }),
    )
        .into_response()
}

// =====================================================================
// GET / HEAD
// =====================================================================

async fn get_object(
    State(state): State<RegistryState>,
    Path(sha256_hex): Path<String>,
) -> Response {
    if !is_valid_sha256_hex(&sha256_hex) {
        return err_response(
            StatusCode::BAD_REQUEST,
            "path must be 64 lowercase hex chars",
        );
    }
    match state.backend.get(&sha256_hex).await {
        Ok(bytes) => {
            // Re-verify SHA-256 over the body before serving (P-HASH-1 / P-HASH-2).
            let computed = Sha256::digest(&bytes);
            if hex::encode(computed) != sha256_hex {
                return err_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "stored bytes do not hash to requested digest",
                );
            }
            // Look up content_type from catalog.
            let content_type = catalog_content_type(&state.pool, &sha256_hex)
                .await
                .unwrap_or_default();
            let mut headers = HeaderMap::new();
            if let Some(ct) = content_type.as_deref() {
                if let Ok(v) = ct.parse() {
                    headers.insert(header::CONTENT_TYPE, v);
                }
            } else {
                headers.insert(
                    header::CONTENT_TYPE,
                    "application/octet-stream".parse().unwrap(),
                );
            }
            (StatusCode::OK, headers, bytes).into_response()
        }
        Err(StorageError::NotFound(_)) => err_response(StatusCode::NOT_FOUND, "not found"),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")),
    }
}

async fn head_object(
    State(state): State<RegistryState>,
    Path(sha256_hex): Path<String>,
) -> Response {
    if !is_valid_sha256_hex(&sha256_hex) {
        return err_response(
            StatusCode::BAD_REQUEST,
            "path must be 64 lowercase hex chars",
        );
    }
    match state.backend.stat(&sha256_hex).await {
        Ok(Some(stat)) => {
            let mut headers = HeaderMap::new();
            if let Some(ct) = stat.content_type.as_deref() {
                if let Ok(v) = ct.parse() {
                    headers.insert(header::CONTENT_TYPE, v);
                }
            }
            headers.insert(header::CONTENT_LENGTH, stat.size_bytes.into());
            (StatusCode::OK, headers).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")),
    }
}

async fn catalog_content_type(
    pool: &Pool,
    hash_hex: &str,
) -> Result<Option<String>, tokio_postgres::Error> {
    let client = pool.get().await.map_err(|_| {
        // Synthesize a real error type. Falling back to a generic db error keeps the
        // signature simple; the caller treats Err as "no content type".
        tokio_postgres::Error::__private_api_timeout()
    })?;
    let row = client
        .query_opt(
            "SELECT content_type FROM oracle_deliveries WHERE sha256_hex = $1 LIMIT 1",
            &[&hash_hex],
        )
        .await?;
    Ok(row.and_then(|r| r.try_get::<_, Option<String>>(0).ok().flatten()))
}

fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
} // =====================================================================
  // Info + seller endpoints
  // =====================================================================

async fn info(State(state): State<RegistryState>) -> Response {
    Json(InfoResponse {
        backend: state.backend_kind,
        max_bytea_bytes: state.max_bytea_bytes,
        max_blob_bytes: state.max_blob_bytes,
        registered_profile_id: state.registered_profile_id,
        oracle_pubkey: state.oracle_pubkey.clone(),
        normative_spec_url: state.normative_spec_url.clone(),
        cluster: state.cluster.clone(),
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
struct ChallengeQuery {
    wallet: String,
}

async fn seller_challenge(
    State(state): State<RegistryState>,
    Query(q): Query<ChallengeQuery>,
) -> Response {
    if q.wallet.trim().is_empty() {
        return err_response(StatusCode::BAD_REQUEST, "wallet query parameter required");
    }
    let resp = state.challenge_store.issue(&q.wallet).await;
    Json(resp).into_response()
}

async fn seller_register(
    State(state): State<RegistryState>,
    Json(req): Json<RegisterRequest>,
) -> Response {
    if !state
        .challenge_store
        .consume(&req.wallet, &req.challenge)
        .await
    {
        return err_response(
            StatusCode::BAD_REQUEST,
            "challenge expired or unknown for this wallet",
        );
    }
    if let Err(e) = verify_signature(&req.wallet, &req.challenge, &req.signature) {
        return err_response(StatusCode::BAD_REQUEST, format!("signature invalid: {e}"));
    }
    let token = generate_bearer();
    let digest = token_digest(&token);
    match insert_seller(&state.pool, &req.wallet, &digest, None).await {
        Ok(id) => Json(RegisterResponse { id, token }).into_response(),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("auth: {e}")),
    }
}

async fn seller_rotate(State(state): State<RegistryState>, headers: HeaderMap) -> Response {
    let id = match require_bearer(&state, &headers).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    if let Err(e) = revoke(&state.pool, id).await {
        return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("auth: {e}"));
    }
    // Issue a fresh token. The wallet associated with the old token is preserved on
    // the row; we look it up so the new row keeps the wallet binding.
    let wallet = match wallet_for_id(&state.pool, id).await {
        Ok(Some(w)) => w,
        Ok(None) => {
            return err_response(StatusCode::NOT_FOUND, "seller key id not found");
        }
        Err(e) => {
            return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}"));
        }
    };
    let token = generate_bearer();
    let digest = token_digest(&token);
    match insert_seller(&state.pool, &wallet, &digest, None).await {
        Ok(new_id) => {
            let resp = RegisterResponse { id: new_id, token };
            Json(resp).into_response()
        }
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("auth: {e}")),
    }
}

async fn wallet_for_id(pool: &Pool, id: i64) -> Result<Option<String>, tokio_postgres::Error> {
    let client = pool
        .get()
        .await
        .map_err(|_| tokio_postgres::Error::__private_api_timeout())?;
    let row = client
        .query_opt(
            "SELECT wallet_pubkey FROM oracle_seller_keys WHERE id = $1",
            &[&id],
        )
        .await?;
    Ok(row.and_then(|r| r.try_get::<_, String>(0).ok()))
}
