//! `TransferEvidence` for the onchain-transfer family.

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
