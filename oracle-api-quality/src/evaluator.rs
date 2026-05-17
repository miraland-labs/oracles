//! `Evaluator` impl for `x402/oracles/api-quality/v1`.
//!
//! Runs the deterministic check battery in fixed order:
//!
//! 1. version check (must be 1)
//! 2. profile_id match
//! 3. status_code in `[min_status_code, max_status_code]`
//! 4. latency_ms ≤ max_latency_ms
//! 5. each `required_fields` entry is a key in `response_body` (when it's an object)
//! 6. `response_schema` validates `response_body` (if present)
//! 7. `min_body_length` ≤ len(serialize(`response_body`)) (if present)
//!
//! On reject, the `resolution_reason` is the standard code corresponding to the
//! first failing check (P-VER-2, P-AQ-2). On approve, `resolution_reason == 0`
//! (P-VER-3).

use async_trait::async_trait;
use oracle_common::{
    error::OracleError,
    evaluator::{EvaluationContext, OracleEvaluator},
    types::{CheckResult, EvaluationResult},
};
use sla_escrow_api::resolution::ResolutionReason;

use crate::{evidence::DeliveryEvidence, sla::SlaDocument, PROFILE_ID};

/// Stateless evaluator for the api-quality family.
#[derive(Clone, Copy, Default)]
pub struct ApiQualityEvaluator {
    pub strict: bool,
}

impl ApiQualityEvaluator {
    pub fn new(strict: bool) -> Self {
        Self { strict }
    }

    /// Pure helper for testing. Equivalent to calling `evaluate` synchronously.
    ///
    /// `payment_created_at`, when supplied (Wave A §1.1), enforces a freshness
    /// lower bound: `evidence.timestamp` must be ≥ this value. Earlier evidence
    /// indicates the seller is replaying a measurement taken before the buyer
    /// funded escrow and is rejected with `GeneralRejection` carrying a
    /// `freshness` check name. When `None`, the check is skipped (legacy /
    /// unknown — preserves backward compatibility for fixtures and the chain
    /// monitor that have not yet populated `Payment.created_at`).
    pub fn evaluate_sync(
        &self,
        sla: &SlaDocument,
        evidence: &DeliveryEvidence,
    ) -> EvaluationResult {
        self.evaluate_with_freshness(sla, evidence, None)
    }

    pub fn evaluate_with_freshness(
        &self,
        sla: &SlaDocument,
        evidence: &DeliveryEvidence,
        payment_created_at: Option<i64>,
    ) -> EvaluationResult {
        self.evaluate_with_freshness_and_uid(sla, evidence, payment_created_at, None)
    }

