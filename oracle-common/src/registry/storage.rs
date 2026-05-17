//! Content-addressed storage backends for the registration HTTP API.
//!
//! Three backends are supported (configured via `ORACLE_REGISTRY_BACKEND`):
//!
//! * [`PostgresBackend`] — small payloads (≤4 MiB by default) stored as `BYTEA` in
//!   `oracle_artifacts`.
//! * [`S3Backend`] — large blobs (default cap 5 GiB) stored at
//!   `oracle-blobs/<sha256_hex>` against any S3-compatible endpoint (AWS S3, MinIO,
//!   Cloudflare R2, Backblaze B2, Wasabi).
//! * [`LocalFsBackend`] — single-host development. Files live at
//!   `<root>/<hash[0..2]>/<hash>`.
//!
//! Each backend computes SHA-256 incrementally during writes (cap-enforced; an
//! oversized stream is aborted and any partial bytes are removed). On reads, the
//! caller is responsible for re-verifying the digest before parsing — the
//! `EvidenceFetcher` does this in `oracle-common/src/fetcher.rs`.

use std::{path::PathBuf, pin::Pin, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::{timeout, Duration},
};
use tokio_postgres::types::Type;

use crate::config::S3Config;

/// Stream type used for streaming uploads.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send + Unpin>>;

/// Stream type returned by streaming downloads.
pub type ByteOutStream = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send + Unpin>>;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("payload exceeds limit ({size} > {max})")]
    TooLarge { size: u64, max: u64 },
    #[error("hash mismatch: expected {expected}, got {got}")]
    HashMismatch { expected: String, got: String },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("upload incomplete (truncated stream)")]
    UploadIncomplete,
    #[error("backend i/o: {0}")]
    Io(String),
    #[error("backend timeout")]
    Timeout,
}

#[derive(Debug, Clone)]
pub struct StoredObject {
    pub hash_hex: String,
    pub size_bytes: u64,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ObjectStat {
    pub size_bytes: u64,
    pub content_type: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Storage abstraction: the shape every backend implements.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Stream-store a blob. The stream must produce all bytes; the backend
    /// computes SHA-256 on the fly and returns the digest. The implementation
    /// MUST abort and remove any partial state if the running total exceeds
    /// `max_bytes`.
    async fn put_streaming(
        &self,
        body: ByteStream,
        max_bytes: u64,
        content_type: Option<String>,
    ) -> Result<StoredObject, StorageError>;

    /// Fetch the entire blob into memory. Use this for SLA / small evidence; for
    /// blobs ≥ a few MiB use [`StorageBackend::get_streaming`] instead.
    async fn get(&self, hash_hex: &str) -> Result<Bytes, StorageError>;

    /// Stream-fetch a blob; the receiver must verify SHA-256 over the chunked body
    /// before parsing.
    async fn get_streaming(&self, hash_hex: &str) -> Result<ByteOutStream, StorageError>;

    /// HEAD / stat. Returns `None` for absent objects.
    async fn stat(&self, hash_hex: &str) -> Result<Option<ObjectStat>, StorageError>;
}

// =====================================================================
// Postgres backend
// =====================================================================

#[derive(Clone)]
pub struct PostgresBackend {
    pool: Pool,
}

impl PostgresBackend {
    const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl StorageBackend for PostgresBackend {
    async fn put_streaming(
        &self,
        mut body: ByteStream,
        max_bytes: u64,
        _content_type: Option<String>,
    ) -> Result<StoredObject, StorageError> {
        let mut hasher = Sha256::new();
        let mut buf: Vec<u8> = Vec::new();

        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| StorageError::Io(e.to_string()))?;
            let new_size = buf.len() as u64 + chunk.len() as u64;
            if new_size > max_bytes {
                return Err(StorageError::TooLarge {
                    size: new_size,
                    max: max_bytes,
                });
            }
            hasher.update(&chunk);
            buf.extend_from_slice(&chunk);
        }

