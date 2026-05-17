//! Shared types used across the chain monitor, pipeline, evaluator, settler, server.
//!
//! These types are intentionally domain-agnostic — the per-family SLA / Evidence shapes
//! live in each family crate, parameterized into the `OracleEvaluator` trait.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// Key name in `oracle_parameters` for the most recently processed chain slot.
/// Used on restart to backfill any deliveries that landed while this oracle was offline.
pub const PARAM_LAST_SEEN_SLOT: &str = "chain.last_seen_slot";

/// A pending evaluation job emitted by the chain monitor and consumed by the worker.
///
/// All fields are populated from on-chain `Payment` state at job-emission time. The
/// `sla_bytes` field is the registry-fetched SLA payload — the chain monitor fetches it
/// once and attaches it to the job so the worker never re-fetches.
#[derive(Debug, Clone)]
pub struct EvaluationJob {
    pub payment_uid: [u8; 32],
    pub payment_pubkey: Pubkey,
    pub sla_hash: [u8; 32],
    pub delivery_hash: [u8; 32],
    pub amount: u64,
    pub mint: Pubkey,
    pub oracle_authority: Pubkey,
    pub expires_at: i64,
    /// Raw SLA bytes pre-fetched by the chain monitor; the worker parses them once
    /// (cheap `SlaEnvelope` peek for dispatch, then full parse inside the runner).
    /// `None` when the job came from a path that couldn't fetch (e.g. manual evaluate
    /// before the registry was reachable); the pipeline will then perform a one-shot
    /// fetch itself.
    pub sla_bytes: Option<Bytes>,
}

/// Result of the oracle's SLA evaluation.
///
/// `resolution_reason` is drawn from `sla_escrow_api::resolution::ResolutionReason`:
/// standard codes 0..=255 are interoperable; custom codes ≥256 are partitioned per
/// family in [`crate::resolution_codes`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvaluationResult {
    pub approved: bool,
    pub resolution_reason: u16,
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// Outcome of the full pipeline: the evaluator's decision plus the on-chain settlement
/// digest and (if settlement succeeded) the transaction signature.
#[derive(Debug, Clone)]
pub struct EvaluationOutcome {
    pub result: EvaluationResult,
    pub resolution_hash: [u8; 32],
    pub signature: Option<String>,
}

/// Live runtime health gauges, surfaced via `GET /health` and `GET /metrics`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RuntimeHealth {
    pub websocket_connected: bool,
    pub last_websocket_connected_at: Option<String>,
    pub last_websocket_message_at: Option<String>,
    pub last_monitor_error: Option<String>,
    pub queue_depth: usize,
    /// Monotonic counter of deliveries emitted by the chain monitor (accepted jobs).
    pub deliveries_observed: u64,
    /// Most recently observed chain slot (via log notifications). Informational for
    /// backfill diagnostics.
    pub last_seen_slot: u64,
}

/// Minimal SLA envelope the dispatcher reads to determine which `OracleEvaluator` to
/// invoke. Only `profile_id` is interpreted; family-specific shapes are parsed by the
/// runner once dispatch resolves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlaEnvelope {
    /// Canonical `x402/<family>/<profile>/<version>` id. REQUIRED in every SLA
    /// document (see design.md C8).
    pub profile_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluation_job_clones_cheaply() {
        let job = EvaluationJob {
            payment_uid: [1u8; 32],
            payment_pubkey: Pubkey::new_unique(),
            sla_hash: [2u8; 32],
            delivery_hash: [3u8; 32],
            amount: 1_000_000,
            mint: Pubkey::new_unique(),
            oracle_authority: Pubkey::new_unique(),
            expires_at: 1_900_000_000,
            sla_bytes: Some(Bytes::from_static(b"hello")),
        };
        let cloned = job.clone();
        assert_eq!(job.payment_uid, cloned.payment_uid);
        assert_eq!(job.amount, cloned.amount);
    }

    #[test]
    fn evaluation_result_serde_round_trip() {
        let r = EvaluationResult {
            approved: false,
            resolution_reason: 258,
            checks: vec![CheckResult {
                name: "amount".into(),
                passed: false,
                detail: "delta=1, want >=10".into(),
            }],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: EvaluationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn sla_envelope_parses_profile_id_only() {
        let bytes = br#"{"profile_id":"x402/oracle/api-quality/v1","version":1,"foo":42}"#;
        let env: SlaEnvelope = serde_json::from_slice(bytes).unwrap();
        assert_eq!(env.profile_id, "x402/oracle/api-quality/v1");
    }

    #[test]
    fn sla_envelope_rejects_missing_profile_id() {
        let bytes = br#"{"version":1}"#;
        let res: Result<SlaEnvelope, _> = serde_json::from_slice(bytes);
        assert!(res.is_err());
    }

    #[test]
    fn runtime_health_default_is_disconnected() {
        let h = RuntimeHealth::default();
        assert!(!h.websocket_connected);
        assert_eq!(h.queue_depth, 0);
        assert_eq!(h.deliveries_observed, 0);
        assert_eq!(h.last_seen_slot, 0);
    }
}
