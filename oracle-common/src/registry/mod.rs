//! Seller delivery registration HTTP API + storage backends. Routes are mounted onto
//! the binary's main Axum router, or run as a sibling `oracle-registry` service.
//!
//! Modules:
//! - [`api`]: route handlers (Task 5.3).
//! - [`auth`]: HMAC challenge + bearer token middleware (Task 5.1, 5.2).
//! - [`storage`]: `StorageBackend` trait + Postgres / S3 / local impls (Task 4).

pub mod api;
pub mod auth;
pub mod storage;
