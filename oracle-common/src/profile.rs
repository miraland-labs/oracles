//! Profile registry.
//!
//! At boot, each binary builds one [`RegisteredProfile`] (v1 supports a single
//! profile per binary) and hands it to the pipeline. The dispatcher resolves an
//! incoming SLA's `profile_id` against the registry; an exact-match miss surfaces
//! as `OracleError::UnknownProfile` and the job is refused at dispatch.
//!
//! There are no aliases — single canonical id per profile (design.md C7, P-DISP-1).

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;

use crate::{
    error::OracleError,
    evaluator::{EvaluationContext, OracleEvaluator},
    settler::compute_resolution_hash_typed,
    types::EvaluationOutcome,
};

/// Type-erased pipeline step bound to a specific evaluator + fetcher pair.
#[async_trait]
pub trait ProfileRunner: Send + Sync {
    /// Returns the canonical profile id this runner serves.
    fn profile_id(&self) -> &'static str;

    /// Run the evaluator against pre-fetched, hash-verified inputs and produce a
    /// settle-ready outcome (verdict + 32-byte resolution hash, no signature yet).
    async fn run(&self, ctx: &EvaluationContext<'_>) -> Result<EvaluationOutcome, OracleError>;
}

/// One registered profile. The runner is the dispatch target.
pub struct RegisteredProfile {
    pub profile_id: &'static str,
    pub run: Arc<dyn ProfileRunner>,
}

/// In-memory registry of known profiles. Built once at startup.
#[derive(Default)]
pub struct ProfileRegistry {
    by_id: HashMap<&'static str, Arc<dyn ProfileRunner>>,
}

impl ProfileRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a profile.
    pub fn register(&mut self, profile: RegisteredProfile) {
        self.by_id.insert(profile.profile_id, profile.run);
    }

    /// Resolve a profile by exact id; returns `None` for unknown ids.
    pub fn resolve(&self, profile_id: &str) -> Option<Arc<dyn ProfileRunner>> {
        self.by_id.get(profile_id).cloned()
    }

    pub fn known_ids(&self) -> Vec<&'static str> {
        self.by_id.keys().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// Adapter binding a typed [`OracleEvaluator`] to a single SLA fetcher and a single
/// evidence fetcher. The pipeline calls [`ProfileRunner::run`], which fetches both
/// artefacts (verifying SHA-256 along the way), evaluates, and computes the
/// resolution hash.
pub struct ProfileBinding<E, FSla, FEv>
where
    E: OracleEvaluator + 'static,
    FSla: crate::fetcher::EvidenceFetcher<Output = E::Sla> + 'static,
    FEv: crate::fetcher::EvidenceFetcher<Output = E::Evidence> + 'static,
{
    pub evaluator: Arc<E>,
    pub sla_fetcher: Arc<FSla>,
    pub evidence_fetcher: Arc<FEv>,
}

#[async_trait]
impl<E, FSla, FEv> ProfileRunner for ProfileBinding<E, FSla, FEv>
where
    E: OracleEvaluator + 'static,
    FSla: crate::fetcher::EvidenceFetcher<Output = E::Sla> + 'static,
    FEv: crate::fetcher::EvidenceFetcher<Output = E::Evidence> + 'static,
{
    fn profile_id(&self) -> &'static str {
        self.evaluator.profile_id()
    }

    async fn run(&self, ctx: &EvaluationContext<'_>) -> Result<EvaluationOutcome, OracleError> {
        use crate::fetcher::ArtifactKind;

        let sla = self
            .sla_fetcher
            .fetch(&ctx.job.sla_hash, ArtifactKind::Sla)
            .await?;
        let evidence = self
            .evidence_fetcher
            .fetch(&ctx.job.delivery_hash, ArtifactKind::Delivery)
            .await?;
        let result = self.evaluator.evaluate(ctx, &sla, &evidence).await?;
        let resolution_hash =
            compute_resolution_hash_typed(ctx.job, self.evaluator.profile_id(), &result, &sla)?;
        Ok(EvaluationOutcome {
            result,
            resolution_hash,
            signature: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test-only runner that records its profile id and returns a stub outcome.
    struct StubRunner {
        id: &'static str,
    }

    #[async_trait]
    impl ProfileRunner for StubRunner {
        fn profile_id(&self) -> &'static str {
            self.id
        }
        async fn run(
            &self,
            _ctx: &EvaluationContext<'_>,
        ) -> Result<EvaluationOutcome, OracleError> {
            unimplemented!("not exercised in registry tests")
        }
    }

    #[test]
    fn register_and_resolve_exact_id() {
        let mut reg = ProfileRegistry::new();
        reg.register(RegisteredProfile {
            profile_id: "x402/oracle/api-quality/v1",
            run: Arc::new(StubRunner {
                id: "x402/oracle/api-quality/v1",
            }),
        });
        assert_eq!(reg.len(), 1);
        let r = reg.resolve("x402/oracle/api-quality/v1").unwrap();
        assert_eq!(r.profile_id(), "x402/oracle/api-quality/v1");
    }

    #[test]
    fn unknown_id_returns_none() {
        let mut reg = ProfileRegistry::new();
        reg.register(RegisteredProfile {
            profile_id: "x402/oracle/api-quality/v1",
            run: Arc::new(StubRunner {
                id: "x402/oracle/api-quality/v1",
            }),
        });
        assert!(reg
            .resolve("x402/oracle/file-delivery/attestation/v1")
            .is_none());
    }

    #[test]
    fn no_alias_no_prefix_match() {
        // P-DISP-1: aliases / prefix matches are explicitly NOT supported. A SLA
        // declaring a similar-but-not-equal id is refused.
        let mut reg = ProfileRegistry::new();
        reg.register(RegisteredProfile {
            profile_id: "x402/oracle/api-quality/v1",
            run: Arc::new(StubRunner {
                id: "x402/oracle/api-quality/v1",
            }),
        });
        for nope in [
            "x402/oracle/api-quality/v2",
            "x402/oracle/api-quality",
            "x402/api-quality/v1",
            "api-quality/v1",
            "X402/oracle/api-quality/v1", // case-sensitive
        ] {
            assert!(reg.resolve(nope).is_none(), "must not resolve: {nope}");
        }
    }

    #[test]
    fn known_ids_lists_registered() {
        let mut reg = ProfileRegistry::new();
        assert!(reg.is_empty());
        reg.register(RegisteredProfile {
            profile_id: "x402/foo/v1",
            run: Arc::new(StubRunner { id: "x402/foo/v1" }),
        });
        let ids = reg.known_ids();
        assert_eq!(ids, vec!["x402/foo/v1"]);
    }
}
