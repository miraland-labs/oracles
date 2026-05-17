//! `Evaluator` impl for `x402/oracle/api-quality/v1`.
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
    pub fn evaluate_sync(
        &self,
        sla: &SlaDocument,
        evidence: &DeliveryEvidence,
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
        _ctx: &EvaluationContext<'_>,
        sla: &Self::Sla,
        evidence: &Self::Evidence,
    ) -> Result<EvaluationResult, OracleError> {
        Ok(self.evaluate_sync(sla, evidence))
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
        // Status fails before latency in the documented order.
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
