//! HTTP server skeleton.
//!
//! Provides the routes every oracle binary exposes:
//!
//! * `GET /` — service info.
//! * `GET /health` — chain / WebSocket / DB / registry / oracle-balance state.
//! * `GET /stats` — JSON counters (the same numbers `/metrics` exposes in Prometheus
//!   form).
//! * `GET /metrics` — Prometheus text exposition.
//! * `POST /evaluate` — operator-token-gated manual re-evaluation.
//!
//! Per-family registry routes mount under `/v1/registry/...` via
//! [`crate::registry::api::registry_router`].
//!
//! Operator authentication accepts both `Authorization: Bearer <token>` and the
//! convenience header `X-Oracle-Token: <token>` so existing operator scripts
//! keep working.

use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solana_client::nonblocking::rpc_client::RpcClient;
use tokio::sync::RwLock;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

use crate::{
    config::OracleConfig,
    db::OracleDb,
    pipeline,
    profile::ProfileRegistry,
    settler,
    types::{CheckResult, EvaluationJob, RuntimeHealth},
};

/// Cumulative counters surfaced via `/stats` and `/metrics`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OracleStats {
    pub total_evaluated: u64,
    pub total_approved: u64,
    pub total_rejected: u64,
    pub total_errors: u64,
    pub total_dead_letter: u64,
    pub total_evidence_fetch_failures: u64,
    pub uptime_seconds: u64,
    pub last_evaluation_at: Option<String>,
}

/// Process-wide application state shared by the chain monitor, worker, and HTTP
/// server. Holds the connection pool, RPC + HTTP clients, mutable counters / health
/// gauges, and the binary's profile registry.
pub struct AppState {
    pub config: OracleConfig,
    pub stats: RwLock<OracleStats>,
    pub health: Arc<RwLock<RuntimeHealth>>,
    pub manual_evaluate_requests: RwLock<VecDeque<Instant>>,
    pub db: Option<OracleDb>,
    pub started_at: Instant,
    pub http: reqwest::Client,
    pub rpc: Arc<RpcClient>,
    pub profiles: Arc<ProfileRegistry>,
}

impl AppState {
    /// True when the operator-token gate is configured. `POST /evaluate` returns 503
    /// when no token AND no `ORACLE_ALLOW_UNAUTHENTICATED_MANUAL_EVALUATE=true`.
    pub fn manual_eval_enabled(&self) -> bool {
        self.config.operator_token_sha256.is_some()
            || self.config.allow_unauthenticated_manual_evaluate
    }
}

/// Build the top-level Axum router (without the registry sub-router; the binary
/// `main.rs` calls `Router::new().merge(create_core_router(state.clone()))
/// .nest("/v1/registry", registry_router(state.registry_state.clone()))` to
/// assemble the full app).
pub fn create_core_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/stats", get(stats))
        .route("/metrics", get(metrics))
        .route("/evaluate", post(manual_evaluate))
        .layer(cors_layer(&state.config))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn cors_layer(config: &OracleConfig) -> CorsLayer {
    let mut layer = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::HeaderName::from_static("x-oracle-token"),
        ]);
    if !config.cors_allowed_origins.is_empty() {
        let origins: Vec<HeaderValue> = config
            .cors_allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        if !origins.is_empty() {
            layer = layer.allow_origin(AllowOrigin::list(origins));
        }
    }
    layer
}

async fn root() -> impl IntoResponse {
    Json(serde_json::json!({
        "service": "oracle-common",
        "version": env!("CARGO_PKG_VERSION"),
        "endpoints": {
            "GET /":         "service info",
            "GET /health":   "health",
            "GET /stats":    "JSON counters",
            "GET /metrics":  "Prometheus text exposition",
            "POST /evaluate": "operator-only manual re-evaluation"
        }
    }))
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let chain_ok = state.rpc.get_slot().await.is_ok();
    let oracle_balance_lamports = state
        .rpc
        .get_balance(&state.config.oracle_pubkey())
        .await
        .ok();
    let registry_ok = registry_probe(&state).await;
    let runtime = state.health.read().await.clone();

    let status = if chain_ok && runtime.websocket_connected {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(serde_json::json!({
            "status": if chain_ok { "healthy" } else { "degraded" },
            "oracle_pubkey": state.config.oracle_pubkey().to_string(),
            "program_id": state.config.escrow_program_id.to_string(),
            "chain_connected": chain_ok,
            "websocket_connected": runtime.websocket_connected,
            "last_websocket_connected_at": runtime.last_websocket_connected_at,
            "last_websocket_message_at": runtime.last_websocket_message_at,
            "last_monitor_error": runtime.last_monitor_error,
            "queue_depth": runtime.queue_depth,
            "deliveries_observed": runtime.deliveries_observed,
            "last_seen_slot": runtime.last_seen_slot,
            "registry_reachable": registry_ok,
            "oracle_balance_lamports": oracle_balance_lamports,
            "database_enabled": state.db.is_some(),
            "strict_profile": state.config.strict_profile,
            "registered_profiles": state.profiles.known_ids(),
        })),
    )
}