        let digest = hasher.finalize();
        let hash_hex = hex::encode(digest);
        let size_bytes = buf.len() as u64;

        const SQL: &str = r#"
            INSERT INTO oracle_artifacts (sha256_hex, bytes)
            VALUES ($1, $2)
            ON CONFLICT (sha256_hex) DO NOTHING
        "#;
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Io(format!("pool: {e}")))?;
        timeout(Self::QUERY_TIMEOUT, client.execute(SQL, &[&hash_hex, &buf]))
            .await
            .map_err(|_| StorageError::Timeout)?
            .map_err(|e| StorageError::Io(e.to_string()))?;

        Ok(StoredObject {
            hash_hex,
            size_bytes,
            content_type: None,
        })
    }

    async fn get(&self, hash_hex: &str) -> Result<Bytes, StorageError> {
        const SQL: &str = r#"SELECT bytes FROM oracle_artifacts WHERE sha256_hex = $1"#;
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Io(format!("pool: {e}")))?;
        let row = timeout(Self::QUERY_TIMEOUT, client.query_opt(SQL, &[&hash_hex]))
            .await
            .map_err(|_| StorageError::Timeout)?
            .map_err(|e| StorageError::Io(e.to_string()))?
            .ok_or_else(|| StorageError::NotFound(hash_hex.to_string()))?;
        let bytes: Vec<u8> = row
            .try_get(0)
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(Bytes::from(bytes))
    }

    async fn get_streaming(&self, hash_hex: &str) -> Result<ByteOutStream, StorageError> {
        // Postgres BYTEA is small (≤4 MiB by config); just load + wrap.
        let bytes = self.get(hash_hex).await?;
        let stream = futures_util::stream::iter(vec![Ok(bytes)]);
        Ok(Box::pin(stream))
    }

    async fn stat(&self, hash_hex: &str) -> Result<Option<ObjectStat>, StorageError> {
        const SQL: &str = r#"
            SELECT octet_length(bytes), created_at
              FROM oracle_artifacts
             WHERE sha256_hex = $1
        "#;
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| StorageError::Io(format!("pool: {e}")))?;
        let row = timeout(Self::QUERY_TIMEOUT, client.query_opt(SQL, &[&hash_hex]))
            .await
            .map_err(|_| StorageError::Timeout)?
            .map_err(|e| StorageError::Io(e.to_string()))?;
        match row {
            Some(r) => {
                let size: i32 = r.try_get(0).map_err(|e| StorageError::Io(e.to_string()))?;
                let created_at: DateTime<Utc> =
                    r.try_get(1).map_err(|e| StorageError::Io(e.to_string()))?;
                Ok(Some(ObjectStat {
                    size_bytes: size as u64,
                    content_type: None,
                    created_at,
                }))
            }
            None => Ok(None),
        }
    }
}

// Suppress unused-import lint when we end up not needing tokio_postgres::types::Type.
#[allow(dead_code)]
fn _type_check_compile_only() -> Type {
    Type::BYTEA
}

// =====================================================================
// S3-compatible backend
// =====================================================================

#[derive(Clone)]
pub struct S3Backend {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl S3Backend {
    pub async fn new(cfg: &S3Config) -> Self {
        let creds = aws_sdk_s3::config::Credentials::new(
            cfg.access_key.clone(),
            cfg.secret_key.clone(),
            None,
            None,
            "oracle-common",
        );
        let aws_cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .credentials_provider(creds)
            .region(aws_config::Region::new(cfg.region.clone()))
            .endpoint_url(cfg.endpoint.clone())
            .load()
            .await;
        let s3_cfg = aws_sdk_s3::config::Builder::from(&aws_cfg)
            // MinIO and many S3-compatibles need path-style addressing.
            .force_path_style(true)
            .build();
        let client = aws_sdk_s3::Client::from_conf(s3_cfg);
        Self {
            client,
            bucket: cfg.bucket.clone(),
        }
    }

