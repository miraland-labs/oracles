//! Shared library for x402 SLA-Escrow oracle implementations.
//!
//! See [the design document](https://github.com/miraland-labs/x402/blob/main/.kiro/specs/multi-category-oracle-architecture/design.md)
//! for the architectural rationale. This crate provides the chain monitor, evidence
//! fetcher, settler, ledger, registration HTTP, storage backends, profile registry, and
//! HTTP server skeleton that every per-family oracle binary reuses.

pub mod chain;
pub mod config;
pub mod db;
pub mod economics;
pub mod error;
pub mod evaluator;
pub mod fetcher;
pub mod pipeline;
pub mod profile;
pub mod registry;
pub mod resolution_codes;
pub mod server;
pub mod settler;
pub mod types;
pub mod worker;

pub use error::OracleError;
