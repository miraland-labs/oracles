//! Per-binary configuration loaded from environment variables.
//!
//! See [design.md §Operational Architecture](../../../.kiro/specs/multi-category-oracle-architecture/design.md#operational-architecture-ubuntu-2404--systemd)
//! for the canonical list. Every variable below has either a sensible default or a hard
//! requirement; missing required variables produce a clear `ConfigError` at startup.

use std::{env, str::FromStr, sync::Arc};

use solana_sdk::{
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair},
    signer::Signer,
};

use crate::error::OracleError;

/// Storage backend selector for the registration HTTP API.
///
/// One of `postgres` / `s3` / `local`. The variant is set via
/// `ORACLE_REGISTRY_BACKEND`; an unset or invalid value causes startup to fail (see
/// [design.md §Storage Strategy for Blobs](../../../.kiro/specs/multi-category-oracle-architecture/design.md#storage-strategy-for-blobs)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryBackend {
    Postgres,
    S3,
    Local,
}

impl FromStr for RegistryBackend {
    type Err = ConfigError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "postgres" => Ok(Self::Postgres),
            "s3" => Ok(Self::S3),
            "local" => Ok(Self::Local),
            other => Err(ConfigError::InvalidEnum {
                var: "ORACLE_REGISTRY_BACKEND".into(),
                got: other.to_string(),
                allowed: "postgres|s3|local".into(),
            }),
        }
    }
}

/// Errors raised while loading [`OracleConfig`] from the environment.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("required env var {0} is not set")]
    Missing(String),
    #[error("env var {var} is not valid: {message}")]
    Invalid { var: String, message: String },
    #[error("env var {var} value '{got}' is not one of: {allowed}")]
    InvalidEnum {
        var: String,
        got: String,
        allowed: String,
    },
    #[error("env var {var} expects {expected} but got {got}")]
    BadType {
        var: String,
        expected: String,
        got: String,
    },
    #[error("failed to read keypair from {path}: {message}")]
    BadKeypair { path: String, message: String },
}

impl From<ConfigError> for OracleError {
    fn from(value: ConfigError) -> Self {
        OracleError::Settlement(value.to_string())
    }
}

/// Optional S3-compatible blob backend configuration. Required when
/// `ORACLE_REGISTRY_BACKEND=s3`, ignored otherwise.
#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

/// All env-driven configuration for an oracle binary.
#[derive(Clone)]
pub struct OracleConfig {
    // Solana
    pub solana_rpc_url: String,
    pub solana_ws_url: String,
    pub oracle_keypair: Arc<Keypair>,
    pub escrow_program_id: Pubkey,

    // HTTP server
    pub bind_addr: String,

    // Pipeline timing
    pub evaluation_timeout_ms: u64,

    // Evidence registry (acts as a fall-through fetch source for direct GETs;
    // the registration HTTP routes are mounted on the same Axum router).
    pub evidence_registry_urls: Vec<String>,
    pub evidence_registry_auth_header: Option<String>,
    pub evidence_fetch_max_retries: u32,
    pub evidence_fetch_retry_base_ms: u64,

    // Postgres ledger
    pub database_url: Option<String>,

    // Registration HTTP / storage backend
    pub registry_backend: RegistryBackend,
    pub registry_max_bytea_bytes: u64,
    pub registry_max_blob_bytes: u64,
    pub s3: Option<S3Config>,

    // Operator manual-evaluate auth + rate limit
    pub operator_token_sha256: Option<[u8; 32]>,
    pub allow_unauthenticated_manual_evaluate: bool,
    pub cors_allowed_origins: Vec<String>,
    pub manual_evaluate_rate_limit: usize,
    pub manual_evaluate_rate_window_ms: u64,

    // Evaluation policy
    pub strict_profile: bool,
    pub dead_letter_max_attempts: u32,
    pub job_channel_capacity: usize,

    // Chain monitor hardening
    pub require_event_match: bool,
    pub backfill_lookback_signatures: usize,

    // Active Guardian: retry + fail-closed reject
    /// Initial delay (seconds) before the first retry of a failed pipeline job.
    pub guardian_retry_initial_delay_sec: u64,
    /// Maximum delay cap (seconds) for exponential backoff between retries.
    pub guardian_retry_max_delay_sec: u64,
    /// Maximum number of retry attempts before giving up and issuing a reject.
    pub guardian_max_retry_attempts: u32,
    /// Safety margin (seconds) before `expires_at` at which the oracle issues a
    /// protective REJECT if evaluation hasn't completed. Must be strictly larger
    /// than the on-chain `Config.delivery_cutoff_seconds` (default 300s).
    pub guardian_reject_safety_margin_sec: i64,
}

