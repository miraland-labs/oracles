//! Chain monitor + startup backfill.
//!
//! The monitor subscribes to `logsSubscribe` against `escrow_program_id`, decodes
//! `DeliverySubmittedEvent` from `Program data:` lines, derives the payment PDA from
//! the matching `SubmitDelivery` instruction's `accounts[4]`, reads the `Payment`
//! account, and emits an `EvaluationJob` over the worker channel.
//!
//! The implementation here lifts the proven oracle-monitoring logic from the earlier
//! single-binary deployment into `oracle-common`
//! verbatim where possible, with the addition of an SLA-bytes pre-fetch so the
//! worker can dispatch by `profile_id` without re-fetching the SLA.

use std::{str::FromStr, sync::Arc, time::Duration};

use base64::{engine::general_purpose::STANDARD as B64_ENGINE, Engine};
use bytes::Bytes;
use futures_util::StreamExt;
use sla_escrow_api::{event::DeliverySubmittedEvent, instruction::EscrowInstruction};
use solana_client::{
    nonblocking::{pubsub_client::PubsubClient, rpc_client::RpcClient},
    rpc_client::GetConfirmedSignaturesForAddress2Config,
    rpc_config::{RpcTransactionConfig, RpcTransactionLogsConfig, RpcTransactionLogsFilter},
};
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signature};
use solana_transaction_status::{
    option_serializer::OptionSerializer, EncodedTransaction, UiCompiledInstruction, UiInstruction,
    UiMessage, UiParsedInstruction, UiPartiallyDecodedInstruction, UiTransactionEncoding,
};
use solana_transaction_status_client_types::ParsedAccount;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::{
    config::OracleConfig,
    db::OracleDb,
    error::OracleError,
    types::{EvaluationJob, RuntimeHealth, PARAM_LAST_SEEN_SLOT},
};

/// Parse `DeliverySubmittedEvent` from RPC log lines (`sol_log_data` →
/// `Program data: <base64>`). Returns every event found in the slice.
pub fn parse_delivery_events_from_logs(logs: &[String]) -> Vec<DeliverySubmittedEvent> {
    const PREFIX: &str = "Program data: ";
    let mut out = Vec::new();
    let expected = std::mem::size_of::<DeliverySubmittedEvent>();
    for line in logs {
        let line = line.trim();
        let Some(b64) = line.strip_prefix(PREFIX) else {
            continue;
        };
        let Ok(bytes) = B64_ENGINE.decode(b64.trim()) else {
            continue;
        };
        if bytes.len() != expected {
            continue;
        }
        if let Ok(ev) = bytemuck::try_from_bytes::<DeliverySubmittedEvent>(bytes.as_slice()) {
            out.push(*ev);
        }
    }
    out
}

fn submit_delivery_discriminant() -> u8 {
    EscrowInstruction::SubmitDelivery as u8
}

/// Returns the payment PDA from a partially-decoded CPI-style instruction whose
/// program matches `escrow_program`.
fn payment_from_partial_ix(
    ix: &UiPartiallyDecodedInstruction,
    escrow_program: &str,
) -> Option<Pubkey> {
    if ix.program_id != escrow_program {
        return None;
    }
    let data = bs58::decode(&ix.data).into_vec().ok()?;
    if data.first().copied() != Some(submit_delivery_discriminant()) {
        return None;
    }
    // submit_delivery account layout: [seller, bank, config, escrow, payment]
    if ix.accounts.len() < 5 {
        return None;
    }
    ix.accounts[4].parse().ok()
}

/// Returns the payment PDA from a compiled instruction + full account-key list.
fn payment_from_compiled_ix(
    ix: &UiCompiledInstruction,
    account_keys: &[ParsedAccount],
    escrow_program: &Pubkey,
) -> Option<Pubkey> {
    let program_id = account_keys
        .get(ix.program_id_index as usize)?
        .pubkey
        .parse::<Pubkey>()
        .ok()?;
    if program_id != *escrow_program {
        return None;
    }
    let data = bs58::decode(&ix.data).into_vec().ok()?;
    if data.first().copied() != Some(submit_delivery_discriminant()) {
        return None;
    }
    let pay_idx = *ix.accounts.get(4)? as usize;
    account_keys.get(pay_idx)?.pubkey.parse().ok()
}

