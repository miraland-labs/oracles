//! Integration test for `oracle-file-delivery` against a real MinIO container.
//!
//! Covers Task 15.9: upload a 5 MiB blob via the `S3Backend` (MinIO has the same
//! API surface as AWS S3), round-trip it through `get_streaming`, and assert the
//! evaluator approves the resulting `FileDeliveryEvidence`.
//!
//! The test is **skip-when-unavailable**: if `ORACLE_TEST_MINIO_ENDPOINT` is
//! not set in the environment, the test prints a notice and returns success
//! without exercising MinIO. This keeps `cargo test --workspace` green on
//! developer machines and CI runners that don't provide MinIO. To run the
//! test for real, point at a local MinIO via:
//!
//! ```bash
//! sudo bash oracles/scripts/bootstrap-minio.sh
//! export ORACLE_TEST_MINIO_ENDPOINT=http://127.0.0.1:9000
//! export ORACLE_TEST_MINIO_BUCKET=oracle-test
//! export ORACLE_TEST_MINIO_ACCESS_KEY=oracle
//! export ORACLE_TEST_MINIO_SECRET_KEY=$(cat /etc/oracle/minio.secret)
//! export ORACLE_TEST_MINIO_REGION=us-east-1
//! cargo test -p oracle-file-delivery --test minio_integration -- --ignored --nocapture
//! ```
//!
//! The test asserts:
//!
//! 1. `S3Backend::put_streaming` returns the expected SHA-256 + size.
//! 2. `S3Backend::stat` reports the object after upload.
//! 3. `S3Backend::get` returns the exact bytes we uploaded.
//! 4. `S3Backend::get_streaming` chunks the body without altering it.
//! 5. The resulting evidence drives `FileDeliveryEvaluator` to an APPROVE.
//! 6. Oversized payloads abort with `TooLarge` (P-REG-2 enforcement).
//! 7. Unknown hashes return `NotFound`.

use std::sync::Arc;

use bytes::Bytes;
use futures_util::stream::{self, StreamExt};
use oracle_common::{
    config::S3Config,
    evaluator::{EvaluationContext, OracleEvaluator},
    registry::storage::{ByteOutStream, ByteStream, S3Backend, StorageBackend, StorageError},
    types::{EvaluationJob, EvaluationResult},
};
use oracle_file_delivery::{
    evaluator::FileDeliveryEvaluator, evidence::FileDeliveryEvidence, sla::FileDeliverySla,
};
use sha2::{Digest, Sha256};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;

const TEST_BLOB_SIZE: usize = 5 * 1024 * 1024; // 5 MiB
const SMALL_CAP: u64 = 1024; // for the cap-enforcement check

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn minio_config() -> Option<S3Config> {
    Some(S3Config {
        endpoint: env("ORACLE_TEST_MINIO_ENDPOINT")?,
        bucket: env("ORACLE_TEST_MINIO_BUCKET").unwrap_or_else(|| "oracle-test".into()),
        access_key: env("ORACLE_TEST_MINIO_ACCESS_KEY").unwrap_or_else(|| "minioadmin".into()),
        secret_key: env("ORACLE_TEST_MINIO_SECRET_KEY").unwrap_or_else(|| "minioadmin".into()),
        region: env("ORACLE_TEST_MINIO_REGION").unwrap_or_else(|| "us-east-1".into()),
    })
}

fn make_blob(seed: u8) -> Vec<u8> {
    // Deterministic, non-trivial content so MIME-sniff doesn't accidentally match
    // a known type and the test is reproducible across runs.
    let mut buf = vec![0u8; TEST_BLOB_SIZE];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = ((i as u8).wrapping_mul(seed)).wrapping_add(seed);
    }
    buf
}

fn into_byte_stream(bytes: Vec<u8>) -> ByteStream {
    let chunks = bytes
        .chunks(64 * 1024)
        .map(|c| Ok(Bytes::from(c.to_vec())))
        .collect::<Vec<_>>();
    Box::pin(stream::iter(chunks))
}