impl OracleConfig {
    pub fn oracle_pubkey(&self) -> Pubkey {
        self.oracle_keypair.pubkey()
    }

    /// Load from `std::env`. Returns `ConfigError` for any missing required value.
    pub fn from_env() -> Result<Self, ConfigError> {
        let solana_rpc_url =
            env::var("SOLANA_RPC_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".into());
        let solana_ws_url =
            env::var("SOLANA_WS_URL").unwrap_or_else(|_| "wss://api.devnet.solana.com".into());

        let keypair_path = env::var("ORACLE_KEYPAIR_PATH")
            .map_err(|_| ConfigError::Missing("ORACLE_KEYPAIR_PATH".into()))?;
        let oracle_keypair =
            read_keypair_file(&keypair_path).map_err(|e| ConfigError::BadKeypair {
                path: keypair_path.clone(),
                message: e.to_string(),
            })?;

        let escrow_program_id = env::var("ESCROW_PROGRAM_ID")
            .ok()
            .map(|s| {
                Pubkey::from_str(&s).map_err(|e| ConfigError::Invalid {
                    var: "ESCROW_PROGRAM_ID".into(),
                    message: e.to_string(),
                })
            })
            .transpose()?
            .unwrap_or(sla_escrow_api::ID);

        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:4020".into());

        let evaluation_timeout_ms = parse_or("EVALUATION_TIMEOUT_MS", 30_000u64)?;

        let evidence_registry_urls =
            parse_url_list("EVIDENCE_REGISTRY_URLS", "EVIDENCE_REGISTRY_URL")?;

        let evidence_registry_auth_header = env::var("EVIDENCE_REGISTRY_AUTH_HEADER")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let evidence_fetch_max_retries = parse_or("EVIDENCE_FETCH_MAX_RETRIES", 3u32)?;
        let evidence_fetch_retry_base_ms = parse_or("EVIDENCE_FETCH_RETRY_BASE_MS", 200u64)?;

        let database_url = env::var("DATABASE_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        // Registry backend MUST be explicitly chosen (per Requirement 17.4 — we refuse
        // to start with an unset or invalid value rather than silently picking a
        // default that hides a misconfiguration).
        let registry_backend = match env::var("ORACLE_REGISTRY_BACKEND") {
            Ok(s) if !s.trim().is_empty() => RegistryBackend::from_str(&s)?,
            _ => return Err(ConfigError::Missing("ORACLE_REGISTRY_BACKEND".into())),
        };

        let registry_max_bytea_bytes =
            parse_or("ORACLE_REGISTRY_MAX_BYTEA_BYTES", 4 * 1024 * 1024u64)?;
        let registry_max_blob_bytes =
            parse_or("ORACLE_REGISTRY_MAX_BLOB_BYTES", 5 * 1024 * 1024 * 1024u64)?;

        let s3 = if registry_backend == RegistryBackend::S3 {
            Some(S3Config {
                endpoint: require("ORACLE_REGISTRY_S3_ENDPOINT")?,
                bucket: require("ORACLE_REGISTRY_S3_BUCKET")?,
                access_key: require("ORACLE_REGISTRY_S3_ACCESS_KEY")?,
                secret_key: require("ORACLE_REGISTRY_S3_SECRET_KEY")?,
                region: env::var("ORACLE_REGISTRY_S3_REGION")
                    .unwrap_or_else(|_| "us-east-1".into()),
            })
        } else {
            None
        };

        let operator_token = env::var("ORACLE_OPERATOR_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let operator_token_sha256 = match env::var("ORACLE_OPERATOR_TOKEN_SHA256") {
            Ok(hex) if !hex.trim().is_empty() => {
                let decoded = hex::decode(hex.trim()).map_err(|e| ConfigError::Invalid {
                    var: "ORACLE_OPERATOR_TOKEN_SHA256".into(),
                    message: e.to_string(),
                })?;
                let arr: [u8; 32] = decoded.try_into().map_err(|_| ConfigError::Invalid {
                    var: "ORACLE_OPERATOR_TOKEN_SHA256".into(),
                    message: "must decode to 32 bytes".into(),
                })?;
                Some(arr)
            }
            _ => operator_token.map(|token| sha256_array(token.as_bytes())),
        };

        let allow_unauthenticated_manual_evaluate =
            parse_bool("ORACLE_ALLOW_UNAUTHENTICATED_MANUAL_EVALUATE", false)?;

        let cors_allowed_origins = env::var("ORACLE_CORS_ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();

        let manual_evaluate_rate_limit = parse_or("ORACLE_MANUAL_EVALUATE_RATE_LIMIT", 30usize)?;
        let manual_evaluate_rate_window_ms =
            parse_or("ORACLE_MANUAL_EVALUATE_RATE_WINDOW_MS", 60_000u64)?;
        let strict_profile = parse_bool("ORACLE_STRICT_PROFILE", true)?;
        let dead_letter_max_attempts = parse_or("ORACLE_DEAD_LETTER_MAX_ATTEMPTS", 5u32)?;
        let job_channel_capacity = parse_or("ORACLE_JOB_CHANNEL_CAPACITY", 256usize)?;
        let require_event_match = parse_bool("ORACLE_REQUIRE_EVENT_MATCH", false)?;
        let backfill_lookback_signatures =
            parse_or("ORACLE_BACKFILL_LOOKBACK_SIGNATURES", 2_000usize)?;

        Ok(Self {
            solana_rpc_url,
            solana_ws_url,
            oracle_keypair: Arc::new(oracle_keypair),
            escrow_program_id,
            bind_addr,
            evaluation_timeout_ms,
            evidence_registry_urls,
            evidence_registry_auth_header,
            evidence_fetch_max_retries,
            evidence_fetch_retry_base_ms,
            database_url,
            registry_backend,
            registry_max_bytea_bytes,
            registry_max_blob_bytes,
            s3,
            operator_token_sha256,
            allow_unauthenticated_manual_evaluate,
            cors_allowed_origins,
            manual_evaluate_rate_limit,
            manual_evaluate_rate_window_ms,
            strict_profile,
            dead_letter_max_attempts,
            job_channel_capacity,
            require_event_match,
            backfill_lookback_signatures,
            guardian_retry_initial_delay_sec: env::var("ORACLE_RETRY_INITIAL_DELAY_SEC")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
            guardian_retry_max_delay_sec: env::var("ORACLE_RETRY_MAX_DELAY_SEC")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(120),
            guardian_max_retry_attempts: env::var("ORACLE_MAX_RETRY_ATTEMPTS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            guardian_reject_safety_margin_sec: env::var("ORACLE_REJECT_SAFETY_MARGIN_SEC")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(600),
        })
    }
}

fn require(var: &str) -> Result<String, ConfigError> {
    env::var(var)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ConfigError::Missing(var.to_string()))
}

fn parse_or<T: FromStr>(var: &str, default: T) -> Result<T, ConfigError>
where
    T::Err: std::fmt::Display,
{
    match env::var(var) {
        Ok(s) if !s.trim().is_empty() => {
            s.trim().parse().map_err(|e: T::Err| ConfigError::BadType {
                var: var.to_string(),
                expected: std::any::type_name::<T>().to_string(),
                got: e.to_string(),
            })
        }
        _ => Ok(default),
    }
}

fn parse_bool(var: &str, default: bool) -> Result<bool, ConfigError> {
    match env::var(var) {
        Ok(s) if !s.trim().is_empty() => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            other => Err(ConfigError::InvalidEnum {
                var: var.to_string(),
                got: other.to_string(),
                allowed: "1|true|yes|on / 0|false|no|off".into(),
            }),
        },
        _ => Ok(default),
    }
}

/// Parse `EVIDENCE_REGISTRY_URLS` (comma-separated mirror list) with `EVIDENCE_REGISTRY_URL`
/// as a single-URL fallback. Empty list defaults to `http://localhost:4021`.
fn parse_url_list(plural_var: &str, singular_var: &str) -> Result<Vec<String>, ConfigError> {
    if let Ok(s) = env::var(plural_var) {
        let parts: Vec<String> = s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        if !parts.is_empty() {
            return Ok(parts);
        }
    }
    Ok(vec![
        env::var(singular_var).unwrap_or_else(|_| "http://localhost:4021".into())
    ])
}

fn sha256_array(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::*;

    /// Env mutex — Rust runs tests in parallel and `std::env` is process-global.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_clean_env<F: FnOnce()>(f: F) {
        let _g = env_lock().lock().unwrap();
        // Clear every var our config touches.
        for k in [
            "SOLANA_RPC_URL",
            "SOLANA_WS_URL",
            "ORACLE_KEYPAIR_PATH",
            "ESCROW_PROGRAM_ID",
            "BIND_ADDR",
            "EVALUATION_TIMEOUT_MS",
            "EVIDENCE_REGISTRY_URL",
            "EVIDENCE_REGISTRY_URLS",
            "EVIDENCE_REGISTRY_AUTH_HEADER",
            "EVIDENCE_FETCH_MAX_RETRIES",
            "EVIDENCE_FETCH_RETRY_BASE_MS",
            "DATABASE_URL",
            "ORACLE_REGISTRY_BACKEND",
            "ORACLE_REGISTRY_MAX_BYTEA_BYTES",
            "ORACLE_REGISTRY_MAX_BLOB_BYTES",
            "ORACLE_REGISTRY_S3_ENDPOINT",
            "ORACLE_REGISTRY_S3_BUCKET",
            "ORACLE_REGISTRY_S3_ACCESS_KEY",
            "ORACLE_REGISTRY_S3_SECRET_KEY",
            "ORACLE_REGISTRY_S3_REGION",
            "ORACLE_OPERATOR_TOKEN",
            "ORACLE_OPERATOR_TOKEN_SHA256",
            "ORACLE_ALLOW_UNAUTHENTICATED_MANUAL_EVALUATE",
            "ORACLE_CORS_ALLOWED_ORIGINS",
            "ORACLE_MANUAL_EVALUATE_RATE_LIMIT",
            "ORACLE_MANUAL_EVALUATE_RATE_WINDOW_MS",
            "ORACLE_STRICT_PROFILE",
            "ORACLE_DEAD_LETTER_MAX_ATTEMPTS",
            "ORACLE_JOB_CHANNEL_CAPACITY",
            "ORACLE_REQUIRE_EVENT_MATCH",
            "ORACLE_BACKFILL_LOOKBACK_SIGNATURES",
        ] {
            // SAFETY: env mutation is single-threaded under `env_lock`.
            unsafe {
                env::remove_var(k);
            }
        }
        f();
    }

