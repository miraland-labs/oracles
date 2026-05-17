//! Evaluation worker: consumes `EvaluationJob`s from the chain-monitor channel,
//! drives them through the pipeline, settles on-chain, and updates the ledger /
//! stats / health gauges.
//!
//! Dedup precedence (matches the original single-binary semantics):
//! 1. **Ledger first** — `OracleDb::is_terminal(payment_uid)` returns true iff a
//!    prior run already settled or dead-lettered the job, in which case the worker
//!    skips immediately.
//! 2. **In-memory fallback** — when `DATABASE_URL` is unset, a `HashSet` of
//!    `payment_uid`s deduplicates within the process's lifetime. The fallback is
//!    intentionally lossy (a restart re-runs everything that didn't settle) — see
//!    design.md §Operational Architecture; production deployments are expected to
//!    set `DATABASE_URL`.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use chrono::Utc;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

use crate::{chain, pipeline, server::AppState, settler, types::EvaluationJob};

/// Run the evaluation worker until the job channel is closed.
///
/// Spawns inside `tokio::main` (the binary's entrypoint) and is expected to live for
/// the process's lifetime.
pub async fn run_worker(state: Arc<AppState>, mut job_rx: mpsc::Receiver<EvaluationJob>) {
    let processed_mem: Arc<Mutex<HashSet<[u8; 32]>>> = Arc::new(Mutex::new(HashSet::new()));
    let attempts_mem: Arc<Mutex<HashMap<[u8; 32], u32>>> = Arc::new(Mutex::new(HashMap::new()));

    info!("Evaluation worker started");
    while let Some(job) = job_rx.recv().await {
        let uid_hex = hex::encode(job.payment_uid);
        let uid = job.payment_uid;

        {
            let mut h = state.health.write().await;
            h.queue_depth = job_rx.len();
        }

        // ----- dedup -----
        if let Some(ledger) = &state.db {
            match ledger.is_terminal(&uid).await {
                Ok(true) => {
                    warn!("Skipping {uid_hex}: ledger already marks terminal");
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(error = %e, "ledger is_terminal probe failed; proceeding cautiously");
                }
            }
        } else {
            let mut seen = processed_mem.lock().await;
            if !seen.insert(uid) {
                warn!("Skipping in-memory duplicate {uid_hex} (ledger disabled)");
                continue;
            }
        }

        // ----- ledger lifecycle: detected → queued → running -----
        if let Some(db) = &state.db {
            let _ = db.record_detected(&job).await;
            let _ = db.record_queued(&job).await;
        }

        info!(payment_uid = %uid_hex, "processing job");
        let attempt_count: u32 = if let Some(ledger) = &state.db {
            match ledger.record_started(&job).await {
                Ok(()) => match ledger.attempt_count(&uid).await {
                    Ok(n) if n > 0 => n as u32,
                    _ => 1,
                },
                Err(e) => {
                    warn!(error = %e, "ledger record_started failed");
                    1
                }
            }
        } else {
            let mut attempts = attempts_mem.lock().await;
            let n = attempts.entry(uid).or_insert(0);
            *n += 1;
            *n
        };

        // ----- pipeline + settle -----
        let timeout = Duration::from_millis(state.config.evaluation_timeout_ms);
        let outcome_res =
            tokio::time::timeout(timeout, run_pipeline_and_settle(&state, &job)).await;

        match outcome_res {
            Ok(Ok((sig, outcome_result, resolution_hash))) => {
                if let Some(db) = &state.db {
                    let _ = db
                        .record_settled(&job, &outcome_result, sig.as_deref(), &resolution_hash)
                        .await;
                    chain::persist_slot_watermark(db, &state.health).await;
                }
                let mut stats = state.stats.write().await;
                stats.total_evaluated += 1;
                if outcome_result.approved {
                    stats.total_approved += 1;
                } else {
                    stats.total_rejected += 1;
                }
                stats.last_evaluation_at = Some(Utc::now().to_rfc3339());
                attempts_mem.lock().await.remove(&uid);
            }
            Ok(Err(e)) => {
                error!("pipeline error for {uid_hex}: {e}");
                let should_dead_letter = attempt_count >= state.config.dead_letter_max_attempts;
                if let Some(db) = &state.db {
                    let _ = if should_dead_letter {
                        db.record_dead_letter(&job, &e.to_string()).await
                    } else {
                        db.record_failed(&job, &e.to_string()).await
                    };
                }
                if !should_dead_letter {
                    processed_mem.lock().await.remove(&uid);
                }
                let mut stats = state.stats.write().await;
                stats.total_errors += 1;
                if should_dead_letter {
                    stats.total_dead_letter += 1;
                    attempts_mem.lock().await.remove(&uid);
                }
            }
            Err(_) => {
                warn!(
                    "pipeline timeout for {uid_hex} ({}ms)",
                    state.config.evaluation_timeout_ms
                );
                let should_dead_letter = attempt_count >= state.config.dead_letter_max_attempts;
                if let Some(db) = &state.db {
                    let _ = if should_dead_letter {
                        db.record_dead_letter(&job, "pipeline timeout").await
                    } else {
                        db.record_failed(&job, "pipeline timeout").await
                    };
                }
                if !should_dead_letter {
                    processed_mem.lock().await.remove(&uid);
                }
                let mut stats = state.stats.write().await;
                stats.total_errors += 1;
                if should_dead_letter {
                    stats.total_dead_letter += 1;
                    attempts_mem.lock().await.remove(&uid);
                }
            }
        }
    }

    info!("Evaluation worker exiting (job channel closed)");
}

async fn run_pipeline_and_settle(
    state: &Arc<AppState>,
    job: &EvaluationJob,
) -> Result<(Option<String>, crate::types::EvaluationResult, [u8; 32]), crate::error::OracleError> {
    // Eligibility — defense-in-depth so we don't waste SOL on
    // payments that have already settled, expired, or been reassigned.
    if !settler::is_eligible(&state.rpc, &state.config, job).await? {
        return Err(crate::error::OracleError::Evaluation(format!(
            "payment {} ineligible",
            hex::encode(job.payment_uid)
        )));
    }

    let ctx = crate::evaluator::EvaluationContext {
        rpc: &state.rpc,
        http: &state.http,
        job,
        strict: state.config.strict_profile,
    };
    let outcome = pipeline::run_pipeline(&state.profiles, &ctx, job.sla_bytes.as_ref()).await?;

    let sig = settler::settle(
        &state.rpc,
        &state.config,
        job,
        outcome.result.approved,
        outcome.result.resolution_reason,
        outcome.resolution_hash,
    )
    .await?;
    Ok((Some(sig), outcome.result, outcome.resolution_hash))
}
