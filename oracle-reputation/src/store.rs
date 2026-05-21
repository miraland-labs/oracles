//! Postgres writer for decoded events.
//!
//! Idempotent on `(signature, log_index)` — the chain can replay the same
//! transaction (slot reorg, RPC re-stream, indexer restart) and we never
//! double-count.

use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use postgres_openssl::MakeTlsConnector;
use thiserror::Error;
use tokio_postgres::types::ToSql;

use crate::decoder::DecodedEvent;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("postgres pool: {0}")]
    Pool(String),
    #[error("postgres query: {0}")]
    Query(String),
    #[error("openssl: {0}")]
    Tls(String),
}

/// Postgres-backed store for the `oracle_events` table.
#[derive(Clone)]
pub struct EventStore {
    pool: Pool,
}

impl EventStore {
    /// Connect using a libpq URL. Mirrors the connection pattern used by
    /// `oracle-common::db` so operators can point both at the same host.
    pub fn connect(url: &str) -> Result<Self, StoreError> {
        let mut cfg = Config::new();
        cfg.url = Some(url.to_string());
        cfg.manager = Some(ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        });

        let mut builder =
            SslConnector::builder(SslMethod::tls()).map_err(|e| StoreError::Tls(e.to_string()))?;
        builder.set_verify(SslVerifyMode::NONE);
        let tls = MakeTlsConnector::new(builder.build());

        let pool = cfg
            .create_pool(Some(Runtime::Tokio1), tls)
            .map_err(|e| StoreError::Pool(e.to_string()))?;

        Ok(Self { pool })
    }

    /// Insert a decoded event. Idempotent on `(signature, log_index)`. Returns
    /// `true` if a new row was inserted, `false` if the conflict path was hit.
    pub async fn insert_event(
        &self,
        signature: &str,
        log_index: i32,
        slot: i64,
        block_time: i64,
        event: &DecodedEvent,
    ) -> Result<bool, StoreError> {
        const SQL: &str = r#"
            INSERT INTO oracle_events
                (signature, log_index, slot, block_time, event_type, payment_uid, payload)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (signature, log_index) DO NOTHING
        "#;

        let payload = event.to_json();
        let payment_uid = event.payment_uid().to_vec();
        let event_type = event.event_type();

        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StoreError::Pool(e.to_string()))?;

        let params: [&(dyn ToSql + Sync); 7] = [
            &signature,
            &log_index,
            &slot,
            &block_time,
            &event_type,
            &payment_uid,
            &payload,
        ];

        let n = client
            .execute(SQL, &params)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(n == 1)
    }

    /// Read the last processed slot from `oracle_reputation_cursor`. Returns
    /// `None` on a fresh deployment.
    pub async fn read_cursor(&self) -> Result<Option<i64>, StoreError> {
        const SQL: &str = "SELECT last_slot FROM oracle_reputation_cursor WHERE id = 1";
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StoreError::Pool(e.to_string()))?;
        let row = client
            .query_opt(SQL, &[])
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(row.and_then(|r| r.get::<_, Option<i64>>(0)))
    }

    /// Persist the most recently processed slot. Called after a successful
    /// batch insert so the next restart can resume cleanly.
    pub async fn write_cursor(&self, slot: i64) -> Result<(), StoreError> {
        const SQL: &str = r#"
            UPDATE oracle_reputation_cursor
            SET last_slot = $1, updated_at = NOW()
            WHERE id = 1
        "#;
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StoreError::Pool(e.to_string()))?;
        client
            .execute(SQL, &[&slot])
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(())
    }
}
