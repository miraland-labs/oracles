//! Reputation indexer binary.
//!
//! Subscribes to `sla-escrow` program logs, decodes payment-lifecycle events,
//! and writes them to Postgres. Runs forever; `Restart=on-failure` in the
//! systemd unit covers actual binary crashes.

use std::sync::Arc;

use anyhow::Context;
use oracle_reputation::{
    config::ReputationConfig,
    ingester::{run_backfill, run_subscription},
    store::EventStore,
};
use solana_client::nonblocking::rpc_client::RpcClient;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present (operators may run from a deployed dir).
    let _ = dotenvy::dotenv();

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("oracle_reputation=info,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .init();

    let config = Arc::new(ReputationConfig::from_env().context("ReputationConfig::from_env")?);
    info!(
        rpc = %config.solana_rpc_url,
        ws = %config.solana_ws_url,
        program = %config.escrow_program_id,
        "Starting oracle-reputation indexer"
    );

    let store = EventStore::connect(&config.database_url).context("connect Postgres")?;
    let rpc = Arc::new(RpcClient::new(config.solana_rpc_url.clone()));

    // Best-effort backfill before going live; failures here log but don't
    // fatal — the live subscription is the source of truth.
    if let Err(e) = run_backfill(config.clone(), rpc.clone(), &store).await {
        tracing::warn!(error = %e, "backfill failed; continuing with live subscription only");
    }

    run_subscription(config, rpc, store).await;
    Ok(())
}
