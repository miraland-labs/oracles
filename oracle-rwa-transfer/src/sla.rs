//! `TransferSla` for the rwa-transfer family.

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
    /// RESERVED (not enforced in v1). Declared for forward-compatibility with a
    /// future swap-delivery revision; the evaluator does not read it today.
    #[serde(default)]
    pub swap_router: Option<String>,
    /// RESERVED (not enforced in v1). See `swap_router`. Slippage is NOT checked
    /// by this evaluator — `min_amount` on each `ExpectedTransfer` is the only
    /// amount gate.
    #[serde(default)]
    pub slippage_bps: Option<u16>,
    #[serde(default)]
    pub deadline_unix: Option<i64>,
    /// Token-2022 program id the RWA mint MUST be owned by.
    pub token_program: String,
    /// Expected Transfer Hook program when the mint has the hook extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_hook_program: Option<String>,
    /// Issuer offering / tranche id (audit metadata).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offering_id: Option<String>,
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

#[cfg(test)]
mod fixture_tests {
    //! Lock the schema↔code contract: every shipped SLA/evidence example MUST
    //! deserialize into the live structs. Guards against drift like a missing
    //! required `token_program` / `payment_uid` (which previously made the
    //! fixtures and the devnet runbook un-parseable).
    use super::*;
    use crate::evidence::TransferEvidence;

    const SLA_APPROVE: &str =
        include_str!("../spec/rwa-transfer-v1/examples/sla.approve.json");
    const SLA_AMOUNT_INSUFFICIENT: &str =
        include_str!("../spec/rwa-transfer-v1/examples/sla.amount-insufficient.json");
    const SLA_WITH_SENDER: &str =
        include_str!("../spec/rwa-transfer-v1/examples/sla.with-sender-binding.json");
    const DELIVERY_APPROVE: &str =
        include_str!("../spec/rwa-transfer-v1/examples/delivery.approve.json");

    fn parse_sla(s: &str) -> TransferSla {
        serde_json::from_str(s).expect("example SLA must deserialize")
    }

    #[test]
    fn example_slas_deserialize_with_required_fields() {
        for raw in [SLA_APPROVE, SLA_AMOUNT_INSUFFICIENT, SLA_WITH_SENDER] {
            let sla = parse_sla(raw);
            assert!(sla.profile_id_matches(), "profile_id must match {PROFILE_ID}");
            assert!(!sla.payment_uid.is_empty(), "payment_uid is required");
            assert!(!sla.token_program.is_empty(), "token_program is required");
            assert!(!sla.expected_transfers.is_empty(), "expected_transfers non-empty");
        }
    }

    #[test]
    fn sender_binding_example_carries_hook_and_sender() {
        let sla = parse_sla(SLA_WITH_SENDER);
        assert!(sla.transfer_hook_program.is_some());
        assert!(sla.expected_transfers[0].sender_owner.is_some());
    }

    #[test]
    fn example_delivery_deserializes() {
        let ev: TransferEvidence =
            serde_json::from_str(DELIVERY_APPROVE).expect("example delivery must deserialize");
        assert_eq!(ev.profile_id, PROFILE_ID);
        assert!(!ev.tx_signature.is_empty());
        assert!(!ev.payment_uid.is_empty(), "evidence payment_uid is required");
    }
}
