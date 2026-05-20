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
            Ok(Ok((sig, outcome_result, resolution_hash, evidence_keys))) => {
                if let Some(db) = &state.db {
                    let _ = db
                        .record_settled(&job, &outcome_result, sig.as_deref(), &resolution_hash)
                        .await;
                    // Wave A §1.3 / §2.2.1 — index evidence keys for cross-payment
                    // replay protection of *future* evaluations. Only on approve so
                    // that rejected attempts don't poison the index against the seller.
                    if outcome_result.approved {
                        for ek in &evidence_keys {
                            if let Err(e) = db
                                .record_evidence_key(&job.payment_uid, &ek.kind, &ek.value)
                                .await
                            {
                                warn!(error = %e, kind = %ek.kind, "evidence-key insert failed");
                            }
                        }
                    }
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

                // ─── Active Guardian: retry or fail-closed reject ───────────
                if e.is_retriable() {
                    let now = chrono::Utc::now().timestamp();
                    let near_expiry =
                        now > job.expires_at - state.config.guardian_reject_safety_margin_sec;
                    let retries_exhausted =
                        job.retry_count >= state.config.guardian_max_retry_attempts;

                    if near_expiry || retries_exhausted {
                        // Fail-closed: protect buyer by issuing REJECT before expiry.
                        let reason_code = if retries_exhausted {
                            crate::error::guardian_reason::EVALUATION_TIMEOUT
                        } else {
                            e.guardian_reason_code()
                        };
                        info!(
                            "Guardian REJECT for {uid_hex}: near_expiry={near_expiry}, \
                             retries_exhausted={retries_exhausted}, reason={reason_code}"
                        );
                        // Check eligibility first to avoid wasting SOL on already-resolved payments.
                        let eligible = settler::is_eligible(&state.rpc, &state.config, &job)
                            .await
                            .unwrap_or(false);
                        if eligible {
                            let resolution_hash = settler::compute_resolution_hash(
                                &job,
                                "x402/oracles/guardian/v1",
                                &crate::types::EvaluationResult {
                                    approved: false,
                                    resolution_reason: reason_code,
                                    checks: vec![],
                                },
                                serde_json::json!({
                                    "reason": e.to_string(),
                                    "retryCount": job.retry_count,
                                }),
                            )
                            .unwrap_or([0u8; 32]);
                            match settler::settle(
                                &state.rpc,
                                &state.config,
                                &job,
                                false,
                                reason_code,
                                resolution_hash,
                            )
                            .await
                            {
                                Ok(sig) => {
                                    info!("Guardian reject settled for {uid_hex}: sig={sig}");
                                    let mut h = state.health.write().await;
                                    h.guardian_rejects_issued =
                                        h.guardian_rejects_issued.saturating_add(1);
                                }
                                Err(settle_err) => {
                                    warn!(
                                        "Guardian reject tx failed for {uid_hex}: {settle_err} \
                                         (payment may already be resolved)"
                                    );
                                }
                            }
                        } else {
                            info!(
                                "Guardian skip for {uid_hex}: payment no longer eligible \
                                 (already resolved or expired)"
                            );
                        }
                        // Remove from in-flight tracking regardless of settle outcome.
                        attempts_mem.lock().await.remove(&uid);
                        if let Some(db) = &state.db {
                            let _ = db
                                .record_dead_letter(&job, &format!("guardian reject: {e}"))
                                .await;
                        }
                    } else {
                        // Re-queue with exponential backoff.
                        let mut requeued_job = job.clone();
                        requeued_job.retry_count += 1;
                        let delay_sec = state
                            .config
                            .guardian_retry_initial_delay_sec
                            .saturating_mul(2u64.saturating_pow(requeued_job.retry_count.min(10)))
                            .min(state.config.guardian_retry_max_delay_sec);
                        info!(
                            "Guardian retry {}/{} for {uid_hex} in {delay_sec}s",
                            requeued_job.retry_count, state.config.guardian_max_retry_attempts
                        );
                        {
                            let mut h = state.health.write().await;
                            h.guardian_retries_total = h.guardian_retries_total.saturating_add(1);
                        }
                        // Allow the uid to be re-processed on the next receive.
                        processed_mem.lock().await.remove(&uid);
                        // Sleep then re-send to the channel. If the channel is full
                        // or closed, the job is lost — acceptable because the backfill
                        // on next restart will re-emit it.
                        let tx_clone = state.job_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(delay_sec)).await;
                            let _ = tx_clone.send(requeued_job).await;
                        });
                    }
                } else {
                    // Non-retriable error: dead-letter as before.
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
) -> Result<
    (
        Option<String>,
        crate::types::EvaluationResult,
        [u8; 32],
        Vec<crate::types::EvidenceKey>,
    ),
    crate::error::OracleError,
> {
    // Eligibility — defense-in-depth so we don't waste SOL on
    // payments that have already settled, expired, or been reassigned.
    if !settler::is_eligible(&state.rpc, &state.config, job).await? {
        return Err(crate::error::OracleError::Evaluation(format!(
            "payment {} ineligible",
            hex::encode(job.payment_uid)
        )));
    }

    // Build the optional ledger probe. We materialize the `Arc<dyn LedgerProbe>`
    // here (rather than storing it on `AppState`) because the trait object
    // lifetime is tied to the borrow we hand to `EvaluationContext`, and
    // `state.db: Option<OracleDb>` is the canonical source of truth.
    let ledger_probe: Option<Arc<dyn crate::evaluator::LedgerProbe>> = state
        .db
        .as_ref()
        .map(|db| Arc::new(db.clone()) as Arc<dyn crate::evaluator::LedgerProbe>);

    let ctx = crate::evaluator::EvaluationContext {
        rpc: &state.rpc,
        http: &state.http,
        job,
        strict: state.config.strict_profile,
        ledger: ledger_probe.as_ref(),
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
    Ok((
        Some(sig),
        outcome.result,
        outcome.resolution_hash,
        outcome.evidence_keys,
    ))
}
