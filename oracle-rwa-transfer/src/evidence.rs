//! `TransferEvidence` for the rwa-transfer family.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferEvidence {
    pub version: u32,
    pub profile_id: String,
    /// Base58 Solana transaction signature for the transfer the seller is asserting
    /// fulfilled the SLA.
    pub tx_signature: String,
    pub asserted_transfers: Vec<AssertedTransfer>,
    pub submitted_at: i64,
    /// REQUIRED (Wave B §1.2). Hex-encoded 32-byte `payment_uid` the seller is
    /// claiming this transfer was for. The evaluator refuses evidence whose
    /// `payment_uid` does not match `job.payment_uid`.
    pub payment_uid: String,
    /// OPTIONAL (Wave B §1.4). Echo of the SLA's `buyer_nonce` when set.
    #[serde(default)]
    pub buyer_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertedTransfer {
    pub mint: String,
    pub recipient_owner: String,
    /// Raw integer (decimal string) the seller claims the recipient received. The
    /// oracle re-derives the actual delta from `getTransaction`'s pre/post token
    /// balances and ignores any disagreement.
    pub claimed_delta: String,
}
