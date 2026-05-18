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
    /// OPTIONAL. Base58 pubkey of the source wallet. When set, the oracle
    /// verifies that the same `(mint, sender_owner)` pair appears in
    /// `pre_token_balances` AND that the signed delta for the sender row is
    /// negative (sender lost tokens) with magnitude at least `min_amount`.
    /// When unset, the sender check is skipped entirely (back-compat for
    /// SLAs authored before this field existed).
    ///
    /// This is defense-in-depth on top of cross-payment replay protection:
    /// it pins which wallet the tokens came from, not just where they
    /// landed. A buyer who knows the seller's treasury wallet
    /// (e.g. AetherVane's Zodiac mint custody account) can pin it here so
    /// a third party who somehow constructed valid recipient-side evidence
    /// cannot bind their own historical transfer to this payment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_owner: Option<String>,
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
