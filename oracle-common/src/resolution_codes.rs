//! Reserved custom-resolution-reason ranges per family.
//!
//! All families share the standard reason codes (0..=255) defined in
//! `sla_escrow_api::resolution::ResolutionReason`. Custom codes (≥256) are
//! partitioned per family per design.md §Resolution Reasons Per Family:
//!
//! | Range | Family |
//! |---|---|
//! | 256–319 | `x402/onchain-transfer/*` |
//! | 320–383 | `x402/file-delivery/*` |
//! | 384–447 | reserved for future `x402/compute-result/*` |
//! | 448–511 | reserved for ecosystem-wide additions |
//! | 512+ | per-deployment customization |

/// Custom resolution-reason codes for `x402/oracles/onchain-transfer/v1`.
pub mod onchain_transfer {
    pub const TRANSFER_TX_NOT_FOUND: u16 = 256;
    pub const TRANSFER_TX_FAILED: u16 = 257;
    pub const TRANSFER_AMOUNT_INSUFFICIENT: u16 = 258;
    pub const TRANSFER_MINT_MISMATCH: u16 = 259;
    pub const TRANSFER_DEADLINE_EXCEEDED: u16 = 260;
    pub const TRANSFER_CLUSTER_MISMATCH: u16 = 261;
    pub const TRANSFER_RECIPIENT_NOT_RESOLVABLE: u16 = 262;
    pub const TRANSFER_DIRECTION_MISMATCH: u16 = 263;

    pub const RANGE: std::ops::RangeInclusive<u16> = 256..=319;
}

/// Custom resolution-reason codes for `x402/oracles/file-delivery/attestation/v1`.
pub mod file_delivery {
    pub const BLOB_SIZE_OUT_OF_RANGE: u16 = 320;
    pub const BLOB_MIME_MISMATCH: u16 = 321;
    pub const BLOB_ATTESTOR_SIGNATURE_INVALID: u16 = 322;
    pub const BLOB_UPLOAD_INCOMPLETE: u16 = 323;

    pub const RANGE: std::ops::RangeInclusive<u16> = 320..=383;
}

/// Reserved range for the future `x402/compute-result/*` family.
pub const COMPUTE_RESULT_RANGE: std::ops::RangeInclusive<u16> = 384..=447;

/// Reserved range for ecosystem-wide additions.
pub const ECOSYSTEM_RESERVED_RANGE: std::ops::RangeInclusive<u16> = 448..=511;

/// Per-deployment customization range.
pub const DEPLOYMENT_LOCAL_RANGE: std::ops::RangeInclusive<u16> = 512..=u16::MAX;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_do_not_overlap_and_cover_intended_space() {
        assert!(onchain_transfer::RANGE.start() == &256);
        assert!(onchain_transfer::RANGE.end() == &319);
        assert!(file_delivery::RANGE.start() == &320);
        assert!(file_delivery::RANGE.end() == &383);
        assert!(COMPUTE_RESULT_RANGE.start() == &384);
        assert!(COMPUTE_RESULT_RANGE.end() == &447);
        assert!(ECOSYSTEM_RESERVED_RANGE.start() == &448);
        assert!(ECOSYSTEM_RESERVED_RANGE.end() == &511);
        assert!(DEPLOYMENT_LOCAL_RANGE.start() == &512);
        assert!(DEPLOYMENT_LOCAL_RANGE.end() == &u16::MAX);

        // Sanity-check named constants are inside their owning range.
        for code in [
            onchain_transfer::TRANSFER_TX_NOT_FOUND,
            onchain_transfer::TRANSFER_TX_FAILED,
            onchain_transfer::TRANSFER_AMOUNT_INSUFFICIENT,
            onchain_transfer::TRANSFER_MINT_MISMATCH,
            onchain_transfer::TRANSFER_DEADLINE_EXCEEDED,
            onchain_transfer::TRANSFER_CLUSTER_MISMATCH,
            onchain_transfer::TRANSFER_RECIPIENT_NOT_RESOLVABLE,
            onchain_transfer::TRANSFER_DIRECTION_MISMATCH,
        ] {
            assert!(onchain_transfer::RANGE.contains(&code));
        }

        for code in [
            file_delivery::BLOB_SIZE_OUT_OF_RANGE,
            file_delivery::BLOB_MIME_MISMATCH,
            file_delivery::BLOB_ATTESTOR_SIGNATURE_INVALID,
            file_delivery::BLOB_UPLOAD_INCOMPLETE,
        ] {
            assert!(file_delivery::RANGE.contains(&code));
        }
    }
}
