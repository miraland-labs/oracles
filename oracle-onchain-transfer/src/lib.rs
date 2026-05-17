//! `oracle-onchain-transfer` library. Hosts the SLA + evidence types and the evaluator
//! impl for the `x402/oracle/onchain-transfer/v1` profile. Implementation lives in Tasks 14.1–14.3.

pub mod evaluator;
pub mod evidence;
pub mod sla;

/// Canonical profile identifier this binary registers under.
pub const PROFILE_ID: &str = "x402/oracle/onchain-transfer/v1";
