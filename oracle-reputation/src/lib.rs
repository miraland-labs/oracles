//! Oracle reputation indexer for the x402 SLA-Escrow ecosystem.
//!
//! Subscribes to the on-chain `sla-escrow` program's `logsSubscribe` stream,
//! decodes the six payment-lifecycle events that drive oracle scorecards, and
//! writes them as raw rows in the `oracle_events` Postgres table. Materialized
//! roll-ups (settlement rate, approval rate, latency percentiles, by-amount
//! buckets) live as SQL views over that table — they are added in later
//! increments without changing this indexing layer.
//!
//! See `oracles/docs/REPUTATION_INDEXER.md` for the architectural rationale.
//!
//! # Increment 1 scope
//!
//! - Event decoder for the six payment-lifecycle events.
//! - Postgres writer (idempotent on `(signature, log_index)`).
//! - WebSocket `logsSubscribe` ingester with reconnect.
//! - Slot cursor + opportunistic backfill on startup.
//!
//! Out of scope until later increments: per-payment roll-up table, SQL views,
//! HTTP API, multi-Facilitator coordination.

pub mod config;
pub mod decoder;
pub mod ingester;
pub mod store;
