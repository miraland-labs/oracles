//! `FileDeliveryEvidence` — outcome of the streaming-fetch path.
//!
//! Unlike api-quality / onchain-transfer, the on-chain `delivery_hash` for this
//! family commits directly to the blob bytes (no JSON envelope). The evaluator
//! receives this struct from the streaming fetcher; the bytes themselves are NOT
//! retained in memory.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDeliveryEvidence {
    pub size_bytes: u64,
    pub sniffed_mime: Option<String>,
    /// Hex digest computed by the streaming fetcher; equals on-chain `delivery_hash`.
    pub blob_sha256_hex: String,
}