fn collect_payment_candidates_from_instructions(
    instructions: &[UiInstruction],
    account_keys: &[ParsedAccount],
    escrow_program: &Pubkey,
) -> Vec<Pubkey> {
    let escrow_str = escrow_program.to_string();
    let mut out = Vec::new();
    for ix in instructions {
        match ix {
            UiInstruction::Parsed(UiParsedInstruction::PartiallyDecoded(p)) => {
                if let Some(pk) = payment_from_partial_ix(p, &escrow_str) {
                    out.push(pk);
                }
            }
            UiInstruction::Parsed(UiParsedInstruction::Parsed(_)) => {}
            UiInstruction::Compiled(c) => {
                if let Some(pk) = payment_from_compiled_ix(c, account_keys, escrow_program) {
                    out.push(pk);
                }
            }
        }
    }
    out
}

/// Read the on-chain `Payment` account at `payment_pubkey` and turn it into an
/// [`EvaluationJob`] iff it is currently this oracle's responsibility:
/// `oracle_authority == oracle_pubkey`, `delivery_timestamp != 0`,
/// `resolution_state == 0`.
pub async fn read_payment(
    rpc: &RpcClient,
    payment_pubkey: &Pubkey,
    oracle_pubkey: &Pubkey,
) -> Result<Option<EvaluationJob>, OracleError> {
    use sla_escrow_api::state::Payment;

    let account = rpc
        .get_account_with_commitment(payment_pubkey, CommitmentConfig::confirmed())
        .await?
        .value
        .ok_or_else(|| OracleError::Chain(format!("Payment account {payment_pubkey} not found")))?;

    if account.data.len() < 8 + std::mem::size_of::<Payment>() {
        return Err(OracleError::Chain("Payment account data too short".into()));
    }
    let payment: &Payment =
        bytemuck::from_bytes(&account.data[8..8 + std::mem::size_of::<Payment>()]);

    if payment.oracle_authority != *oracle_pubkey {
        return Ok(None);
    }

    if payment.delivery_timestamp == 0 || payment.resolution_state != 0 {
        return Ok(None);
    }

    Ok(Some(EvaluationJob {
        payment_uid: payment.payment_uid,
        payment_pubkey: *payment_pubkey,
        sla_hash: payment.sla_hash,
        delivery_hash: payment.delivery_hash,
        amount: payment.amount,
        mint: payment.mint,
        oracle_authority: payment.oracle_authority,
        expires_at: payment.expires_at,
        // Wave A §1.1: plumb on-chain timestamps so evaluators can enforce the
        // freshness lower bound (`created_at`) and the deadline-side sanity
        // bound (`expires_at - delivery_cutoff_seconds`).
        created_at: payment.created_at,
        delivery_cutoff_seconds: payment.delivery_cutoff_seconds,
        sla_bytes: None, // hoisted in by the chain monitor below if available
        retry_count: 0,
    }))
}

/// Pre-fetch SLA bytes for an emitted job. Best effort: if every mirror fails
/// (or the registry simply doesn't have the document yet), return `None` and
/// let the pipeline do the fetch with proper retry / fail-closed semantics.
async fn try_prefetch_sla(
    http: &reqwest::Client,
    cfg: &OracleConfig,
    sla_hash: &[u8; 32],
) -> Option<Bytes> {
    let hash_hex = hex::encode(sla_hash);
    for base in &cfg.evidence_registry_urls {
        let url = format!("{}/{}", base.trim_end_matches('/'), hash_hex);
        let mut req = http.get(&url);
        if let Some(auth) = &cfg.evidence_registry_auth_header {
            req = req.header(reqwest::header::AUTHORIZATION, auth);
        }
        if let Ok(resp) = req.send().await {
            if resp.status().is_success() {
                if let Ok(b) = resp.bytes().await {
                    return Some(b);
                }
            }
        }
    }
    None
}

