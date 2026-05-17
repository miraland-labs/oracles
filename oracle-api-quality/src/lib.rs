//! `oracle-api-quality` library. Hosts the SLA + evidence types and the evaluator
//! impl for the `x402/oracle/api-quality/v1` profile. Implementation lives in Tasks 13.1–13.3.

pub mod evaluator;
pub mod evidence;
pub mod sla;

/// Canonical profile identifier this binary registers under.
pub const PROFILE_ID: &str = "x402/oracle/api-quality/v1";
