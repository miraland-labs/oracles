//! Operator-side tip-floor economics for the worker.
//!
//! # Why this exists
//!
//! The on-chain `sla_escrow` program enforces only an upper bound on
//! `Payment.oracle_fee_bps` (`MAX_ORACLE_FEE_BPS = 500`, 5%). It does not
//! enforce a lower bound; a buyer-funded escrow with a tiny tip would still
//! settle and pay the oracle peanuts (or, on micro-payments, less than the
//! Solana priority-fee cost of issuing `ConfirmOracle`).
//!
//! Each oracle operator decides their own minimum acceptable tip via
//! [`OracleConfig::min_verdict_tip_default_raw`] and the optional per-mint map
//! [`OracleConfig::min_verdict_tip_by_mint_raw`]. The worker calls
//! [`evaluate_tip_floor`] before pipeline dispatch; underpriced jobs are
//! skipped and the Active Guardian eventually issues a fail-closed REJECT
//! before expiry so the buyer is refunded.
//!
//! All thresholds are stored in **raw mint units**, never dollars. The
//! daemon never consults a price feed — that's deliberate (determinism,
//! attack-surface, single-trust). Operators who want to maintain a stable
//! USD-equivalent floor across SOL price swings should run a separate
//! sidecar that updates the parameter row periodically; the daemon picks
//! up the new value on its config refresh cycle.
//!
//! # Default mint
//!
//! The pr402 ecosystem is USDC-first. When [`min_verdict_tip_default_raw`]
//! is unset, the operator is presumed to accept any USDC tip
//! ([`USDC_DEFAULT_FLOOR_RAW`] = `5_000` raw = `$0.005` at 6 decimals).
//! Other mints (SOL, USDT, custom) require an explicit per-mint entry; an
//! unrecognized mint with no entry passes through (zero floor) so an
//! operator who hasn't opted in to a non-USDC tip floor never inadvertently
//! refuses a valid job.
//!
//! [`OracleConfig::min_verdict_tip_default_raw`]: crate::config::OracleConfig
//! [`OracleConfig::min_verdict_tip_by_mint_raw`]: crate::config::OracleConfig
//! [`min_verdict_tip_default_raw`]: crate::config::OracleConfig

use std::collections::HashMap;

use solana_sdk::pubkey::Pubkey;

/// Default minimum-tip floor for the canonical pr402 stablecoin (USDC, 6
/// decimals): `5_000` raw = `$0.005`. Applied when an operator has no
/// explicit `ORACLE_MIN_VERDICT_TIP_DEFAULT_RAW` and the job's mint is USDC
/// (mainnet or devnet).
pub const USDC_DEFAULT_FLOOR_RAW: u64 = 5_000;

/// USDC mainnet mint pubkey (`EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`).
pub const USDC_MAINNET_MINT: Pubkey =
    solana_sdk::pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

/// USDC devnet mint pubkey (`4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`).
/// Devnet uses a separate USDC mint from mainnet so testing flows don't pollute
/// the production mint.
pub const USDC_DEVNET_MINT: Pubkey =
    solana_sdk::pubkey!("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU");

/// Outcome of a tip-floor evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipFloorVerdict {
    /// The projected tip clears the operator's floor. Job proceeds to pipeline.
    Accept {
        projected_tip_raw: u64,
        floor_raw: u64,
    },
    /// The projected tip is below the operator's floor. Worker should skip the
    /// job; the Active Guardian will eventually issue a fail-closed REJECT.
    Skip {
        projected_tip_raw: u64,
        floor_raw: u64,
    },
}

