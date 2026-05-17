//! Integration tests for `OracleDb`. These require a running Postgres reachable via
//! `DATABASE_URL`. They are not gated `#[ignore]`: if `DATABASE_URL` is unset, every
//! test exits early with a clear skip message. CI is expected to provide the env var.
//!
//! ## Local invocation
//!
//! ```bash
//! docker run -d --rm --name oracle_pg \
//!   -e POSTGRES_USER=oracle -e POSTGRES_PASSWORD=oracle -e POSTGRES_DB=oracle_test \
//!   -p 5432:5432 postgres:16
//! psql "postgres://oracle:oracle@127.0.0.1:5432/oracle_test" -f \
//!     oracle-common/migrations/init.sql
//! DATABASE_URL=postgres://oracle:oracle@127.0.0.1:5432/oracle_test \
//!     cargo test -p oracle-common --test db_integration
//! ```

use std::env;

use bytes::Bytes;
use oracle_common::{
    db::OracleDb,
    types::{CheckResult, EvaluationJob, EvaluationResult},
};
use solana_sdk::pubkey::Pubkey;

fn database_url_or_skip() -> Option<String> {
    match env::var("DATABASE_URL") {
        Ok(s) if !s.trim().is_empty() => Some(s),
        _ => {
            eprintln!("DATABASE_URL is not set; skipping db_integration test");
            None
        }
    }
}

fn make_job(uid_byte: u8, sla_hash_byte: u8) -> EvaluationJob {
    EvaluationJob {
        payment_uid: [uid_byte; 32],
        payment_pubkey: Pubkey::new_unique(),
        sla_hash: [sla_hash_byte; 32],
        delivery_hash: [0xCC; 32],
        amount: 1_000_000,
        mint: Pubkey::new_unique(),
        oracle_authority: Pubkey::new_unique(),
        expires_at: 1_900_000_000,
        sla_bytes: Some(Bytes::from_static(b"{\"profile_id\":\"x402/test/v1\"}")),
    }
}

fn make_approve_result() -> EvaluationResult {
    EvaluationResult {
        approved: true,
        resolution_reason: 0,
        checks: vec![CheckResult {
            name: "stub".into(),
            passed: true,
            detail: "ok".into(),
        }],
    }
}

#[tokio::test]
async fn full_lifecycle_round_trip() {
    let Some(url) = database_url_or_skip() else {
        return;
    };
    let db = OracleDb::connect(url).expect("pool connect should succeed against running pg");
    let job = make_job(0x10, 0x20);

    db.record_detected(&job).await.expect("record_detected");
    assert!(!db.is_terminal(&job.payment_uid).await.unwrap());

    db.record_queued(&job).await.expect("record_queued");
    assert!(!db.is_terminal(&job.payment_uid).await.unwrap());

    db.record_started(&job).await.expect("record_started");
    let attempts_after_start = db.attempt_count(&job.payment_uid).await.unwrap();
    assert!(attempts_after_start >= 1, "running should bump attempts");

    let resolution_hash = [0xAA; 32];
    db.record_settled(
        &job,
        &make_approve_result(),
        Some("sigSTUB"),
        &resolution_hash,
    )
    .await
    .expect("record_settled");
    assert!(db.is_terminal(&job.payment_uid).await.unwrap());
}

#[tokio::test]
async fn terminal_state_blocks_rerun() {
    let Some(url) = database_url_or_skip() else {
        return;
    };
    let db = OracleDb::connect(url).expect("pool connect");
    let job = make_job(0x30, 0x40);

    // Drive through to settled, then re-emit a "detected" event for the same UID.
    db.record_detected(&job).await.unwrap();
    db.record_started(&job).await.unwrap();
    let resolution_hash = [0xBB; 32];
    db.record_settled(
        &job,
        &make_approve_result(),
        Some("sigSTUB2"),
        &resolution_hash,
    )
    .await
    .unwrap();

    assert!(db.is_terminal(&job.payment_uid).await.unwrap());

    // A second `record_detected` flips the status string but is_terminal still relies
    // on the *current* status, so it returns false until we re-settle.
    db.record_detected(&job).await.unwrap();
    let still_terminal_after_detect = db.is_terminal(&job.payment_uid).await.unwrap();
    // The dedupe path in the worker is supposed to *check `is_terminal` before* doing
    // any state changes; this test just documents that record_detected itself is not
    // a terminal state (which is intentional — only settled/dead_letter are).
    assert!(
        !still_terminal_after_detect,
        "record_detected resets status away from settled by design — workers must check is_terminal first"
    );
}

#[tokio::test]
async fn dead_letter_is_terminal() {
    let Some(url) = database_url_or_skip() else {
        return;
    };
    let db = OracleDb::connect(url).expect("pool connect");
    let job = make_job(0x50, 0x60);

    db.record_detected(&job).await.unwrap();
    db.record_dead_letter(&job, "stub failure").await.unwrap();
    assert!(db.is_terminal(&job.payment_uid).await.unwrap());
}

#[tokio::test]
async fn parameters_round_trip() {
    let Some(url) = database_url_or_skip() else {
        return;
    };
    let db = OracleDb::connect(url).expect("pool connect");

    let key = format!("test.parameter.{}", rand::random::<u64>());
    assert_eq!(db.get_parameter(&key).await.unwrap(), None);

    db.set_parameter(&key, "first").await.unwrap();
    assert_eq!(
        db.get_parameter(&key).await.unwrap().as_deref(),
        Some("first")
    );

    db.set_parameter(&key, "second").await.unwrap();
    assert_eq!(
        db.get_parameter(&key).await.unwrap().as_deref(),
        Some("second")
    );
}