    fn key(hash_hex: &str) -> String {
        format!("oracle-blobs/{hash_hex}")
    }
}

#[async_trait]
impl StorageBackend for S3Backend {
    async fn put_streaming(
        &self,
        mut body: ByteStream,
        max_bytes: u64,
        content_type: Option<String>,
    ) -> Result<StoredObject, StorageError> {
        // Buffer + hash. Single-shot upload is simplest for now; chunked / multipart
        // can be added when blobs exceed the AWS single-PUT limit (5 GiB) which is
        // also our default cap.
        let mut hasher = Sha256::new();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| StorageError::Io(e.to_string()))?;
            let new_size = buf.len() as u64 + chunk.len() as u64;
            if new_size > max_bytes {
                return Err(StorageError::TooLarge {
                    size: new_size,
                    max: max_bytes,
                });
            }
            hasher.update(&chunk);
            buf.extend_from_slice(&chunk);
        }
        let digest = hasher.finalize();
        let hash_hex = hex::encode(digest);
        let size_bytes = buf.len() as u64;
        let key = Self::key(&hash_hex);

        let mut put = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(buf.into());
        if let Some(ct) = content_type.as_deref() {
            put = put.content_type(ct);
        }
        put.send()
            .await
            .map_err(|e| StorageError::Io(format!("s3 put_object: {e}")))?;

        Ok(StoredObject {
            hash_hex,
            size_bytes,
            content_type,
        })
    }

    async fn get(&self, hash_hex: &str) -> Result<Bytes, StorageError> {
        let key = Self::key(hash_hex);
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| {
                let s = e.to_string();
                if s.contains("NoSuchKey") || s.contains("404") {
                    StorageError::NotFound(hash_hex.to_string())
                } else {
                    StorageError::Io(s)
                }
            })?;
        let aggregated = resp
            .body
            .collect()
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(aggregated.into_bytes())
    }

    async fn get_streaming(&self, hash_hex: &str) -> Result<ByteOutStream, StorageError> {
        let key = Self::key(hash_hex);
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| {
                let s = e.to_string();
                if s.contains("NoSuchKey") || s.contains("404") {
                    StorageError::NotFound(hash_hex.to_string())
                } else {
                    StorageError::Io(s)
                }
            })?;

        // aws-sdk-s3 returns a `ByteStream` (sdk-level type); we convert chunk-by-chunk.
        // Buffer the body fully — for our 5 GiB cap and current single-shot upload
        // semantics, full-body fetch is consistent with the upload path.
        let aggregated = resp
            .body
            .collect()
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;
        let bytes = aggregated.into_bytes();
        let stream = futures_util::stream::iter(vec![Ok(bytes)]);
        Ok(Box::pin(stream))
    }

    async fn stat(&self, hash_hex: &str) -> Result<Option<ObjectStat>, StorageError> {
        let key = Self::key(hash_hex);
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(resp) => {
                let size = resp.content_length().unwrap_or(0) as u64;
                let created_at = resp
                    .last_modified()
                    .map(|d| {
                        DateTime::<Utc>::from_timestamp(d.secs(), d.subsec_nanos())
                            .unwrap_or_else(Utc::now)
                    })
                    .unwrap_or_else(Utc::now);
                let content_type = resp.content_type().map(|s| s.to_string());
                Ok(Some(ObjectStat {
                    size_bytes: size,
                    content_type,
                    created_at,
                }))
            }
            Err(e) => {
                let s = e.to_string();
                if s.contains("NoSuchKey") || s.contains("404") || s.contains("NotFound") {
                    Ok(None)
                } else {
                    Err(StorageError::Io(s))
                }
            }
        }
    }
}

// =====================================================================
// Local filesystem backend (development)
// =====================================================================

#[derive(Clone)]
pub struct LocalFsBackend {
    root: PathBuf,
}

impl LocalFsBackend {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path_for(&self, hash_hex: &str) -> PathBuf {
        // Two-character shard prefix to avoid one giant directory.
        let shard = &hash_hex[0..2];
        self.root.join(shard).join(hash_hex)
    }
}