async fn registry_probe(state: &AppState) -> bool {
    let Some(base) = state.config.evidence_registry_urls.first() else {
        return false;
    };
    let url = base.trim_end_matches('/');
    state
        .http
        .head(url)
        .send()
        .await
        .map(|r| r.status().is_success() || r.status().is_client_error())
        .unwrap_or(false)
}

async fn stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut stats = state.stats.read().await.clone();
    stats.uptime_seconds = state.started_at.elapsed().as_secs();
    Json(stats)
}

async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let stats = state.stats.read().await.clone();
    let runtime = state.health.read().await.clone();
    let uptime = state.started_at.elapsed().as_secs();

    let body = format!(
        "# HELP oracle_uptime_seconds Process uptime in seconds\n\
         # TYPE oracle_uptime_seconds counter\n\
         oracle_uptime_seconds {uptime}\n\
         # HELP oracle_total_evaluated Total evaluations completed\n\
         # TYPE oracle_total_evaluated counter\n\
         oracle_total_evaluated {evaluated}\n\
         # HELP oracle_total_approved Total evaluations approved\n\
         # TYPE oracle_total_approved counter\n\
         oracle_total_approved {approved}\n\
         # HELP oracle_total_rejected Total evaluations rejected\n\
         # TYPE oracle_total_rejected counter\n\
         oracle_total_rejected {rejected}\n\
         # HELP oracle_total_errors Total pipeline errors\n\
         # TYPE oracle_total_errors counter\n\
         oracle_total_errors {errors}\n\
         # HELP oracle_total_dead_letter Total jobs moved to dead_letter\n\
         # TYPE oracle_total_dead_letter counter\n\
         oracle_total_dead_letter {dead_letter}\n\
         # HELP oracle_total_evidence_fetch_failures Total evidence registry fetch failures (post-retry)\n\
         # TYPE oracle_total_evidence_fetch_failures counter\n\
         oracle_total_evidence_fetch_failures {evidence_fail}\n\
         # HELP oracle_queue_depth Current job channel queue depth\n\
         # TYPE oracle_queue_depth gauge\n\
         oracle_queue_depth {queue}\n\
         # HELP oracle_websocket_connected 1 when WebSocket subscription is active\n\
         # TYPE oracle_websocket_connected gauge\n\
         oracle_websocket_connected {ws}\n\
         # HELP oracle_deliveries_observed Monotonic count of accepted delivery events\n\
         # TYPE oracle_deliveries_observed counter\n\
         oracle_deliveries_observed {deliveries}\n\
         # HELP oracle_last_seen_slot Highest Solana slot observed by the chain monitor\n\
         # TYPE oracle_last_seen_slot gauge\n\
         oracle_last_seen_slot {slot}\n",
        uptime = uptime,
        evaluated = stats.total_evaluated,
        approved = stats.total_approved,
        rejected = stats.total_rejected,
        errors = stats.total_errors,
        dead_letter = stats.total_dead_letter,
        evidence_fail = stats.total_evidence_fetch_failures,
        queue = runtime.queue_depth,
        ws = if runtime.websocket_connected { 1 } else { 0 },
        deliveries = runtime.deliveries_observed,
        slot = runtime.last_seen_slot,
    );

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

#[derive(Deserialize)]
struct ManualEvaluateRequest {
    payment_pubkey: String,
}

#[derive(Serialize)]
struct ManualEvaluateResponse {
    approved: bool,
    signature: Option<String>,
    checks: Vec<CheckResult>,
    error: Option<String>,
}

