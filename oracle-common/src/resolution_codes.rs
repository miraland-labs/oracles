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
    /// Wave A §1.1 — observed `block_time` predates `Payment.created_at`,
    /// indicating the seller is replaying a transfer that happened before the
    /// buyer funded the escrow. Reject with this code.
    pub const TRANSFER_EVIDENCE_PREDATES_PAYMENT: u16 = 264;
    /// Wave A §2.2.1 — same `tx_signature` was already settled for a different
    /// `payment_uid` on this oracle's ledger. Cross-payment replay refusal.
    pub const TRANSFER_TX_SIGNATURE_REUSED: u16 = 265;
    /// Wave B §1.2 — evidence's `payment_uid` does not match the on-chain
    /// `Payment.payment_uid` this evaluation is bound to. Hard refusal.
    pub const TRANSFER_PAYMENT_UID_MISMATCH: u16 = 266;
    /// Wave B §1.4 — SLA carries a `buyer_nonce` and the evidence didn't echo
    /// it back. Refusal.
    pub const TRANSFER_BUYER_NONCE_MISMATCH: u16 = 267;
    /// Wave A §2.2.2 — `block_time` is missing from RPC metadata and the
    /// evaluator is in strict-mandatory-blocktime mode.
    pub const TRANSFER_BLOCK_TIME_MISSING: u16 = 268;
    /// Production-hardening §1 — SLA pinned a `sender_owner` and one of two
    /// failure modes occurred:
    ///
    /// 1. No matching `(mint, sender_owner)` row was found in the
    ///    transaction's `pre_token_balances`.
    /// 2. The matching row was found but the sender's signed delta was
    ///    non-negative (sender gained or stayed flat instead of losing
    ///    tokens).
    ///
    /// The two paths share this code; the diagnostic detail string in the
    /// `CheckResult` distinguishes them. Defense-in-depth on top of
    /// cross-payment replay protection: it pins which wallet the tokens
    /// came from, not just where they landed.
    pub const TRANSFER_SENDER_MISMATCH: u16 = 269;

    pub const RANGE: std::ops::RangeInclusive<u16> = 256..=319;
}

/// Custom resolution-reason codes for `x402/oracles/file-delivery/attestation/v1`.
pub mod file_delivery {
    pub const BLOB_SIZE_OUT_OF_RANGE: u16 = 320;
    pub const BLOB_MIME_MISMATCH: u16 = 321;
    pub const BLOB_ATTESTOR_SIGNATURE_INVALID: u16 = 322;
    pub const BLOB_UPLOAD_INCOMPLETE: u16 = 323;
    /// Wave A §1.1 — registry blob `created_at` predates `Payment.created_at`,
    /// the seller is reusing a pre-funding upload.
    pub const BLOB_PREDATES_PAYMENT: u16 = 324;
    /// Wave A §1.3 — same `delivery_hash` (blob) already settled for a
    /// different `payment_uid`. Cross-payment replay refusal.
    pub const BLOB_DELIVERY_HASH_REUSED: u16 = 325;
    /// Wave B §1.2 — companion-evidence's `payment_uid` does not match the
    /// on-chain `Payment.payment_uid`.
    pub const BLOB_PAYMENT_UID_MISMATCH: u16 = 326;
    /// Wave B §1.4 — SLA carries a `buyer_nonce` and the evidence didn't echo
    /// it back.
    pub const BLOB_BUYER_NONCE_MISMATCH: u16 = 327;

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
            onchain_transfer::TRANSFER_EVIDENCE_PREDATES_PAYMENT,
            onchain_transfer::TRANSFER_TX_SIGNATURE_REUSED,
            onchain_transfer::TRANSFER_PAYMENT_UID_MISMATCH,
            onchain_transfer::TRANSFER_BUYER_NONCE_MISMATCH,
            onchain_transfer::TRANSFER_BLOCK_TIME_MISSING,
            onchain_transfer::TRANSFER_SENDER_MISMATCH,
        ] {
            assert!(onchain_transfer::RANGE.contains(&code));
        }

        for code in [
            file_delivery::BLOB_SIZE_OUT_OF_RANGE,
            file_delivery::BLOB_MIME_MISMATCH,
            file_delivery::BLOB_ATTESTOR_SIGNATURE_INVALID,
            file_delivery::BLOB_UPLOAD_INCOMPLETE,
            file_delivery::BLOB_PREDATES_PAYMENT,
            file_delivery::BLOB_DELIVERY_HASH_REUSED,
            file_delivery::BLOB_PAYMENT_UID_MISMATCH,
            file_delivery::BLOB_BUYER_NONCE_MISMATCH,
        ] {
            assert!(file_delivery::RANGE.contains(&code));
        }
    }
}
