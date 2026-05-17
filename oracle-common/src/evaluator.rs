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
    types::{EvaluationJob, EvaluationResult},
};

/// Cross-cutting concerns handed to evaluators (RPC, HTTP, the job's metadata,
/// the deployment's strict-profile flag).
pub struct EvaluationContext<'a> {
    pub rpc: &'a Arc<RpcClient>,
    pub http: &'a reqwest::Client,
    pub job: &'a EvaluationJob,
    pub strict: bool,
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
}
