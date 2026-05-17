//! Oracle error taxonomy. Implementation lives in Task 2.1.

use thiserror::Error;

/// Top-level error type covering every off-chain oracle failure mode.
///
/// The taxonomy mirrors `design.md` §Error Taxonomy. Variants below `RpcVerification`
/// were added for the multi-family case; the rest were lifted from the original
/// single-binary oracle taxonomy.
#[derive(Debug, Error)]
pub enum OracleError {
    #[error("Chain error: {0}")]
    Chain(String),

    #[error("Evidence not found for hash: {0}")]
    EvidenceNotFound(String),

    #[error("SLA document parse error: {0}")]
    SlaParse(String),

    #[error("Delivery evidence parse error: {0}")]
    DeliveryParse(String),

    #[error("Evaluation failed: {0}")]
    Evaluation(String),

    #[error("Settlement failed: {0}")]
    Settlement(String),

    #[error("Database failed: {0}")]
    Database(String),

    #[error("Unknown profile id: {0}")]
    UnknownProfile(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Registry error: {0}")]
    Registry(String),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("RPC verification failed: {0}")]
    RpcVerification(String),
}

impl From<solana_client::client_error::ClientError> for OracleError {
    fn from(e: solana_client::client_error::ClientError) -> Self {
        OracleError::Chain(e.to_string())
    }
}

impl From<reqwest::Error> for OracleError {
    fn from(e: reqwest::Error) -> Self {
        // Most reqwest errors during evidence fetch surface as "evidence unreachable";
        // the streaming fetcher in `fetcher.rs` produces a more precise message
        // distinguishing hash mismatch from transport errors.
        OracleError::EvidenceNotFound(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_strings_are_stable() {
        let cases: Vec<(OracleError, &str)> = vec![
            (
                OracleError::Chain("rpc down".into()),
                "Chain error: rpc down",
            ),
            (
                OracleError::EvidenceNotFound("abc".into()),
                "Evidence not found for hash: abc",
            ),
            (
                OracleError::SlaParse("bad json".into()),
                "SLA document parse error: bad json",
            ),
            (
                OracleError::DeliveryParse("oops".into()),
                "Delivery evidence parse error: oops",
            ),
            (
                OracleError::Evaluation("nope".into()),
                "Evaluation failed: nope",
            ),
            (
                OracleError::Settlement("blockhash".into()),
                "Settlement failed: blockhash",
            ),
            (
                OracleError::Database("pool full".into()),
                "Database failed: pool full",
            ),
            (
                OracleError::UnknownProfile("x402/wrong/v1".into()),
                "Unknown profile id: x402/wrong/v1",
            ),
            (
                OracleError::Storage("bucket".into()),
                "Storage error: bucket",
            ),
            (OracleError::Registry("dup".into()), "Registry error: dup"),
            (
                OracleError::Auth("bad token".into()),
                "Authentication failed: bad token",
            ),
            (
                OracleError::RpcVerification("getTx none".into()),
                "RPC verification failed: getTx none",
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }
}
