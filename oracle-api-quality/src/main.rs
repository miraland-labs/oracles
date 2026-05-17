//! `oracle-api-quality` binary.
//!
//! Wires together:
//!
//! * [`oracle_common::config::OracleConfig`] from env vars.
//! * Postgres ledger if `DATABASE_URL` is set.
//! * Storage backend per `ORACLE_REGISTRY_BACKEND`.
//! * Single registered profile `x402/oracle/api-quality/v1`.
//! * Chain monitor + worker + HTTP server.

use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use deadpool_postgres::{Config as PgConfig, PoolConfig, Runtime};
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use oracle_api_quality::{
    evaluator::ApiQualityEvaluator, evidence::DeliveryEvidence, sla::SlaDocument, PROFILE_ID,
};
use oracle_common::{
    chain,
    config::OracleConfig,
    db::OracleDb,
    fetcher::{FetcherConfig, RegistryJsonFetcher},
    profile::{ProfileBinding, ProfileRegistry, RegisteredProfile},
    registry::{
        api::{registry_router, BackendKind, RegistryState},
        auth::ChallengeStore,
        storage::make_backend,
    },
    server::{create_core_router, AppState, OracleStats},
    types::RuntimeHealth,
    worker::run_worker,
};
use postgres_openssl::MakeTlsConnector;
use solana_client::nonblocking::rpc_client::RpcClient;
use tokio::sync::{mpsc, RwLock};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            "oracle_api_quality=info,oracle_common=info,tower_http=info".into()
        }))
        .init();

    let config = OracleConfig::from_env().context("loading OracleConfig from env")?;

    info!("╔══════════════════════════════════════════════════════╗");
    info!("║  oracle-api-quality — x402/oracle/api-quality/v1     ║");
    info!("╚══════════════════════════════════════════════════════╝");
    info!("oracle_pubkey:  {}", config.oracle_pubkey());
    info!("program_id:     {}", config.escrow_program_id);
    info!("rpc:            {}", config.solana_rpc_url);
    info!("ws:             {}", config.solana_ws_url);
    info!("bind:           {}", config.bind_addr);
    info!("backend:        {:?}", config.registry_backend);
    info!("strict_profile: {}", config.strict_profile);
    info!("registry_urls:  {:?}", config.evidence_registry_urls);

    // ---- Postgres ledger + pool (always required for the registration HTTP API) ----
    let database_url = config
        .database_url
        .as_deref()
        .context("DATABASE_URL is required for the registration HTTP API")?;
    let pg_pool = build_pg_pool(database_url).context("building Postgres pool")?;
    let db = Some(OracleDb::connect(database_url).context("connecting OracleDb")?);

    // ---- Shared HTTP + RPC clients ----
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let rpc = Arc::new(RpcClient::new(config.solana_rpc_url.clone()));

    // ---- Storage backend ----
    let backend = make_backend(
        config.registry_backend,
        Some(pg_pool.clone()),
        config.s3.as_ref(),
        Some(std::path::PathBuf::from(
            "/var/lib/oracle/api-quality/blobs",
        )),
    )
    .await
    .map_err(|e| anyhow::anyhow!("storage backend init: {e}"))?;

    // ---- Profile registry: register exactly one profile (Requirement 25.1) ----
    let evaluator = Arc::new(ApiQualityEvaluator::new(config.strict_profile));
    let fetcher_cfg = Arc::new(FetcherConfig {
        mirrors: config.evidence_registry_urls.clone(),
        auth_header: config.evidence_registry_auth_header.clone(),
        max_retries: config.evidence_fetch_max_retries,
        retry_base: Duration::from_millis(config.evidence_fetch_retry_base_ms),
    });
    let sla_fetcher: Arc<RegistryJsonFetcher<SlaDocument>> =
        Arc::new(RegistryJsonFetcher::new(http.clone(), fetcher_cfg.clone()));
    let evidence_fetcher: Arc<RegistryJsonFetcher<DeliveryEvidence>> =
        Arc::new(RegistryJsonFetcher::new(http.clone(), fetcher_cfg.clone()));

    let mut profiles = ProfileRegistry::new();
    profiles.register(RegisteredProfile {
        profile_id: PROFILE_ID,
        run: Arc::new(ProfileBinding {
            evaluator,
            sla_fetcher,
            evidence_fetcher,
        }),
    });
    if profiles.is_empty() {
        anyhow::bail!("no profiles registered (Requirement 25.2)");
    }
    info!("registered profile: {PROFILE_ID}");

    // ---- App state ----
    let runtime_health = Arc::new(RwLock::new(RuntimeHealth::default()));
    let state = Arc::new(AppState {
        config: config.clone(),
        stats: RwLock::new(OracleStats::default()),
        health: runtime_health.clone(),
        manual_evaluate_requests: RwLock::new(VecDeque::new()),
        db: db.clone(),
        started_at: Instant::now(),
        http: http.clone(),
        rpc: rpc.clone(),
        profiles: Arc::new(profiles),
    });

    // ---- Chain monitor + backfill ----
    let (job_tx, job_rx) = mpsc::channel(config.job_channel_capacity);

    {
        let cfg = Arc::new(config.clone());
        let rpc_b = rpc.clone();
        let http_b = Arc::new(http.clone());
        let tx_b = job_tx.clone();
        let db_b = db.clone();
        let health_b = runtime_health.clone();
        tokio::spawn(async move {
            chain::backfill_missed_deliveries(cfg, rpc_b, http_b, tx_b, db_b, health_b).await;
        });
    }

    {
        let cfg = Arc::new(config.clone());
        let rpc_m = rpc.clone();
        let http_m = Arc::new(http.clone());
        let tx_m = job_tx.clone();
        let health_m = runtime_health.clone();
        tokio::spawn(async move {
            chain::monitor_deliveries(cfg, rpc_m, http_m, tx_m, health_m).await;
        });
    }

    // ---- Worker ----
    let worker_state = state.clone();
    tokio::spawn(async move {
        run_worker(worker_state, job_rx).await;
    });

    // ---- HTTP server (core + registry sub-router) ----
    let registry_state = RegistryState {
        pool: pg_pool.clone(),
        backend,
        backend_kind: BackendKind::from(config.registry_backend),
        challenge_store: Arc::new(ChallengeStore::new(Duration::from_secs(5 * 60))),
        max_bytea_bytes: config.registry_max_bytea_bytes,
        max_blob_bytes: config.registry_max_blob_bytes,
        registered_profile_id: PROFILE_ID,
    };
    let app =
        create_core_router(state.clone()).nest("/v1/registry", registry_router(registry_state));

    let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
    info!("HTTP server listening on {}", config.bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

fn build_pg_pool(database_url: &str) -> anyhow::Result<deadpool_postgres::Pool> {
    let mut cfg = PgConfig::new();
    cfg.url = Some(database_url.to_string());
    cfg.pool = Some(PoolConfig {
        max_size: 8,
        timeouts: deadpool_postgres::Timeouts {
            wait: Some(Duration::from_secs(15)),
            create: Some(Duration::from_secs(10)),
            recycle: Some(Duration::from_secs(30)),
        },
        ..Default::default()
    });
    let mut builder = SslConnector::builder(SslMethod::tls())?;
    builder.set_verify(SslVerifyMode::NONE);
    let tls = MakeTlsConnector::new(builder.build());
    Ok(cfg.create_pool(Some(Runtime::Tokio1), tls)?)
}