#[async_trait]
impl StorageBackend for LocalFsBackend {
    async fn put_streaming(
        &self,
        mut body: ByteStream,
        max_bytes: u64,
        content_type: Option<String>,
    ) -> Result<StoredObject, StorageError> {
        let mut hasher = Sha256::new();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| StorageError::Io(e.to_string()))?;
            let new_size = buf.len() as u64 + chunk.len() as u64;
            if new_size > max_bytes {
                return Err(StorageError::TooLarge {
                    size: new_size,
                    max: max_bytes,
                });
            }
            hasher.update(&chunk);
            buf.extend_from_slice(&chunk);
        }
        let digest = hasher.finalize();
        let hash_hex = hex::encode(digest);
        let size_bytes = buf.len() as u64;
        let path = self.path_for(&hash_hex);

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| StorageError::Io(e.to_string()))?;
        }

        // Write to a temp file in the same dir then rename to ensure atomicity —
        // a half-written file with the right hash would be a confusing crash artefact.
        let tmp_path = path.with_extension("tmp");
        let mut f = tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;
        f.write_all(&buf)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;
        f.flush()
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;
        drop(f);
        tokio::fs::rename(&tmp_path, &path)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;

        Ok(StoredObject {
            hash_hex,
            size_bytes,
            content_type,
        })
    }

    async fn get(&self, hash_hex: &str) -> Result<Bytes, StorageError> {
        let path = self.path_for(hash_hex);
        match tokio::fs::File::open(&path).await {
            Ok(mut f) => {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)
                    .await
                    .map_err(|e| StorageError::Io(e.to_string()))?;
                Ok(Bytes::from(buf))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StorageError::NotFound(hash_hex.to_string()))
            }
            Err(e) => Err(StorageError::Io(e.to_string())),
        }
    }

    async fn get_streaming(&self, hash_hex: &str) -> Result<ByteOutStream, StorageError> {
        let bytes = self.get(hash_hex).await?;
        let stream = futures_util::stream::iter(vec![Ok(bytes)]);
        Ok(Box::pin(stream))
    }

    async fn stat(&self, hash_hex: &str) -> Result<Option<ObjectStat>, StorageError> {
        let path = self.path_for(hash_hex);
        match tokio::fs::metadata(&path).await {
            Ok(meta) => {
                let created_at = meta
                    .modified()
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_else(|_| Utc::now());
                Ok(Some(ObjectStat {
                    size_bytes: meta.len(),
                    content_type: None,
                    created_at,
                }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StorageError::Io(e.to_string())),
        }
    }
}

// =====================================================================
// Factory
// =====================================================================

/// Build the storage backend selected by [`OracleConfig::registry_backend`].
///
/// * `Postgres` requires the caller to supply a connected pool.
/// * `S3` requires the [`S3Config`] populated by `OracleConfig::from_env`.
/// * `Local` writes under `<state_dir>/blobs/`.
pub async fn make_backend(
    backend: crate::config::RegistryBackend,
    pg_pool: Option<Pool>,
    s3_cfg: Option<&S3Config>,
    local_root: Option<PathBuf>,
) -> Result<Arc<dyn StorageBackend>, StorageError> {
    use crate::config::RegistryBackend;
    match backend {
        RegistryBackend::Postgres => {
            let pool = pg_pool.ok_or_else(|| {
                StorageError::Io("Postgres backend requested but no pool provided".into())
            })?;
            Ok(Arc::new(PostgresBackend::new(pool)))
        }
        RegistryBackend::S3 => {
            let cfg = s3_cfg.ok_or_else(|| {
                StorageError::Io("S3 backend requested but no S3 config provided".into())
            })?;
            Ok(Arc::new(S3Backend::new(cfg).await))
        }
        RegistryBackend::Local => {
            let root = local_root.unwrap_or_else(|| PathBuf::from("/var/lib/oracle/blobs"));
            tokio::fs::create_dir_all(&root)
                .await
                .map_err(|e| StorageError::Io(e.to_string()))?;
            Ok(Arc::new(LocalFsBackend::new(root)))
        }
    }
}

#[cfg(test)]
mod tests {
    use futures_util::stream;
    use tempfile::TempDir;

    use super::*;

    fn stream_of(parts: Vec<&'static [u8]>) -> ByteStream {
        Box::pin(stream::iter(
            parts
                .into_iter()
                .map(|p| Ok::<_, std::io::Error>(Bytes::from_static(p)))
                .collect::<Vec<_>>(),
        ))
    }

    #[tokio::test]
    async fn local_backend_round_trip() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalFsBackend::new(tmp.path().to_path_buf());

        let body = stream_of(vec![b"hello, ", b"world"]);
        let stored = backend.put_streaming(body, 1024, None).await.unwrap();
        assert_eq!(stored.size_bytes, 12);
        // SHA256("hello, world") = 09ca7e4eaa6e8ae9c7d2615f96fdc40ad6e2f3d29c39b8f9...
        assert_eq!(stored.hash_hex.len(), 64);

        let bytes = backend.get(&stored.hash_hex).await.unwrap();
        assert_eq!(&bytes[..], b"hello, world");

        let stat = backend.stat(&stored.hash_hex).await.unwrap().unwrap();
        assert_eq!(stat.size_bytes, 12);
    }

    #[tokio::test]
    async fn local_backend_rejects_oversize() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalFsBackend::new(tmp.path().to_path_buf());
        let body = stream_of(vec![b"abcdef", b"ghijkl"]);
        let err = backend.put_streaming(body, 6, None).await.unwrap_err();
        assert!(matches!(err, StorageError::TooLarge { size: 12, max: 6 }));
    }

    #[tokio::test]
    async fn local_backend_not_found_returns_error() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalFsBackend::new(tmp.path().to_path_buf());
        let err = backend.get("00".repeat(32).as_str()).await.unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
    }

    #[tokio::test]
    async fn local_backend_stat_returns_none_for_missing() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalFsBackend::new(tmp.path().to_path_buf());
        let stat = backend.stat("00".repeat(32).as_str()).await.unwrap();
        assert!(stat.is_none());
    }

    #[tokio::test]
    async fn local_backend_streaming_get_returns_full_body() {
        let tmp = TempDir::new().unwrap();
        let backend = LocalFsBackend::new(tmp.path().to_path_buf());
        let body = stream_of(vec![b"the quick brown fox jumps"]);
        let stored = backend.put_streaming(body, 1024, None).await.unwrap();

        let mut s = backend.get_streaming(&stored.hash_hex).await.unwrap();
        let mut all = Vec::new();
        while let Some(chunk) = s.next().await {
            all.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(all.as_slice(), b"the quick brown fox jumps");
    }

    #[tokio::test]
    async fn factory_local_creates_root() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        let backend = make_backend(
            crate::config::RegistryBackend::Local,
            None,
            None,
            Some(nested.clone()),
        )
        .await
        .unwrap();
        // Round-trip a tiny blob to confirm the factory wired things up.
        let body = stream_of(vec![b"hi"]);
        let stored = backend.put_streaming(body, 16, None).await.unwrap();
        assert_eq!(stored.size_bytes, 2);
        assert!(nested.exists());
    }

    #[tokio::test]
    async fn factory_postgres_requires_pool() {
        let res = make_backend(crate::config::RegistryBackend::Postgres, None, None, None).await;
        match res {
            Ok(_) => panic!("expected error"),
            Err(StorageError::Io(s)) => assert!(s.contains("Postgres backend"), "got: {s}"),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[tokio::test]
    async fn factory_s3_requires_config() {
        let res = make_backend(crate::config::RegistryBackend::S3, None, None, None).await;
        match res {
            Ok(_) => panic!("expected error"),
            Err(StorageError::Io(s)) => assert!(s.contains("S3 backend"), "got: {s}"),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
}
