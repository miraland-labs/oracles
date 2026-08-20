//! `FileDeliverySla` for the file-delivery family.

use serde::{Deserialize, Serialize};

use crate::PROFILE_ID;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDeliverySla {
    pub version: u32,
    pub profile_id: String,
    /// REQUIRED. Forge listing identity exactly as Forge publishes it — the
    /// `{listing_id}` path segment of the oracle verdict door
    /// (`GET /api/v1/oracle/listings/{listing_id}/artifact`, pinned
    /// `http402-forge-api` contract). Identifies the listing being judged.
    /// Distinct from `payment_uid`: never derived from or replaced by it.
    pub listing_id: String,
    /// REQUIRED (Wave B §1.2). Hex-encoded 32-byte `payment_uid` from the
    /// on-chain `Payment` this SLA is bound to. Hashed into `Payment.sla_hash`
    /// so the SLA is cryptographically tied to one payment.
    pub payment_uid: String,
    /// OPTIONAL (Wave B §1.4). Hex-encoded fresh random 32-byte buyer nonce.
    /// When set, the seller must echo it back in the companion evidence shape
    /// (a future-evidence-version requirement; the current attestation-only
    /// path doesn't carry one).
    #[serde(default)]
    pub buyer_nonce: Option<String>,
    pub expected_size_bytes_min: u64,
    pub expected_size_bytes_max: u64,
    #[serde(default)]
    pub expected_mime: Option<String>,
    #[serde(default)]
    pub expected_extension: Option<String>,
    #[serde(default)]
    pub attestor_pubkey: Option<String>,
}

impl FileDeliverySla {
    pub fn profile_id_matches(&self) -> bool {
        self.profile_id == PROFILE_ID
    }
}