    /// Production entry point: also enforces the on-chain `payment_uid` and the
    /// buyer-nonce echo when the SLA carries one (Wave B §1.2 / §1.4).
    pub fn evaluate_with_freshness_and_uid(
        &self,
        sla: &SlaDocument,
        evidence: &DeliveryEvidence,
        payment_created_at: Option<i64>,
        on_chain_payment_uid: Option<&[u8; 32]>,
    ) -> EvaluationResult {
        let mut checks = Vec::new();

        // Check: version
        let version_ok = sla.version == 1;
        checks.push(CheckResult {
            name: "version".into(),
            passed: version_ok,
            detail: if version_ok {
                "1".into()
            } else {
                format!("expected 1, got {}", sla.version)
            },
        });

        // Check: profile_id (always required per design.md C8)
        let profile_ok = sla.profile_id_matches();
        checks.push(CheckResult {
            name: "profile_id".into(),
            passed: profile_ok,
            detail: if profile_ok {
                PROFILE_ID.into()
            } else {
                format!("expected '{PROFILE_ID}', got '{}'", sla.profile_id)
            },
        });

        // Check: payment_uid binding (Wave B §1.2). The on-chain payment_uid
        // (when known via the chain monitor) MUST match both the SLA's and the
        // evidence's claimed payment_uid. SLA → on-chain binding catches a
        // seller publishing a single SLA template across many payments;
        // evidence → SLA binding catches a seller submitting evidence taken for
        // payment A against payment B.
        if let Some(uid_bytes) = on_chain_payment_uid {
            let want = hex::encode(uid_bytes);
            let sla_ok = sla.payment_uid.eq_ignore_ascii_case(&want);
            let evidence_ok = evidence.payment_uid.eq_ignore_ascii_case(&want);
            checks.push(CheckResult {
                name: "payment_uid".into(),
                passed: sla_ok && evidence_ok,
                detail: if sla_ok && evidence_ok {
                    format!("matches on-chain payment_uid {want}")
                } else if !sla_ok {
                    format!(
                        "sla.payment_uid {} differs from on-chain payment_uid {}",
                        sla.payment_uid, want
                    )
                } else {
                    format!(
                        "evidence.payment_uid {} differs from on-chain payment_uid {}",
                        evidence.payment_uid, want
                    )
                },
            });
        }

        // Check: buyer-nonce echo (Wave B §1.4). When the SLA carries a nonce,
        // the seller MUST echo it verbatim in the evidence; missing or
        // different rejects.
        if let Some(want_nonce) = sla.buyer_nonce.as_deref() {
            let got = evidence.buyer_nonce.as_deref().unwrap_or("");
            let ok = !got.is_empty() && got.eq_ignore_ascii_case(want_nonce);
            checks.push(CheckResult {
                name: "buyer_nonce".into(),
                passed: ok,
                detail: if ok {
                    "matches".into()
                } else if got.is_empty() {
                    "SLA carries buyer_nonce but evidence is missing one".into()
                } else {
                    format!("evidence.buyer_nonce {got} differs from sla.buyer_nonce {want_nonce}")
                },
            });
        }

        // Check: freshness lower bound (Wave A §1.1). Only enforced when the
        // chain monitor populated `Payment.created_at`.
        if let Some(created_at) = payment_created_at {
            let fresh_ok = evidence.timestamp >= created_at;
            checks.push(CheckResult {
                name: "freshness".into(),
                passed: fresh_ok,
                detail: if fresh_ok {
                    format!(
                        "timestamp {} ≥ created_at {}",
                        evidence.timestamp, created_at
                    )
                } else {
                    format!(
                        "timestamp {} < created_at {}; evidence predates payment funding",
                        evidence.timestamp, created_at
                    )
                },
            });
        }

        // Check: status code
        let status_ok = evidence.status_code >= sla.min_status_code
            && evidence.status_code <= sla.max_status_code;
        checks.push(CheckResult {
            name: "status_code".into(),
            passed: status_ok,
            detail: format!(
                "Got {} (expected {}-{})",
                evidence.status_code, sla.min_status_code, sla.max_status_code
            ),
        });

        // Check: latency
        let latency_ok = evidence.latency_ms <= sla.max_latency_ms;
        checks.push(CheckResult {
            name: "latency".into(),
            passed: latency_ok,
            detail: format!("{}ms (max {}ms)", evidence.latency_ms, sla.max_latency_ms),
        });

        // Check: required fields
        if !sla.required_fields.is_empty() {
            let body_obj = evidence.response_body.as_object();
            for field in &sla.required_fields {
                let present = body_obj.map(|obj| obj.contains_key(field)).unwrap_or(false);
                checks.push(CheckResult {
                    name: format!("required_field:{field}"),
                    passed: present,
                    detail: if present {
                        "present".into()
                    } else {
                        "missing".into()
                    },
                });
            }
        }

        // Check: JSON schema (if specified)
        if let Some(schema) = &sla.response_schema {
            match jsonschema::validator_for(schema) {
                Ok(validator) => {
                    let valid = validator.is_valid(&evidence.response_body);
                    checks.push(CheckResult {
                        name: "json_schema".into(),
                        passed: valid,
                        detail: if valid {
                            "valid".into()
                        } else {
                            let errs: Vec<String> = validator
                                .iter_errors(&evidence.response_body)
                                .take(3)
                                .map(|e| e.to_string())
                                .collect();
                            format!("invalid: {}", errs.join("; "))
                        },
                    });
                }
                Err(e) => {
                    checks.push(CheckResult {
                        name: "json_schema".into(),
                        passed: false,
                        detail: format!("schema compile error: {e}"),
                    });
                }
            }
        }

        // Check: body length floor
        if let Some(min_len) = sla.min_body_length {
            let body_str = serde_json::to_string(&evidence.response_body).unwrap_or_default();
            let len_ok = body_str.len() >= min_len;
            checks.push(CheckResult {
                name: "body_length".into(),
                passed: len_ok,
                detail: format!("{} bytes (min {})", body_str.len(), min_len),
            });
        }

        let approved = checks.iter().all(|c| c.passed);
        let resolution_reason: u16 = if approved {
            ResolutionReason::None.into()
        } else {
            checks
                .iter()
                .find(|c| !c.passed)
                .map(|c| match c.name.as_str() {
                    "profile_id" | "version" => ResolutionReason::GeneralRejection,
                    "payment_uid" => ResolutionReason::GeneralRejection,
                    "buyer_nonce" => ResolutionReason::GeneralRejection,
                    "freshness" => ResolutionReason::GeneralRejection,
                    "status_code" => ResolutionReason::StatusCodeOutOfRange,
                    "latency" => ResolutionReason::LatencyExceeded,
                    "json_schema" => ResolutionReason::SchemaValidationFailed,
                    "body_length" => ResolutionReason::BodyTooShort,
                    name if name.starts_with("required_field:") => {
                        ResolutionReason::RequiredFieldsMissing
                    }
                    _ => ResolutionReason::GeneralRejection,
                })
                .unwrap_or(ResolutionReason::GeneralRejection)
                .into()
        };

        EvaluationResult {
            approved,
            resolution_reason,
            checks,
        }
    }
}