/// Two-strategy delivery extraction: prefer the structured `SubmitDelivery` account
/// layout from parsed/compiled instructions; fall back to scanning every account
/// key referenced by the transaction (handy with RPCs that truncate `programData`).
async fn fetch_delivery_job(
    rpc: &RpcClient,
    sig: &Signature,
    config: &OracleConfig,
    http: &reqwest::Client,
) -> Result<Option<EvaluationJob>, OracleError> {
    let tx_config = RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::JsonParsed),
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
    };

    let enc = rpc
        .get_transaction_with_config(sig, tx_config)
        .await
        .map_err(|e| OracleError::Chain(format!("Failed to fetch tx {sig}: {e}")))?;

    let EncodedTransaction::Json(ui_tx) = &enc.transaction.transaction else {
        warn!("getTransaction for {sig} was not JsonParsed; cannot extract layout");
        return Ok(None);
    };

    let oracle_pk = config.oracle_pubkey();
    let escrow = config.escrow_program_id;

    let log_events: Vec<DeliverySubmittedEvent> = enc
        .transaction
        .meta
        .as_ref()
        .and_then(|m| match &m.log_messages {
            OptionSerializer::Some(logs) => Some(parse_delivery_events_from_logs(logs)),
            _ => None,
        })
        .unwrap_or_default();
    let require_match = config.require_event_match;

    if let UiMessage::Parsed(pm) = &ui_tx.message {
        // Strategy 1: structured account-list lookup.
        let mut candidates = collect_payment_candidates_from_instructions(
            &pm.instructions,
            &pm.account_keys,
            &escrow,
        );

        if let Some(OptionSerializer::Some(groups)) =
            enc.transaction.meta.as_ref().map(|m| &m.inner_instructions)
        {
            for g in groups {
                candidates.extend(collect_payment_candidates_from_instructions(
                    &g.instructions,
                    &pm.account_keys,
                    &escrow,
                ));
            }
        }
        candidates.sort_by_key(|p| p.to_string());
        candidates.dedup();

        for pk in candidates {
            if let Ok(Some(mut job)) = read_payment(rpc, &pk, &oracle_pk).await {
                if event_matches_job(&log_events, &job) {
                    job.sla_bytes = try_prefetch_sla(http, config, &job.sla_hash).await;
                    return Ok(Some(job));
                }
                if !require_match && log_events.is_empty() {
                    job.sla_bytes = try_prefetch_sla(http, config, &job.sla_hash).await;
                    return Ok(Some(job));
                }
            }
        }

        if require_match {
            return Ok(None);
        }

        // Strategy 2: scan every account key referenced by the tx.
        let account_keys: Vec<Pubkey> = pm
            .account_keys
            .iter()
            .filter_map(|a| a.pubkey.parse().ok())
            .collect();
        for key in account_keys {
            if let Ok(Some(mut job)) = read_payment(rpc, &key, &oracle_pk).await {
                if event_matches_job(&log_events, &job) || log_events.is_empty() {
                    job.sla_bytes = try_prefetch_sla(http, config, &job.sla_hash).await;
                    return Ok(Some(job));
                }
            }
        }
    }

    Ok(None)
}

fn event_matches_job(events: &[DeliverySubmittedEvent], job: &EvaluationJob) -> bool {
    if events.is_empty() {
        return true;
    }
    events
        .iter()
        .any(|e| e.payment_uid == job.payment_uid && e.delivery_hash == job.delivery_hash)
}

