//! `logsSubscribe` ingester + startup backfill.
//!
//! Mirrors the proven oracle-common chain-monitor pattern but stripped to the
//! reputation-only essentials: no oracle eligibility check, no SLA prefetch,
//! no job channel. Just `(signature, log) → decoded events → Postgres rows`.
//!
//! Reconnect indefinitely on WebSocket failure so the systemd unit's
//! `Restart=on-failure` is reserved for actual binary crashes.

use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use solana_client::{
    nonblocking::{pubsub_client::PubsubClient, rpc_client::RpcClient},
    rpc_client::GetConfirmedSignaturesForAddress2Config,
    rpc_config::{RpcTransactionConfig, RpcTransactionLogsConfig, RpcTransactionLogsFilter},
};
use solana_sdk::{commitment_config::CommitmentConfig, signature::Signature};
use solana_transaction_status::{EncodedConfirmedTransactionWithStatusMeta, UiTransactionEncoding};
use solana_transaction_status_client_types::option_serializer::OptionSerializer;
use tracing::{error, info, warn};

use crate::{config::ReputationConfig, decoder, store::EventStore};

const RECONNECT_DELAY: Duration = Duration::from_secs(5);
const FETCH_TX_RETRIES: u32 = 3;
const FETCH_TX_RETRY_BASE_MS: u64 = 200;

/// Subscribe to escrow program logs and persist every decoded
/// payment-lifecycle event. Loops forever, reconnecting on disconnect.
pub async fn run_subscription(
    config: Arc<ReputationConfig>,
    rpc: Arc<RpcClient>,
    store: EventStore,
) {
    loop {
        info!(ws_url = %config.solana_ws_url, "Connecting to Solana WebSocket");
        match PubsubClient::new(&config.solana_ws_url).await {
            Ok(pubsub) => {
                let filter =
                    RpcTransactionLogsFilter::Mentions(vec![config.escrow_program_id.to_string()]);
                let log_config = RpcTransactionLogsConfig {
                    commitment: Some(CommitmentConfig::confirmed()),
                };
                match pubsub.logs_subscribe(filter, log_config).await {
                    Ok((mut stream, _unsub)) => {
                        info!("Subscribed to escrow program logs");
                        while let Some(resp) = stream.next().await {
                            let slot = resp.context.slot;

                            // Quick filter — most logs aren't `Program data:` lines.
                            let has_program_data =
                                resp.value.logs.iter().any(|l| l.contains("Program data:"));
                            if !has_program_data {
                                continue;
                            }

                            let Ok(sig) = resp.value.signature.parse::<Signature>() else {
                                continue;
                            };

                            // Fetch the full tx (we need block_time, which is
                            // not in the logs subscription response).
                            match fetch_tx(&rpc, &sig).await {
                                Ok(Some(tx)) => {
                                    persist_events_from_tx(
                                        &store,
                                        &resp.value.signature,
                                        slot,
                                        &tx,
                                    )
                                    .await;
                                }
                                Ok(None) => {} // tx not found yet; will replay on retry
                                Err(e) => warn!(error = %e, %sig, "fetch tx failed"),
                            }
                        }
                        warn!("Log subscription stream ended; reconnecting");
                    }
                    Err(e) => error!(error = %e, "logs_subscribe failed"),
                }
            }
            Err(e) => error!(error = %e, "WebSocket connection failed"),
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// Catch up on signatures we missed while offline. Bounded by
/// `config.backfill_lookback_signatures` so we never replay history forever.
pub async fn run_backfill(
    config: Arc<ReputationConfig>,
    rpc: Arc<RpcClient>,
    store: &EventStore,
) -> anyhow::Result<usize> {
    if config.backfill_lookback_signatures == 0 {
        info!("Backfill disabled (REPUTATION_BACKFILL_LOOKBACK_SIGNATURES=0)");
        return Ok(0);
    }
    info!(
        lookback = config.backfill_lookback_signatures,
        "Starting startup backfill"
    );

    let mut total_events = 0usize;
    let mut before: Option<Signature> = None;
    let mut signatures_seen = 0usize;

    while signatures_seen < config.backfill_lookback_signatures {
        let cfg = GetConfirmedSignaturesForAddress2Config {
            before,
            until: None,
            limit: Some(1_000.min(config.backfill_lookback_signatures - signatures_seen)),
            commitment: Some(CommitmentConfig::confirmed()),
        };
        let batch = rpc
            .get_signatures_for_address_with_config(&config.escrow_program_id, cfg)
            .await?;
        if batch.is_empty() {
            break;
        }

        for entry in &batch {
            signatures_seen += 1;
            let Ok(sig) = entry.signature.parse::<Signature>() else {
                continue;
            };
            match fetch_tx(&rpc, &sig).await {
                Ok(Some(tx)) => {
                    let n = persist_events_from_tx(store, &entry.signature, entry.slot, &tx).await;
                    total_events = total_events.saturating_add(n);
                }
                Ok(None) => {}
                Err(e) => warn!(error = %e, %sig, "backfill fetch tx failed"),
            }
        }

        before = batch
            .last()
            .and_then(|e| e.signature.parse::<Signature>().ok());
        if before.is_none() {
            break;
        }
    }

    info!(
        signatures_seen,
        events_inserted = total_events,
        "Backfill complete"
    );
    Ok(total_events)
}

async fn fetch_tx(
    rpc: &RpcClient,
    sig: &Signature,
) -> anyhow::Result<Option<EncodedConfirmedTransactionWithStatusMeta>> {
    let cfg = RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::Json),
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
    };
    for attempt in 0..FETCH_TX_RETRIES {
        match rpc.get_transaction_with_config(sig, cfg).await {
            Ok(tx) => return Ok(Some(tx)),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("not found") || msg.contains("BlockNotAvailable") {
                    return Ok(None);
                }
                if attempt + 1 == FETCH_TX_RETRIES {
                    return Err(anyhow::anyhow!(msg));
                }
                let backoff = FETCH_TX_RETRY_BASE_MS << attempt;
                tokio::time::sleep(Duration::from_millis(backoff)).await;
            }
        }
    }
    Ok(None)
}

/// Decode every `Program data:` log line in a fetched transaction and persist
/// the resulting events. Returns the count of newly-inserted rows (excludes
/// idempotent conflicts).
async fn persist_events_from_tx(
    store: &EventStore,
    signature: &str,
    slot: u64,
    tx: &EncodedConfirmedTransactionWithStatusMeta,
) -> usize {
    let block_time = tx.block_time.unwrap_or(0);
    let Some(meta) = tx.transaction.meta.as_ref() else {
        return 0;
    };
    let logs = match &meta.log_messages {
        OptionSerializer::Some(v) => v,
        _ => return 0,
    };

    let decoded = decoder::decode_program_data_lines(logs);
    if decoded.is_empty() {
        return 0;
    }

    let mut inserted = 0usize;
    for (log_index, event) in decoded {
        match store
            .insert_event(signature, log_index as i32, slot as i64, block_time, &event)
            .await
        {
            Ok(true) => inserted += 1,
            Ok(false) => {} // idempotent conflict — already indexed
            Err(e) => warn!(error = %e, %signature, log_index, "insert event failed"),
        }
    }

    if inserted > 0 {
        info!(%signature, slot, inserted, "indexed events");
        let _ = store.write_cursor(slot as i64).await;
    }
    inserted
}
