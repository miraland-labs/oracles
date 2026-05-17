//! `DeliveryEvidence` for the api-quality family.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryEvidence {
    pub status_code: u16,
    pub latency_ms: u64,
    pub response_body: serde_json::Value,
    #[serde(default)]
    pub response_headers: Option<serde_json::Map<String, serde_json::Value>>,
    pub timestamp: i64,
    /// REQUIRED (Wave B §1.2). Hex-encoded 32-byte `payment_uid` the seller is
    /// claiming this measurement was taken for. The evaluator refuses evidence
    /// whose `payment_uid` does not match `job.payment_uid` — defeats binding
    /// reuse across payments.
    pub payment_uid: String,
    /// OPTIONAL (Wave B §1.4). Echo of the SLA's `buyer_nonce`. Required when
    /// the SLA carries one; missing or different → reject.
    #[serde(default)]
    pub buyer_nonce: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_evidence_with_defaults() {
        let e: DeliveryEvidence = serde_json::from_str(
            r#"{"status_code":200,"latency_ms":42,"response_body":{"ok":true},"timestamp":1700000000,"payment_uid":"0000000000000000000000000000000000000000000000000000000000000000"}"#,
        )
        .unwrap();
        assert_eq!(e.status_code, 200);
        assert!(e.response_headers.is_none());
        assert!(e.buyer_nonce.is_none());
    }
}
