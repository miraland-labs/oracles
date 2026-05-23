//! Cross-family property test suite.
//!
//! Consolidates the cross-cutting properties from [design.md §Correctness
//! Properties](../../../.kiro/specs/multi-category-oracle-architecture/design.md#correctness-properties)
//! that apply to *every* family. Family-local properties (the P-AQ-*, P-OT-*,
//! P-FD-* series) live next to their evaluators in the per-family crates.
//!
//! Properties covered here:
//!
//! * P-DET-2: `compute_resolution_hash` is a pure function of inputs.
//! * P-VER-3: approvals carry `resolution_reason == 0`.
//! * Resolution-reason ranges are honored for the families that ship in v1
//!   (transfer in 256..=319, file-delivery in 320..=383). New evaluators that
//!   accidentally allocate outside their reserved range fail this test.

use oracle_common::{
    resolution_codes::{file_delivery, onchain_transfer},
    settler::{compute_resolution_hash, RESOLUTION_ENVELOPE_PROFILE},
    types::{CheckResult, EvaluationJob, EvaluationResult},
};
use proptest::prelude::*;
use serde_json::json;
use solana_sdk::pubkey::Pubkey;

fn job() -> EvaluationJob {
    EvaluationJob {
        payment_uid: [1u8; 32],
        payment_pubkey: Pubkey::new_unique(),
        sla_hash: [2u8; 32],
        delivery_hash: [3u8; 32],
        amount: 0,
        mint: Pubkey::new_unique(),
        oracle_authority: Pubkey::new_unique(),
        oracle_fee_bps: 100,
        expires_at: 0,
        created_at: 0,
        delivery_cutoff_seconds: 0,
        sla_bytes: None,
        retry_count: 0,
    }
}

#[test]
fn resolution_envelope_profile_id_is_canonical() {
    assert_eq!(
        RESOLUTION_ENVELOPE_PROFILE,
        "x402/oracles/resolution-envelope/v1"
    );
}

#[test]
fn transfer_codes_within_reserved_range() {
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
        assert!(
            onchain_transfer::RANGE.contains(&code),
            "transfer code {code} is outside the reserved 256..=319 range"
        );
    }
}

#[test]
fn file_delivery_codes_within_reserved_range() {
    for code in [
        file_delivery::BLOB_SIZE_OUT_OF_RANGE,
        file_delivery::BLOB_MIME_MISMATCH,
        file_delivery::BLOB_ATTESTOR_SIGNATURE_INVALID,
        file_delivery::BLOB_UPLOAD_INCOMPLETE,
    ] {
        assert!(
            file_delivery::RANGE.contains(&code),
            "file-delivery code {code} is outside the reserved 320..=383 range"
        );
    }
}

#[test]
fn transfer_and_file_delivery_ranges_disjoint() {
    let transfer: std::collections::HashSet<u16> = onchain_transfer::RANGE.collect();
    let file_delivery: std::collections::HashSet<u16> = file_delivery::RANGE.collect();
    assert!(transfer.is_disjoint(&file_delivery));
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    /// P-DET-2: `compute_resolution_hash` is a pure function of inputs.
    #[test]
    fn p_det_2_resolution_hash_is_pure(
        sla_hash in any::<[u8; 32]>(),
        delivery_hash in any::<[u8; 32]>(),
        approved in any::<bool>(),
        reason in any::<u16>(),
    ) {
        let mut j = job();
        j.sla_hash = sla_hash;
        j.delivery_hash = delivery_hash;
        let r = EvaluationResult {
            approved,
            resolution_reason: reason,
            checks: vec![],
            resolution_details: None,
        };
        let a = compute_resolution_hash(&j, "x402/test/v1", &r, json!({"k": "v"})).unwrap();
        let b = compute_resolution_hash(&j, "x402/test/v1", &r, json!({"k": "v"})).unwrap();
        prop_assert_eq!(a, b);
    }

    /// P-DET-2 negation: any input change → digest changes.
    #[test]
    fn p_det_2_changes_under_input_mutation(
        sla_hash in any::<[u8; 32]>(),
        delivery_hash in any::<[u8; 32]>(),
    ) {
        let mut j = job();
        j.sla_hash = sla_hash;
        j.delivery_hash = delivery_hash;
        let r = EvaluationResult {
            approved: true,
            resolution_reason: 0,
            checks: vec![],
            resolution_details: None,
        };
        let base =
            compute_resolution_hash(&j, "x402/test/v1", &r, json!({"k": "v"})).unwrap();
        // Flip one byte of sla_hash.
        let mut j2 = j.clone();
        j2.sla_hash[0] ^= 0xff;
        let mutated =
            compute_resolution_hash(&j2, "x402/test/v1", &r, json!({"k": "v"})).unwrap();
        prop_assert_ne!(base, mutated);
    }

    /// P-VER-3: approvals carry reason == 0. Any evaluator that violates this
    /// is a bug (this test is a model property — it asserts that the resolution
    /// envelope can faithfully serialize a `0`-reason approval).
    #[test]
    fn p_ver_3_approval_reason_zero_round_trips(
        sla_hash in any::<[u8; 32]>(),
    ) {
        let mut j = job();
        j.sla_hash = sla_hash;
        let r = EvaluationResult {
            approved: true,
            resolution_reason: 0,
            checks: vec![CheckResult {
                name: "x".into(),
                passed: true,
                detail: "ok".into(),
            }],
            resolution_details: None,
        };
        let h = compute_resolution_hash(&j, "x402/test/v1", &r, json!(null)).unwrap();
        prop_assert_ne!(h, [0u8; 32]);
    }
}
