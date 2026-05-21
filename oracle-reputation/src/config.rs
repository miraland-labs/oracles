//! Env-driven configuration for the reputation indexer.
//!
//! All keys default sensibly; only `DATABASE_URL` is required (no point indexing
//! into a void). RPC endpoints default to public devnet so a fresh clone can
//! sanity-check without operator prep.

use std::{env, str::FromStr};

use solana_sdk::pubkey::Pubkey;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("required env var {0} is not set")]
    Missing(String),
    #[error("env var {var} is not valid: {message}")]
    Invalid { var: String, message: String },
}

/// Reputation indexer configuration.
#[derive(Debug, Clone)]
pub struct ReputationConfig {
    /// HTTPS RPC for `getTransaction` and `getSignaturesForAddress` (used during
    /// startup backfill and retries).
    pub solana_rpc_url: String,
    /// WebSocket RPC for `logsSubscribe`.
    pub solana_ws_url: String,
    /// `sla-escrow` program id to filter events by. Defaults to the
    /// `sla_escrow_api::ID` baked into the linked crate.
    pub escrow_program_id: Pubkey,
    /// Postgres URL for the `oracle_events` table.
    pub database_url: String,
    /// On startup, look back at most this many recent signatures for the
    /// escrow program (capped fetch — never replays the whole history).
    /// `0` disables backfill (subscribe-only mode).
    pub backfill_lookback_signatures: usize,
}

impl ReputationConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let solana_rpc_url =
            env::var("SOLANA_RPC_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".into());
        let solana_ws_url =
            env::var("SOLANA_WS_URL").unwrap_or_else(|_| "wss://api.devnet.solana.com".into());

        let escrow_program_id = match env::var("ESCROW_PROGRAM_ID").ok() {
            Some(s) if !s.trim().is_empty() => {
                Pubkey::from_str(s.trim()).map_err(|e| ConfigError::Invalid {
                    var: "ESCROW_PROGRAM_ID".into(),
                    message: e.to_string(),
                })?
            }
            _ => sla_escrow_api::ID,
        };

        let database_url = env::var("DATABASE_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ConfigError::Missing("DATABASE_URL".into()))?;

        let backfill_lookback_signatures = env::var("REPUTATION_BACKFILL_LOOKBACK_SIGNATURES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(2_000);

        Ok(Self {
            solana_rpc_url,
            solana_ws_url,
            escrow_program_id,
            database_url,
            backfill_lookback_signatures,
        })
    }
}