#[async_trait]
impl OracleEvaluator for ApiQualityEvaluator {
    type Sla = SlaDocument;
    type Evidence = DeliveryEvidence;

    fn profile_id(&self) -> &'static str {
        PROFILE_ID
    }

    async fn evaluate(
        &self,
        ctx: &EvaluationContext<'_>,
        sla: &Self::Sla,
        evidence: &Self::Evidence,
    ) -> Result<EvaluationResult, OracleError> {
        let payment_created_at = if ctx.job.created_at > 0 {
            Some(ctx.job.created_at)
        } else {
            None
        };
        let on_chain_uid = Some(&ctx.job.payment_uid);
        Ok(self.evaluate_with_freshness_and_uid(sla, evidence, payment_created_at, on_chain_uid))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn valid_sla() -> SlaDocument {
        SlaDocument {
            version: 1,
            profile_id: PROFILE_ID.into(),
            payment_uid: "00".repeat(32),
            buyer_nonce: None,
            endpoint: "https://api.example.test/data".into(),
            method: "GET".into(),
            response_schema: None,
            required_fields: vec!["result".into()],
            max_latency_ms: 500,
            min_status_code: 200,
            max_status_code: 299,
            min_body_length: None,
        }
    }

    fn valid_evidence() -> DeliveryEvidence {
        DeliveryEvidence {
            status_code: 200,
            latency_ms: 42,
            response_body: serde_json::json!({"result": "ok"}),
            response_headers: None,
            timestamp: 1_700_000_000,
            payment_uid: "00".repeat(32),
            buyer_nonce: None,
        }
    }

    #[test]
    fn approves_when_all_checks_pass() {
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&valid_sla(), &valid_evidence());
        assert!(r.approved);
        assert_eq!(r.resolution_reason, 0);
    }

    #[test]
    fn rejects_status_out_of_range() {
        let mut e = valid_evidence();
        e.status_code = 500;
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&valid_sla(), &e);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            u16::from(ResolutionReason::StatusCodeOutOfRange)
        );
    }

    #[test]
    fn rejects_latency_exceeded() {
        let mut e = valid_evidence();
        e.latency_ms = 100_000;
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&valid_sla(), &e);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            u16::from(ResolutionReason::LatencyExceeded)
        );
    }

    #[test]
    fn rejects_required_field_missing() {
        let mut e = valid_evidence();
        e.response_body = serde_json::json!({"other": 1});
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&valid_sla(), &e);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            u16::from(ResolutionReason::RequiredFieldsMissing)
        );
    }

    #[test]
    fn rejects_schema_validation_failure() {
        let mut sla = valid_sla();
        sla.response_schema = Some(serde_json::json!({
            "type": "object",
            "required": ["mandatory"]
        }));
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &valid_evidence());
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            u16::from(ResolutionReason::SchemaValidationFailed)
        );
    }

    #[test]
    fn rejects_body_length_floor() {
        let mut sla = valid_sla();
        sla.min_body_length = Some(10_000);
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &valid_evidence());
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            u16::from(ResolutionReason::BodyTooShort)
        );
    }

    #[test]
    fn rejects_profile_id_mismatch_with_general_rejection() {
        let mut sla = valid_sla();
        sla.profile_id = "x402/something-else/v1".into();
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &valid_evidence());
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            u16::from(ResolutionReason::GeneralRejection)
        );
    }

    #[test]
    fn rejects_version_two_with_general_rejection() {
        let mut sla = valid_sla();
        sla.version = 2;
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &valid_evidence());
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            u16::from(ResolutionReason::GeneralRejection)
        );
    }

    #[test]
    fn first_failing_check_determines_reason() {
        let mut e = valid_evidence();
        e.status_code = 500;
        e.latency_ms = 999_999;
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&valid_sla(), &e);
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            u16::from(ResolutionReason::StatusCodeOutOfRange)
        );
    }

    // -------------------------------------------------------------------------
    // Wave A §1.1 — freshness lower bound (created_at)
    // -------------------------------------------------------------------------

    #[test]
    fn rejects_when_evidence_timestamp_predates_payment_created_at() {
        // Evidence captured before the buyer funded escrow → reject.
        let mut e = valid_evidence();
        e.timestamp = 1_700_000_000;
        let r = ApiQualityEvaluator::new(true).evaluate_with_freshness(
            &valid_sla(),
            &e,
            Some(1_700_000_001),
        );
        assert!(!r.approved);
        assert_eq!(
            r.resolution_reason,
            u16::from(ResolutionReason::GeneralRejection)
        );
        // The freshness check is the one that flipped.
        let failure = r.checks.iter().find(|c| !c.passed).unwrap();
        assert_eq!(failure.name, "freshness");
    }

    #[test]
    fn approves_when_evidence_timestamp_equals_payment_created_at() {
        // Equality is allowed (≥ boundary).
        let mut e = valid_evidence();
        e.timestamp = 1_700_000_000;
        let r = ApiQualityEvaluator::new(true).evaluate_with_freshness(
            &valid_sla(),
            &e,
            Some(1_700_000_000),
        );
        assert!(r.approved);
    }

    #[test]
    fn skips_freshness_check_when_created_at_unknown() {
        // Backward compat: no `Payment.created_at` populated → no gate.
        let mut e = valid_evidence();
        e.timestamp = 0; // would fail any non-trivial lower bound
        let r = ApiQualityEvaluator::new(true).evaluate_with_freshness(&valid_sla(), &e, None);
        assert!(r.approved);
    }

    // -------------------------------------------------------------------------
    // Wave B §1.2 / §1.4 — payment_uid binding & buyer-nonce echo
    // -------------------------------------------------------------------------

    #[test]
    fn rejects_when_evidence_payment_uid_differs_from_on_chain() {
        let sla = valid_sla();
        let mut e = valid_evidence();
        e.payment_uid = "ff".repeat(32);
        let r = ApiQualityEvaluator::new(true).evaluate_with_freshness_and_uid(
            &sla,
            &e,
            None,
            Some(&[0u8; 32]),
        );
        assert!(!r.approved);
        let failure = r.checks.iter().find(|c| !c.passed).unwrap();
        assert_eq!(failure.name, "payment_uid");
    }

    #[test]
    fn rejects_when_sla_payment_uid_differs_from_on_chain() {
        let mut sla = valid_sla();
        sla.payment_uid = "11".repeat(32);
        let e = valid_evidence();
        let r = ApiQualityEvaluator::new(true).evaluate_with_freshness_and_uid(
            &sla,
            &e,
            None,
            Some(&[0u8; 32]),
        );
        assert!(!r.approved);
        let failure = r.checks.iter().find(|c| !c.passed).unwrap();
        assert_eq!(failure.name, "payment_uid");
    }

    #[test]
    fn skips_payment_uid_check_when_on_chain_uid_unknown() {
        // Back-compat: unit tests / legacy fixtures pass None and the check is
        // not added.
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&valid_sla(), &valid_evidence());
        assert!(r.approved);
    }

    #[test]
    fn rejects_when_sla_carries_nonce_and_evidence_omits_it() {
        let mut sla = valid_sla();
        sla.buyer_nonce = Some("ab".repeat(32));
        let e = valid_evidence(); // buyer_nonce: None
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &e);
        assert!(!r.approved);
        let failure = r.checks.iter().find(|c| !c.passed).unwrap();
        assert_eq!(failure.name, "buyer_nonce");
    }

    #[test]
    fn rejects_when_buyer_nonce_differs() {
        let mut sla = valid_sla();
        sla.buyer_nonce = Some("ab".repeat(32));
        let mut e = valid_evidence();
        e.buyer_nonce = Some("cd".repeat(32));
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &e);
        assert!(!r.approved);
        let failure = r.checks.iter().find(|c| !c.passed).unwrap();
        assert_eq!(failure.name, "buyer_nonce");
    }

    #[test]
    fn approves_when_buyer_nonce_matches() {
        let mut sla = valid_sla();
        sla.buyer_nonce = Some("ab".repeat(32));
        let mut e = valid_evidence();
        e.buyer_nonce = Some("ab".repeat(32));
        let r = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &e);
        assert!(r.approved);
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 64,
            ..ProptestConfig::default()
        })]

        /// P-AQ-2 / P-VER-1: approved iff every check in the battery passes.
        #[test]
        fn p_aq_2_conjunction_of_checks(
            status in 100u16..600,
            latency in 0u64..20_000,
            min_status in 100u16..400,
            max_status in 200u16..600,
            max_latency in 100u64..10_000,
        ) {
            let min_status = min_status.min(max_status);
            let max_status = max_status.max(min_status);

            let mut sla = valid_sla();
            sla.min_status_code = min_status;
            sla.max_status_code = max_status;
            sla.max_latency_ms = max_latency;
            sla.required_fields = vec![]; // simplify the universe

            let mut e = valid_evidence();
            e.status_code = status;
            e.latency_ms = latency;

            let r = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &e);

            let status_ok = status >= min_status && status <= max_status;
            let latency_ok = latency <= max_latency;
            let expected = status_ok && latency_ok;

            prop_assert_eq!(r.approved, expected);
            // P-VER-3
            if expected {
                prop_assert_eq!(r.resolution_reason, 0);
            }
        }

        /// P-DET-1: deterministic across runs with identical inputs.
        #[test]
        fn p_det_1_evaluator_is_deterministic(
            status in 100u16..600,
            latency in 0u64..20_000,
        ) {
            let sla = valid_sla();
            let mut e = valid_evidence();
            e.status_code = status;
            e.latency_ms = latency;

            let a = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &e);
            let b = ApiQualityEvaluator::new(true).evaluate_sync(&sla, &e);
            prop_assert_eq!(a, b);
        }
    }
}