async fn manual_evaluate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ManualEvaluateRequest>,
) -> impl IntoResponse {
    if let Err((status, msg)) = authorize_manual_evaluate(&state, &headers).await {
        return (
            status,
            Json(ManualEvaluateResponse {
                approved: false,
                signature: None,
                checks: vec![],
                error: Some(msg),
            }),
        );
    }

    let payment_pubkey: solana_sdk::pubkey::Pubkey = match req.payment_pubkey.parse() {
        Ok(pk) => pk,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ManualEvaluateResponse {
                    approved: false,
                    signature: None,
                    checks: vec![],
                    error: Some(format!("Invalid pubkey: {e}")),
                }),
            );
        }
    };

    let job = match crate::chain::read_payment(
        &state.rpc,
        &payment_pubkey,
        &state.config.oracle_pubkey(),
    )
    .await
    {
        Ok(Some(job)) => job,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ManualEvaluateResponse {
                    approved: false,
                    signature: None,
                    checks: vec![],
                    error: Some(
                        "Payment not found, not assigned to this oracle, or not deliverable".into(),
                    ),
                }),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ManualEvaluateResponse {
                    approved: false,
                    signature: None,
                    checks: vec![],
                    error: Some(format!("Chain read error: {e}")),
                }),
            );
        }
    };

    if let Some(db) = &state.db {
        let _ = db.record_queued(&job).await;
    }

    let timeout = Duration::from_millis(state.config.evaluation_timeout_ms);
    let outcome_res =
        tokio::time::timeout(timeout, run_pipeline_with_state(state.clone(), &job)).await;

    match outcome_res {
        Ok(Ok(outcome)) => {
            // Settle on-chain too — manual evaluate runs the full lifecycle.
            let settle_res = settler::settle(
                &state.rpc,
                &state.config,
                &job,
                outcome.result.approved,
                outcome.result.resolution_reason,
                outcome.resolution_hash,
            )
            .await;
            let signature = match settle_res {
                Ok(sig) => Some(sig),
                Err(e) => {
                    if let Some(db) = &state.db {
                        let _ = db.record_failed(&job, &e.to_string()).await;
                    }
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ManualEvaluateResponse {
                            approved: outcome.result.approved,
                            signature: None,
                            checks: outcome.result.checks,
                            error: Some(e.to_string()),
                        }),
                    );
                }
            };
            if let Some(db) = &state.db {
                let _ = db
                    .record_settled(
                        &job,
                        &outcome.result,
                        signature.as_deref(),
                        &outcome.resolution_hash,
                    )
                    .await;
            }
            let mut stats = state.stats.write().await;
            stats.total_evaluated += 1;
            if outcome.result.approved {
                stats.total_approved += 1;
            } else {
                stats.total_rejected += 1;
            }
            stats.last_evaluation_at = Some(Utc::now().to_rfc3339());

            (
                StatusCode::OK,
                Json(ManualEvaluateResponse {
                    approved: outcome.result.approved,
                    signature,
                    checks: outcome.result.checks,
                    error: None,
                }),
            )
        }
        Ok(Err(e)) => {
            if let Some(db) = &state.db {
                let _ = db.record_failed(&job, &e.to_string()).await;
            }
            let mut stats = state.stats.write().await;
            stats.total_errors += 1;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ManualEvaluateResponse {
                    approved: false,
                    signature: None,
                    checks: vec![],
                    error: Some(e.to_string()),
                }),
            )
        }
        Err(_) => {
            if let Some(db) = &state.db {
                let _ = db.record_failed(&job, "manual pipeline timeout").await;
            }
            let mut stats = state.stats.write().await;
            stats.total_errors += 1;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ManualEvaluateResponse {
                    approved: false,
                    signature: None,
                    checks: vec![],
                    error: Some(format!(
                        "Pipeline timed out after {}ms",
                        state.config.evaluation_timeout_ms
                    )),
                }),
            )
        }
    }
}

/// Run the pipeline with `AppState`-borrowed handles. Wrapper around `pipeline::run_pipeline`
/// that builds the `EvaluationContext` from `AppState`.
async fn run_pipeline_with_state(
    state: Arc<AppState>,
    job: &EvaluationJob,
) -> Result<pipeline::PipelineOutcome, crate::error::OracleError> {
    let ctx = crate::evaluator::EvaluationContext {
        rpc: &state.rpc,
        http: &state.http,
        job,
        strict: state.config.strict_profile,
    };
    pipeline::run_pipeline(&state.profiles, &ctx, job.sla_bytes.as_ref()).await
}

async fn authorize_manual_evaluate(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, String)> {
    if state.config.operator_token_sha256.is_none()
        && !state.config.allow_unauthenticated_manual_evaluate
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Manual evaluation is disabled until ORACLE_OPERATOR_TOKEN_SHA256 or \
             ORACLE_OPERATOR_TOKEN is configured"
                .into(),
        ));
    }

    if let Some(expected_hash) = state.config.operator_token_sha256 {
        let supplied = bearer_token(headers).or_else(|| {
            headers
                .get("x-oracle-token")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        });
        let Some(token) = supplied else {
            return Err((StatusCode::UNAUTHORIZED, "Missing operator token".into()));
        };
        let hash = Sha256::digest(token.as_bytes());
        if hash.as_slice() != &expected_hash[..] {
            return Err((StatusCode::UNAUTHORIZED, "Invalid operator token".into()));
        }
    }

    let now = Instant::now();
    let window = Duration::from_millis(state.config.manual_evaluate_rate_window_ms.max(1));
    let mut requests = state.manual_evaluate_requests.write().await;
    while requests
        .front()
        .map(|t| now.duration_since(*t) > window)
        .unwrap_or(false)
    {
        requests.pop_front();
    }
    if requests.len() >= state.config.manual_evaluate_rate_limit {
        return Err((StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded".into()));
    }
    requests.push_back(now);
    Ok(())
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ").map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_map(headers: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in headers {
            h.insert(
                header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn bearer_token_extracted_from_authorization() {
        let h = header_map(&[("authorization", "Bearer abc123")]);
        assert_eq!(bearer_token(&h).as_deref(), Some("abc123"));
    }

    #[test]
    fn bearer_token_returns_none_without_prefix() {
        let h = header_map(&[("authorization", "abc123")]);
        assert!(bearer_token(&h).is_none());
    }

    #[test]
    fn bearer_token_strips_extra_whitespace() {
        let h = header_map(&[("authorization", "Bearer    spaced   ")]);
        assert_eq!(bearer_token(&h).as_deref(), Some("spaced"));
    }
}
