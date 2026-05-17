//! `SlaDocument` for the api-quality family.
//!
//! See `spec/api-quality-v1/NORMATIVE.md` for the canonical contract. This is the
//! Rust shape the oracle deserializes registry-fetched bytes into; the JSON Schema
//! at `schema/sla-document.schema.json` is the authoritative wire format.

use serde::{Deserialize, Serialize};

use crate::PROFILE_ID;

/// Off-chain SLA document for `x402/oracle/api-quality/v1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlaDocument {
    pub version: u32,
    /// REQUIRED. Must equal `x402/oracle/api-quality/v1` (see C8 in design.md).
    pub profile_id: String,
    pub endpoint: String,
    pub method: String,
    #[serde(default)]
    pub response_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub required_fields: Vec<String>,
    #[serde(default = "default_max_latency")]
    pub max_latency_ms: u64,
    #[serde(default = "default_min_status")]
    pub min_status_code: u16,
    #[serde(default = "default_max_status")]
    pub max_status_code: u16,
    #[serde(default)]
    pub min_body_length: Option<usize>,
}

fn default_max_latency() -> u64 {
    5000
}
fn default_min_status() -> u16 {
    200
}
fn default_max_status() -> u16 {
    299
}

impl SlaDocument {
    /// True when the SLA's `profile_id` matches this family's canonical id.
    /// Returning false is a hard refusal upstream (P-DISP-1, P-AQ-1).
    pub fn profile_id_matches(&self) -> bool {
        self.profile_id == PROFILE_ID
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_to_optional_fields() {
        let s: SlaDocument = serde_json::from_str(
            r#"{"version":1,"profile_id":"x402/oracle/api-quality/v1","endpoint":"https://x","method":"GET"}"#,
        )
        .unwrap();
        assert_eq!(s.version, 1);
        assert_eq!(s.max_latency_ms, 5000);
        assert_eq!(s.min_status_code, 200);
        assert_eq!(s.max_status_code, 299);
        assert!(s.required_fields.is_empty());
        assert_eq!(s.min_body_length, None);
        assert!(s.profile_id_matches());
    }

    #[test]
    fn profile_id_mismatch_detected() {
        let s: SlaDocument = serde_json::from_str(
            r#"{"version":1,"profile_id":"x402/something/v1","endpoint":"https://x","method":"GET"}"#,
        )
        .unwrap();
        assert!(!s.profile_id_matches());
    }
}
