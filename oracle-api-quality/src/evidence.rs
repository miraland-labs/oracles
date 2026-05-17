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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_evidence_with_defaults() {
        let e: DeliveryEvidence = serde_json::from_str(
            r#"{"status_code":200,"latency_ms":42,"response_body":{"ok":true},"timestamp":1700000000}"#,
        )
        .unwrap();
        assert_eq!(e.status_code, 200);
        assert!(e.response_headers.is_none());
    }
}
