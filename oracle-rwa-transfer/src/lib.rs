//! `oracle-rwa-transfer` library. Hosts the SLA + evidence types and the
//! `TransferEvaluator` for the `x402/oracles/rwa-transfer/v1` profile: it
//! re-derives Token-2022 transfer deltas from `getTransaction(jsonParsed)`,
//! pins the mint's token program and Transfer Hook program, and enforces the
//! payment_uid / buyer_nonce bindings and cross-payment replay refusal.

pub mod evaluator;
pub mod evidence;
pub mod sla;

/// Canonical profile identifier this binary registers under.
pub const PROFILE_ID: &str = "x402/oracles/rwa-transfer/v1";
