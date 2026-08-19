//! `oracle-file-delivery` library. Hosts the SLA + streaming-fetch evidence types and
//! the evaluator impl for the `x402/oracles/file-delivery/attestation/v1` profile.
//! Implementation lives in Tasks 15.1–15.4.

pub mod evaluator;
pub mod evidence;
pub mod fetcher;
pub mod runner;
pub mod sla;

/// Canonical profile identifier this binary registers under.
pub const PROFILE_ID: &str = "x402/oracles/file-delivery/attestation/v1";