    fn write_keypair() -> tempfile::NamedTempFile {
        let kp = Keypair::new();
        let bytes: Vec<u8> = kp.to_bytes().to_vec();
        let json = serde_json::to_string(&bytes).unwrap();
        let mut f = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, json.as_bytes()).unwrap();
        f
    }

    #[test]
    fn registry_backend_required() {
        with_clean_env(|| {
            let kp = write_keypair();
            unsafe {
                env::set_var("ORACLE_KEYPAIR_PATH", kp.path());
            }
            let res = OracleConfig::from_env();
            assert!(
                res.is_err(),
                "expected ORACLE_REGISTRY_BACKEND to be required"
            );
            match res {
                Err(ConfigError::Missing(ref v)) if v == "ORACLE_REGISTRY_BACKEND" => {}
                Err(other) => panic!("unexpected error: {other}"),
                Ok(_) => panic!("expected error"),
            }
        });
    }

    #[test]
    fn registry_backend_invalid_rejected() {
        with_clean_env(|| {
            let kp = write_keypair();
            unsafe {
                env::set_var("ORACLE_KEYPAIR_PATH", kp.path());
                env::set_var("ORACLE_REGISTRY_BACKEND", "carrier-pigeon");
            }
            let res = OracleConfig::from_env();
            match res {
                Err(ConfigError::InvalidEnum { ref var, .. })
                    if var == "ORACLE_REGISTRY_BACKEND" => {}
                Err(other) => panic!("unexpected error: {other}"),
                Ok(_) => panic!("expected error"),
            }
        });
    }

    #[test]
    fn s3_backend_requires_credentials() {
        with_clean_env(|| {
            let kp = write_keypair();
            unsafe {
                env::set_var("ORACLE_KEYPAIR_PATH", kp.path());
                env::set_var("ORACLE_REGISTRY_BACKEND", "s3");
            }
            let res = OracleConfig::from_env();
            match res {
                Err(ConfigError::Missing(ref v)) if v == "ORACLE_REGISTRY_S3_ENDPOINT" => {}
                Err(other) => panic!("unexpected error: {other}"),
                Ok(_) => panic!("expected error"),
            }
        });
    }

    #[test]
    fn happy_path_postgres_backend() {
        with_clean_env(|| {
            let kp = write_keypair();
            unsafe {
                env::set_var("ORACLE_KEYPAIR_PATH", kp.path());
                env::set_var("ORACLE_REGISTRY_BACKEND", "postgres");
            }
            let cfg = OracleConfig::from_env().expect("postgres-backend config should load");
            assert_eq!(cfg.registry_backend, RegistryBackend::Postgres);
            assert!(cfg.s3.is_none());
            assert_eq!(cfg.evidence_registry_urls, vec!["http://localhost:4021"]);
            assert!(cfg.strict_profile, "default ORACLE_STRICT_PROFILE is true");
            assert_eq!(cfg.evaluation_timeout_ms, 30_000);
            assert_eq!(cfg.evidence_fetch_max_retries, 3);
            assert_eq!(cfg.dead_letter_max_attempts, 5);
            assert_eq!(cfg.job_channel_capacity, 256);
        });
    }

    #[test]
    fn evidence_registry_urls_comma_list() {
        with_clean_env(|| {
            let kp = write_keypair();
            unsafe {
                env::set_var("ORACLE_KEYPAIR_PATH", kp.path());
                env::set_var("ORACLE_REGISTRY_BACKEND", "local");
                env::set_var(
                    "EVIDENCE_REGISTRY_URLS",
                    "https://primary.example.com/r,https://mirror.example.com/r",
                );
            }
            let cfg = OracleConfig::from_env().expect("local-backend config should load");
            assert_eq!(
                cfg.evidence_registry_urls,
                vec![
                    "https://primary.example.com/r".to_string(),
                    "https://mirror.example.com/r".to_string()
                ]
            );
        });
    }

    #[test]
    fn operator_token_sha256_hex_parsed() {
        with_clean_env(|| {
            let kp = write_keypair();
            // 32-byte hex (sha256 of "secret")
            let hexed = "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b";
            unsafe {
                env::set_var("ORACLE_KEYPAIR_PATH", kp.path());
                env::set_var("ORACLE_REGISTRY_BACKEND", "postgres");
                env::set_var("ORACLE_OPERATOR_TOKEN_SHA256", hexed);
            }
            let cfg = OracleConfig::from_env().expect("config should load");
            let want = hex::decode(hexed).unwrap();
            assert_eq!(cfg.operator_token_sha256.unwrap()[..], want[..]);
        });
    }

    #[test]
    fn operator_token_falls_back_to_plain() {
        with_clean_env(|| {
            let kp = write_keypair();
            unsafe {
                env::set_var("ORACLE_KEYPAIR_PATH", kp.path());
                env::set_var("ORACLE_REGISTRY_BACKEND", "postgres");
                env::set_var("ORACLE_OPERATOR_TOKEN", "open-sesame");
            }
            let cfg = OracleConfig::from_env().expect("config should load");
            let expected = sha256_array(b"open-sesame");
            assert_eq!(cfg.operator_token_sha256.unwrap(), expected);
        });
    }

    #[test]
    fn keypair_path_required() {
        with_clean_env(|| {
            unsafe {
                env::set_var("ORACLE_REGISTRY_BACKEND", "postgres");
            }
            let res = OracleConfig::from_env();
            match res {
                Err(ConfigError::Missing(ref v)) if v == "ORACLE_KEYPAIR_PATH" => {}
                Err(other) => panic!("unexpected error: {other}"),
                Ok(_) => panic!("expected error"),
            }
        });
    }

    #[test]
    fn cors_origins_split_and_trim() {
        with_clean_env(|| {
            let kp = write_keypair();
            unsafe {
                env::set_var("ORACLE_KEYPAIR_PATH", kp.path());
                env::set_var("ORACLE_REGISTRY_BACKEND", "postgres");
                env::set_var(
                    "ORACLE_CORS_ALLOWED_ORIGINS",
                    " https://a.example.com , https://b.example.com ",
                );
            }
            let cfg = OracleConfig::from_env().expect("config should load");
            assert_eq!(
                cfg.cors_allowed_origins,
                vec![
                    "https://a.example.com".to_string(),
                    "https://b.example.com".to_string()
                ]
            );
        });
    }
}
