//! `oracle-rwa-transfer` binary entrypoint.
//!
//! The wiring mirrors `oracle-api-quality::main`. The cluster the evaluator
//! verifies against is read from `TRANSFER_CLUSTER` (one of `mainnet-beta`,
//! `devnet`, `testnet`); any mismatch with the SLA `cluster` field is rejected
//! with `Custom(261)` (TransferClusterMismatch).
//!
//! NOTE: full pre/post-balance verification will arrive once Task 14.6 / 14.7 lands
//! the mocked-RPC test harness; the binary currently boots a profile registry,
//! starts the chain monitor + worker, and serves the registry HTTP routes.

use std::{
    collections::VecDeque,
    env,
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
use oracle_rwa_transfer::{
    evaluator::TransferEvaluator,
    evidence::TransferEvidence,
    sla::{TransferCluster, TransferSla},
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
            "oracle_rwa_transfer=info,oracle_common=info,tower_http=info".into()
        }))
        .init();

    let config = OracleConfig::from_env().context("loading OracleConfig from env")?;
    let cluster = parse_cluster(&env::var("TRANSFER_CLUSTER").unwrap_or_else(|_| "devnet".into()))?;

    info!("oracle-rwa-transfer cluster={cluster:?}");

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
            "/var/lib/oracle/rwa-transfer/blobs",
        )),
    )
    .await
    .map_err(|e| anyhow::anyhow!("storage backend init: {e}"))?;

    let evaluator = Arc::new(TransferEvaluator::new(cluster));
    let fetcher_cfg = Arc::new(FetcherConfig {
        mirrors: config.evidence_registry_urls.clone(),
        auth_header: config.evidence_registry_auth_header.clone(),
        max_retries: config.evidence_fetch_max_retries,
        retry_base: Duration::from_millis(config.evidence_fetch_retry_base_ms),
    });
    let sla_fetcher: Arc<RegistryJsonFetcher<TransferSla>> =
        Arc::new(RegistryJsonFetcher::new(http.clone(), fetcher_cfg.clone()));
    let evidence_fetcher: Arc<RegistryJsonFetcher<TransferEvidence>> =
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
    let (job_tx, job_rx) = mpsc::channel(config.job_channel_capacity);
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
        job_tx: job_tx.clone(),
    });

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
             spec/rwa-transfer/v1/NORMATIVE.md"
                .into(),
        ),
        // Pinned cluster so sellers / buyers / pr402 can sanity-check before
        // funding (otherwise a Devnet-vs-Mainnet mismatch surfaces only as
        // a wasted on-chain settlement with `Custom(258) ClusterMismatch`).
        cluster: Some(
            match cluster {
                TransferCluster::MainnetBeta => "mainnet-beta",
                TransferCluster::Devnet => "devnet",
                TransferCluster::Testnet => "testnet",
            }
            .to_string(),
        ),
    };
    let app =
        create_core_router(state.clone()).nest("/v1/registry", registry_router(registry_state));

    let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
    info!("HTTP server listening on {}", config.bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

fn parse_cluster(s: &str) -> anyhow::Result<TransferCluster> {
    Ok(match s.trim().to_ascii_lowercase().as_str() {
        "mainnet-beta" | "mainnet" => TransferCluster::MainnetBeta,
        "devnet" => TransferCluster::Devnet,
        "testnet" => TransferCluster::Testnet,
        other => anyhow::bail!("invalid TRANSFER_CLUSTER: {other}"),
    })
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
