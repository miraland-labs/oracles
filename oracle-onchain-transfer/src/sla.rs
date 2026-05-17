//! `TransferSla` for the onchain-transfer family.

use serde::{Deserialize, Serialize};

use crate::PROFILE_ID;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferSla {
    pub version: u32,
    pub profile_id: String,
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
