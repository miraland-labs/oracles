//! `OracleEvaluator` trait.
//!
//! Each oracle family implements this trait once. The chain monitor, settler, ledger,
//! HTTP server, and registry are reused unchanged. See design.md §Pluggable Trait
//! Surface for the rationale.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;

use crate::{
    error::OracleError,
    types::{EvaluationJob, EvaluationResult, EvidenceKey},
};

/// Read-only ledger probe handed to evaluators that need cross-payment
/// replay protection (Wave A §1.3 / §2.2.1).
///
/// The trait is intentionally minimal: a single async query that asks "has
/// this evidence key already been settled for a *different* payment_uid?".
/// Evaluators get the answer as a `Result<bool>` and decide what to do with
/// it (typically reject with a family-specific reason code). Keeping it a
/// trait lets unit tests pass `None` and integration tests pass an
/// `OracleDb`-backed implementation.
#[async_trait]
pub trait LedgerProbe: Send + Sync {
    async fn evidence_key_settled_for_other_payment(
        &self,
        current_uid: &[u8; 32],
        key_kind: &str,
        key_value: &str,
    ) -> Result<bool, OracleError>;
}

/// Cross-cutting concerns handed to evaluators (RPC, HTTP, the job's metadata,
/// the deployment's strict-profile flag, and an optional ledger probe for
/// cross-payment replay checks).
pub struct EvaluationContext<'a> {
    pub rpc: &'a Arc<RpcClient>,
    pub http: &'a reqwest::Client,
    pub job: &'a EvaluationJob,
    pub strict: bool,
    /// Wave A §1.3 / §2.2.1 — present when the binary is configured with a
    /// Postgres ledger; absent for in-memory and unit-test contexts.
    /// Evaluators that don't need cross-payment replay protection can ignore
    /// this field.
    pub ledger: Option<&'a Arc<dyn LedgerProbe>>,
}

/// Evaluation contract for one oracle family.
///
/// In v1 each binary registers exactly one evaluator (see Requirement 25.1).
#[async_trait]
pub trait OracleEvaluator: Send + Sync {
    /// SLA shape for this profile. Must be (de)serializable so the registry-fetched
    /// bytes can be decoded into it and so it can be fingerprinted into the
    /// resolution hash via the canonical recipe.
    type Sla: DeserializeOwned + Serialize + Send + Sync;

    /// Evidence shape. Same constraints as Sla.
    type Evidence: DeserializeOwned + Serialize + Send + Sync;

    /// Stable, canonical profile identifier (e.g. `"x402/oracles/api-quality/v1"`).
    /// Single canonical id per profile; aliases are not supported.
    fn profile_id(&self) -> &'static str;

    /// Run the deterministic check battery. Must return reasons drawn from
    /// `sla_escrow_api::resolution::ResolutionReason` (standard ≤255 or Custom
    /// ≥256, partitioned per family in [`crate::resolution_codes`]).
    async fn evaluate(
        &self,
        ctx: &EvaluationContext<'_>,
        sla: &Self::Sla,
        evidence: &Self::Evidence,
    ) -> Result<EvaluationResult, OracleError>;

    /// Wave A §1.3 / §2.2.1 — declare the evidence keys this evaluation
    /// consumed (e.g. an onchain-transfer's `tx_signature`, or a file-delivery's
    /// `delivery_hash`). The worker records each into `oracle_evidence_keys`
    /// after a successful approve so future evaluations of *other* payments can
    /// detect and refuse cross-payment reuse.
    ///
    /// Default impl returns an empty list (no cross-payment dedupe). Evaluators
    /// that need this protection MUST override.
    fn evidence_keys(&self, _sla: &Self::Sla, _evidence: &Self::Evidence) -> Vec<EvidenceKey> {
        Vec::new()
    }
}