/// Subscribe to `logsSubscribe` and emit `EvaluationJob`s when delivery events appear.
///
/// On WebSocket disconnect, sleeps `RECONNECT_DELAY` and retries indefinitely so the
/// systemd unit's `Restart=on-failure` is reserved for actual binary crashes.
pub async fn monitor_deliveries(
    config: Arc<OracleConfig>,
    rpc: Arc<RpcClient>,
    http: Arc<reqwest::Client>,
    tx: mpsc::Sender<EvaluationJob>,
    health: Arc<RwLock<RuntimeHealth>>,
) {
    const RECONNECT_DELAY: Duration = Duration::from_secs(5);
    loop {
        info!("Connecting to Solana WebSocket at {}", config.solana_ws_url);
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
                        {
                            let mut h = health.write().await;
                            h.websocket_connected = true;
                            h.last_websocket_connected_at = Some(chrono::Utc::now().to_rfc3339());
                            h.last_monitor_error = None;
                        }
                        while let Some(log_response) = stream.next().await {
                            let slot = log_response.context.slot;
                            {
                                let mut h = health.write().await;
                                h.last_websocket_message_at = Some(chrono::Utc::now().to_rfc3339());
                                if slot > h.last_seen_slot {
                                    h.last_seen_slot = slot;
                                }
                            }
                            let logs = &log_response.value.logs;
                            let has_delivery = logs.iter().any(|l| {
                                l.contains("DeliverySubmittedEvent") || l.contains("Program data:")
                            });
                            if !has_delivery {
                                continue;
                            }
                            if let Ok(sig) = log_response.value.signature.parse::<Signature>() {
                                match fetch_delivery_job(&rpc, &sig, &config, &http).await {
                                    Ok(Some(job)) => {
                                        info!(
                                            "New delivery detected: payment_uid={}",
                                            hex::encode(job.payment_uid)
                                        );
                                        {
                                            let mut h = health.write().await;
                                            h.deliveries_observed =
                                                h.deliveries_observed.saturating_add(1);
                                        }
                                        if tx.send(job).await.is_err() {
                                            error!("Job channel closed; stopping monitor");
                                            return;
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(e) => warn!("Failed to process delivery tx: {e}"),
                                }
                            }
                        }
                        warn!("Log subscription stream ended; reconnecting...");
                        let mut h = health.write().await;
                        h.websocket_connected = false;
                        h.last_monitor_error = Some("log subscription stream ended".into());
                    }
                    Err(e) => {
                        error!("Failed to subscribe to logs: {e}");
                        let mut h = health.write().await;
                        h.websocket_connected = false;
                        h.last_monitor_error = Some(e.to_string());
                    }
                }
            }
            Err(e) => {
                error!("WebSocket connection failed: {e}");
                let mut h = health.write().await;
                h.websocket_connected = false;
                h.last_monitor_error = Some(e.to_string());
            }
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// Catch up on deliveries the oracle was offline for.
///
/// Walks `getSignaturesForAddress` backwards from the most recent signature, decodes
/// matching `SubmitDelivery` instructions, and emits jobs. Stops at the high-water
/// `chain.last_seen_slot` watermark stored in `oracle_parameters`.
pub async fn backfill_missed_deliveries(
    config: Arc<OracleConfig>,
    rpc: Arc<RpcClient>,
    http: Arc<reqwest::Client>,
    tx: mpsc::Sender<EvaluationJob>,
    db: Option<OracleDb>,
    health: Arc<RwLock<RuntimeHealth>>,
) {
    if config.backfill_lookback_signatures == 0 {
        return;
    }

    let last_seen_slot: u64 = match &db {
        Some(ledger) => match ledger.get_parameter(PARAM_LAST_SEEN_SLOT).await {
            Ok(Some(raw)) => raw.parse().unwrap_or(0),
            _ => 0,
        },
        None => 0,
    };

    info!(
        "Backfill: scanning up to {} signatures for {} (last_seen_slot={})",
        config.backfill_lookback_signatures, config.escrow_program_id, last_seen_slot
    );

    let limit_per_page: usize = 1000;
    let max = config.backfill_lookback_signatures;
    let mut scanned: usize = 0;
    let mut before: Option<Signature> = None;
    let mut emitted: usize = 0;
    let mut highest_slot: u64 = last_seen_slot;

    'outer: loop {
        let want = (max - scanned).min(limit_per_page);
        if want == 0 {
            break;
        }
        let fetch_config = GetConfirmedSignaturesForAddress2Config {
            before,
            until: None,
            limit: Some(want),
            commitment: Some(CommitmentConfig::confirmed()),
        };
        let batch = match rpc
            .get_signatures_for_address_with_config(&config.escrow_program_id, fetch_config)
            .await
        {
            Ok(batch) => batch,
            Err(e) => {
                warn!("Backfill: getSignaturesForAddress failed: {e}");
                break;
            }
        };
        if batch.is_empty() {
            break;
        }
        for entry in &batch {
            scanned += 1;
            let slot = entry.slot;
            if slot > highest_slot {
                highest_slot = slot;
            }
            if last_seen_slot > 0 && slot <= last_seen_slot {
                break 'outer;
            }
            if entry.err.is_some() {
                continue;
            }
            let sig = match Signature::from_str(&entry.signature) {
                Ok(s) => s,
                Err(_) => continue,
            };
            match fetch_delivery_job(&rpc, &sig, &config, &http).await {
                Ok(Some(job)) => {
                    info!(
                        "Backfill: emitting payment_uid={} (sig={sig}, slot={slot})",
                        hex::encode(job.payment_uid)
                    );
                    if tx.send(job).await.is_err() {
                        warn!("Backfill: job channel closed; aborting");
                        break 'outer;
                    }
                    emitted += 1;
                }
                Ok(None) => {}
                Err(e) => debug!("Backfill: skip {sig}: {e}"),
            }
            before = Some(sig);
        }
        if batch.len() < want {
            break;
        }
    }

    if highest_slot > 0 {
        if let Some(ledger) = &db {
            if let Err(e) = ledger
                .set_parameter(PARAM_LAST_SEEN_SLOT, &highest_slot.to_string())
                .await
            {
                warn!(error = %e, "Backfill: failed to persist last_seen_slot");
            }
        }
        let mut h = health.write().await;
        if highest_slot > h.last_seen_slot {
            h.last_seen_slot = highest_slot;
        }
    }

    info!("Backfill complete: scanned {scanned}, emitted {emitted}, last_seen_slot={highest_slot}");
}