/// Pure tip-floor check. Computes the projected tip from `(amount, bps)` and
/// compares it against the operator's per-mint floor (with USDC fallback).
///
/// # Arguments
///
/// * `amount` — `Payment.amount`, raw mint units.
/// * `oracle_fee_bps` — `Payment.oracle_fee_bps`, snapshotted from `Escrow` at
///   funding time (0..=500).
/// * `mint` — `Payment.mint`. Used to look up a per-mint floor before falling
///   back to defaults.
/// * `min_default_raw` — operator-set default floor in raw units. `None` means
///   "no operator default; fall back to USDC convention if the mint is USDC".
/// * `min_by_mint_raw` — per-mint overrides; consulted first.
///
/// # Resolution order
///
/// 1. `min_by_mint_raw[mint]` if present.
/// 2. `min_default_raw` if `Some`.
/// 3. [`USDC_DEFAULT_FLOOR_RAW`] when the mint is USDC mainnet or devnet.
/// 4. `0` (accept anything) for unrecognized mints with no operator config.
pub fn evaluate_tip_floor(
    amount: u64,
    oracle_fee_bps: u16,
    mint: &Pubkey,
    min_default_raw: Option<u64>,
    min_by_mint_raw: &HashMap<Pubkey, u64>,
) -> TipFloorVerdict {
    let projected_tip_raw =
        ((amount as u128).saturating_mul(oracle_fee_bps as u128) / 10_000u128) as u64;

    let floor_raw = if let Some(&v) = min_by_mint_raw.get(mint) {
        v
    } else if let Some(v) = min_default_raw {
        v
    } else if mint == &USDC_MAINNET_MINT || mint == &USDC_DEVNET_MINT {
        USDC_DEFAULT_FLOOR_RAW
    } else {
        // Unknown mint, no operator override → don't refuse. Operators must
        // opt-in to non-USDC floors explicitly.
        0
    };

    if projected_tip_raw >= floor_raw {
        TipFloorVerdict::Accept {
            projected_tip_raw,
            floor_raw,
        }
    } else {
        TipFloorVerdict::Skip {
            projected_tip_raw,
            floor_raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_map() -> HashMap<Pubkey, u64> {
        HashMap::new()
    }

    #[test]
    fn usdc_with_100bps_on_one_dollar_clears_default_floor() {
        // $1 USDC × 100 bps = $0.01 = 10_000 raw → above $0.005 floor.
        let v = evaluate_tip_floor(1_000_000, 100, &USDC_MAINNET_MINT, None, &empty_map());
        assert_eq!(
            v,
            TipFloorVerdict::Accept {
                projected_tip_raw: 10_000,
                floor_raw: USDC_DEFAULT_FLOOR_RAW,
            }
        );
    }

    #[test]
    fn usdc_with_50bps_on_50cents_skips_default_floor() {
        // $0.50 USDC × 50 bps = $0.0025 = 2_500 raw → below $0.005 floor.
        let v = evaluate_tip_floor(500_000, 50, &USDC_MAINNET_MINT, None, &empty_map());
        assert_eq!(
            v,
            TipFloorVerdict::Skip {
                projected_tip_raw: 2_500,
                floor_raw: USDC_DEFAULT_FLOOR_RAW,
            }
        );
    }

    #[test]
    fn devnet_usdc_uses_same_default_floor() {
        let v = evaluate_tip_floor(500_000, 50, &USDC_DEVNET_MINT, None, &empty_map());
        assert!(matches!(v, TipFloorVerdict::Skip { .. }));
    }

    #[test]
    fn explicit_default_overrides_usdc_convention() {
        // Operator explicitly says "no floor at all" — accept anything.
        let v = evaluate_tip_floor(500_000, 50, &USDC_MAINNET_MINT, Some(0), &empty_map());
        assert_eq!(
            v,
            TipFloorVerdict::Accept {
                projected_tip_raw: 2_500,
                floor_raw: 0,
            }
        );
    }

    #[test]
    fn per_mint_override_wins_over_default() {
        let mint = Pubkey::new_unique();
        let mut map = HashMap::new();
        map.insert(mint, 100_000);
        // 1_000_000 raw × 50 bps = 5_000 raw, well below the per-mint floor.
        let v = evaluate_tip_floor(1_000_000, 50, &mint, Some(1_000), &map);
        assert_eq!(
            v,
            TipFloorVerdict::Skip {
                projected_tip_raw: 5_000,
                floor_raw: 100_000,
            }
        );
    }

    #[test]
    fn unknown_mint_with_no_config_accepts() {
        // No default, no per-mint entry, mint isn't USDC → floor = 0, accept.
        let mint = Pubkey::new_unique();
        let v = evaluate_tip_floor(100, 50, &mint, None, &empty_map());
        assert_eq!(
            v,
            TipFloorVerdict::Accept {
                projected_tip_raw: 0,
                floor_raw: 0,
            }
        );
    }

    #[test]
    fn zero_bps_yields_zero_tip() {
        // Disabled oracle tip — projected is 0. Falls below USDC default → Skip.
        let v = evaluate_tip_floor(1_000_000, 0, &USDC_MAINNET_MINT, None, &empty_map());
        assert_eq!(
            v,
            TipFloorVerdict::Skip {
                projected_tip_raw: 0,
                floor_raw: USDC_DEFAULT_FLOOR_RAW,
            }
        );
    }

    #[test]
    fn projected_tip_does_not_overflow_on_max_bps() {
        // u64::MAX × 500 bps fits in u128 safely; verify no panic.
        let v = evaluate_tip_floor(u64::MAX, 500, &USDC_MAINNET_MINT, None, &empty_map());
        // Mathematically: u64::MAX * 500 / 10_000 = u64::MAX / 20.
        let expected = (u64::MAX as u128 * 500 / 10_000) as u64;
        match v {
            TipFloorVerdict::Accept {
                projected_tip_raw, ..
            } => assert_eq!(projected_tip_raw, expected),
            other => panic!("expected Accept, got {other:?}"),
        }
    }
}