async fn collect_bytes(mut s: ByteOutStream) -> Result<Bytes, StorageError> {
    let mut acc = Vec::new();
    while let Some(chunk) = s.next().await {
        let chunk = chunk?;
        acc.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(acc))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Marked `#[ignore]` so `cargo test --workspace` skips it by default; CI or
/// operators run it explicitly with `--ignored`. See module docs for setup.
#[tokio::test]
#[ignore]
async fn minio_round_trip_5mib_blob() {
    let cfg = match minio_config() {
        Some(c) => c,
        None => {
            eprintln!(
                "ORACLE_TEST_MINIO_ENDPOINT unset — skipping. Bootstrap MinIO via \
                 oracles/scripts/bootstrap-minio.sh and re-run with \
                 `cargo test ... -- --ignored`."
            );
            return;
        }
    };

    let backend = S3Backend::new(&cfg).await;
    let blob = make_blob(0x5A);
    let expected_hash = sha256_hex(&blob);
    let expected_size = blob.len() as u64;

    // 1. Upload streaming with the default 5 GiB cap.
    let stored = backend
        .put_streaming(into_byte_stream(blob.clone()), 5 * 1024 * 1024 * 1024, None)
        .await
        .expect("put_streaming");
    assert_eq!(
        stored.hash_hex, expected_hash,
        "incremental SHA-256 must match"
    );
    assert_eq!(stored.size_bytes, expected_size, "size must match");

    // 2. Stat reports the object.
    let stat = backend
        .stat(&expected_hash)
        .await
        .expect("stat ok")
        .expect("must exist");
    assert_eq!(stat.size_bytes, expected_size);

    // 3. get returns the same bytes.
    let got = backend.get(&expected_hash).await.expect("get");
    assert_eq!(got.len(), blob.len());
    assert_eq!(sha256_hex(&got), expected_hash, "round-trip hash");

    // 4. get_streaming returns equivalent bytes (chunked — depends on backend).
    let streamed = collect_bytes(
        backend
            .get_streaming(&expected_hash)
            .await
            .expect("get_streaming"),
    )
    .await
    .expect("collect");
    assert_eq!(sha256_hex(&streamed), expected_hash, "streaming round-trip");

    // 5. Build the SLA + evidence and run the evaluator.
    let sla = FileDeliverySla {
        version: 1,
        profile_id: "x402/oracles/file-delivery/attestation/v1".into(),
        payment_uid: "00".repeat(32),
        buyer_nonce: None,
        expected_size_bytes_min: 4 * 1024 * 1024,
        expected_size_bytes_max: 6 * 1024 * 1024,
        expected_mime: None,
        expected_extension: None,
        attestor_pubkey: None,
    };
    let evidence = FileDeliveryEvidence {
        size_bytes: stored.size_bytes,
        sniffed_mime: None,
        blob_sha256_hex: stored.hash_hex.clone(),
    };

    let evaluator = FileDeliveryEvaluator::new();
    let job = EvaluationJob {
        payment_uid: [1u8; 32],
        payment_pubkey: Pubkey::new_unique(),
        sla_hash: [2u8; 32],
        delivery_hash: hex::decode(&stored.hash_hex)
            .expect("hex decode")
            .try_into()
            .expect("32 bytes"),
        oracle_authority: Pubkey::new_unique(),
        oracle_fee_bps: 100,
        mint: Pubkey::new_unique(),
        amount: 1,
        expires_at: i64::MAX,
        created_at: 0,
        delivery_cutoff_seconds: 0,
        sla_bytes: Some(Bytes::from(serde_json::to_vec(&sla).unwrap())),
        retry_count: 0,
    };
    let rpc: Arc<RpcClient> = Arc::new(RpcClient::new("http://127.0.0.1:8899".into()));
    let http = reqwest::Client::new();
    let ctx = EvaluationContext {
        rpc: &rpc,
        http: &http,
        job: &job,
        strict: true,
        ledger: None,
    };
    let result: EvaluationResult = evaluator
        .evaluate(&ctx, &sla, &evidence)
        .await
        .expect("evaluate");
    assert!(
        result.approved,
        "evaluator must approve the round-tripped blob: {:?}",
        result.checks
    );
    assert_eq!(result.resolution_reason, 0, "approval reason must be 0");
}

#[tokio::test]
#[ignore]
async fn minio_oversized_upload_is_aborted() {
    let cfg = match minio_config() {
        Some(c) => c,
        None => {
            eprintln!("ORACLE_TEST_MINIO_ENDPOINT unset — skipping.");
            return;
        }
    };
    let backend = S3Backend::new(&cfg).await;
    let blob = vec![0xAAu8; (SMALL_CAP as usize) + 16];
    let result = backend
        .put_streaming(into_byte_stream(blob), SMALL_CAP, None)
        .await;
    match result {
        Err(StorageError::TooLarge { size, max }) => {
            assert!(size > max, "size {size} must exceed cap {max}");
        }
        Ok(stored) => panic!("expected TooLarge, got {stored:?}"),
        Err(e) => panic!("expected TooLarge, got {e:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn minio_get_unknown_hash_returns_not_found() {
    let cfg = match minio_config() {
        Some(c) => c,
        None => {
            eprintln!("ORACLE_TEST_MINIO_ENDPOINT unset — skipping.");
            return;
        }
    };
    let backend = S3Backend::new(&cfg).await;
    let absent = "0".repeat(64);
    match backend.get(&absent).await {
        Err(StorageError::NotFound(h)) => assert_eq!(h, absent),
        Err(e) => panic!("expected NotFound, got {e:?}"),
        Ok(b) => panic!("expected NotFound, got {} bytes", b.len()),
    }
}
