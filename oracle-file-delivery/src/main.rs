//! `oracle-file-delivery` binary entrypoint.
//!
//! NOTE: file-delivery dispatch reads SLA bytes via the JSON fetcher (the SLA is
//! a small JSON document) but evidence is the blob bytes themselves. The current
//! pipeline / `ProfileBinding` shape passes the same JSON fetcher for both; full
//! streaming-fetch evidence wiring lands with Task 15.3 / 15.4.
//!
//! For now this binary boots a `FileDeliveryEvaluator` against the JSON evidence
//! shape `FileDeliveryEvidence`, providing the chain monitor + worker + HTTP
//! server scaffolding so integration tests (Task 15.9) can run end-to-end against
//! a real MinIO container.

use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use deadpool_postgres::{Config as PgConfig, PoolConfig, Runtime};
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
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
use oracle_file_delivery::{
    evaluator::FileDeliveryEvaluator, evidence::FileDeliveryEvidence, sla::FileDeliverySla,
    PROFILE_ID,
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
            "oracle_file_delivery=info,oracle_common=info,tower_http=info".into()
        }))
        .init();

    let config = OracleConfig::from_env().context("loading OracleConfig from env")?;
    let database_url = config
        .database_url
        .as_deref()
        .context("DATABASE_URL is required")?;
    let pg_pool = build_pg_pool(database_url)?;
    let db = Some(OracleDb::connect(database_url)?);

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let rpc = Arc::new(RpcClient::new(config.solana_rpc_url.clone()));

    let backend = make_backend(
        config.registry_backend,
        Some(pg_pool.clone()),
        config.s3.as_ref(),
        Some(std::path::PathBuf::from(
            "/var/lib/oracle/file-delivery/blobs",
        )),
    )
    .await
    .map_err(|e| anyhow::anyhow!("storage backend init: {e}"))?;

    let evaluator = Arc::new(FileDeliveryEvaluator::new());
    let fetcher_cfg = Arc::new(FetcherConfig {
        mirrors: config.evidence_registry_urls.clone(),
        auth_header: config.evidence_registry_auth_header.clone(),
        max_retries: config.evidence_fetch_max_retries,
        retry_base: Duration::from_millis(config.evidence_fetch_retry_base_ms),
    });
    let sla_fetcher: Arc<RegistryJsonFetcher<FileDeliverySla>> =
        Arc::new(RegistryJsonFetcher::new(http.clone(), fetcher_cfg.clone()));
    // Evidence in v1 is JSON-shaped (the streaming fetcher's outcome serialized as
    // JSON). Future revisions will swap this for a true streaming `EvidenceFetcher`
    // returning `FileDeliveryEvidence` directly from the blob bytes.
    let evidence_fetcher: Arc<RegistryJsonFetcher<FileDeliveryEvidence>> =
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
    let worker_state = state.clone();
    tokio::spawn(async move {
        run_worker(worker_state, job_rx).await;
    });

    let registry_state = RegistryState {
        pool: pg_pool.clone(),
        backend,
        backend_kind: BackendKind::from(config.registry_backend),
        challenge_store: Arc::new(ChallengeStore::new(Duration::from_secs(5 * 60))),
        max_bytea_bytes: config.registry_max_bytea_bytes,
        max_blob_bytes: config.registry_max_blob_bytes,
        registered_profile_id: PROFILE_ID,
        oracle_pubkey: config.oracle_pubkey().to_string(),
        normative_spec_url: Some(
            "https://github.com/miraland-labs/oracles/blob/main/\
             oracle-file-delivery/spec/file-delivery-attestation-v1/NORMATIVE.md"
                .into(),
        ),
        cluster: None,
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