/// Persist the current `last_seen_slot` from `RuntimeHealth` into the ledger. Call
/// this from the worker after each successful settle so a restart's backfill skips
/// already-seen events.
pub async fn persist_slot_watermark(db: &OracleDb, health: &Arc<RwLock<RuntimeHealth>>) {
    let slot = health.read().await.last_seen_slot;
    if slot == 0 {
        return;
    }
    if let Err(e) = db
        .set_parameter(PARAM_LAST_SEEN_SLOT, &slot.to_string())
        .await
    {
        debug!(error = %e, "ledger: failed to persist last_seen_slot");
    }
}

#[cfg(test)]
mod tests {
    use solana_sdk::pubkey::Pubkey;

    use super::*;

    #[test]
    fn parse_delivery_event_round_trip() {
        let ev = DeliverySubmittedEvent {
            payment_uid: [7u8; 32],
            delivery_hash: [8u8; 32],
            timestamp: 1_700_000_000,
            seller: Pubkey::new_unique(),
        };
        let raw = bytemuck::bytes_of(&ev);
        let line = format!("Program data: {}", B64_ENGINE.encode(raw));
        let parsed = parse_delivery_events_from_logs(&[line]);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].payment_uid, ev.payment_uid);
        assert_eq!(parsed[0].delivery_hash, ev.delivery_hash);
        assert_eq!(parsed[0].timestamp, ev.timestamp);
    }

    #[test]
    fn parse_delivery_event_skips_garbage_lines() {
        let lines = vec![
            "Program log: nothing".to_string(),
            "Program data: not-base64-!!!".to_string(),
            "Program data: aGVsbG8=".to_string(), // valid b64 but wrong size
        ];
        let parsed = parse_delivery_events_from_logs(&lines);
        assert!(parsed.is_empty());
    }

    #[test]
    fn event_matches_job_when_both_match() {
        let ev = DeliverySubmittedEvent {
            payment_uid: [1u8; 32],
            delivery_hash: [2u8; 32],
            timestamp: 0,
            seller: Pubkey::new_unique(),
        };
        let job = EvaluationJob {
            payment_uid: [1u8; 32],
            payment_pubkey: Pubkey::new_unique(),
            sla_hash: [0u8; 32],
            delivery_hash: [2u8; 32],
            amount: 0,
            mint: Pubkey::new_unique(),
            oracle_authority: Pubkey::new_unique(),
            expires_at: 0,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        };
        assert!(event_matches_job(&[ev], &job));
    }

    #[test]
    fn event_matches_job_with_no_events_is_permissive() {
        let job = EvaluationJob {
            payment_uid: [1u8; 32],
            payment_pubkey: Pubkey::new_unique(),
            sla_hash: [0u8; 32],
            delivery_hash: [2u8; 32],
            amount: 0,
            mint: Pubkey::new_unique(),
            oracle_authority: Pubkey::new_unique(),
            expires_at: 0,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        };
        // Empty events → permissive (caller decides via require_event_match).
        assert!(event_matches_job(&[], &job));
    }

    #[test]
    fn event_matches_job_rejects_uid_mismatch() {
        let ev = DeliverySubmittedEvent {
            payment_uid: [9u8; 32],
            delivery_hash: [2u8; 32],
            timestamp: 0,
            seller: Pubkey::new_unique(),
        };
        let job = EvaluationJob {
            payment_uid: [1u8; 32],
            payment_pubkey: Pubkey::new_unique(),
            sla_hash: [0u8; 32],
            delivery_hash: [2u8; 32],
            amount: 0,
            mint: Pubkey::new_unique(),
            oracle_authority: Pubkey::new_unique(),
            expires_at: 0,
            created_at: 0,
            delivery_cutoff_seconds: 0,
            sla_bytes: None,
            retry_count: 0,
        };
        assert!(!event_matches_job(&[ev], &job));
    }
}
