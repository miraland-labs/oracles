//! `TransferSla` for the onchain-transfer family.

use serde::{Deserialize, Serialize};

use crate::PROFILE_ID;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferSla {
    pub version: u32,
    pub profile_id: String,
    /// REQUIRED (Wave B §1.2). Hex-encoded 32-byte `payment_uid` from the
    /// on-chain `Payment` this SLA is bound to. The SLA is hashed *with this
    /// field included* into `Payment.sla_hash`, so the document is
    /// cryptographically tied to one payment. The evaluator refuses evidence
    /// whose `payment_uid` does not match the on-chain payment that the job
    /// was built for.
    pub payment_uid: String,
    /// OPTIONAL (Wave B §1.4). Hex-encoded fresh random 32-byte buyer nonce.
    /// When set, the seller must echo it back in `TransferEvidence`. Defeats
    /// cross-SLA reuse where two buyers with identical SLA templates could
    /// otherwise have a seller replay one's evidence against the other's
    /// payment.
    #[serde(default)]
    pub buyer_nonce: Option<String>,
    pub cluster: TransferCluster,
    pub expected_transfers: Vec<ExpectedTransfer>,
    #[serde(default)]
    pub swap_router: Option<String>,
    #[serde(default)]
    pub slippage_bps: Option<u16>,
    #[serde(default)]
    pub deadline_unix: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TransferCluster {
    MainnetBeta,
    Devnet,
    Testnet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedTransfer {
    pub mint: String,
    pub recipient_owner: String,
    pub min_amount: String,
    pub direction: TransferDirection,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransferDirection {
    In,
    Out,
}

impl TransferSla {
    pub fn profile_id_matches(&self) -> bool {
        self.profile_id == PROFILE_ID
    }
}
