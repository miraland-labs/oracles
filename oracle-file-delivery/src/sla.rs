//! `FileDeliverySla` for the file-delivery family.

use serde::{Deserialize, Serialize};

use crate::PROFILE_ID;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDeliverySla {
    pub version: u32,
    pub profile_id: String,
    pub expected_size_bytes_min: u64,
    pub expected_size_bytes_max: u64,
    #[serde(default)]
    pub expected_mime: Option<String>,
    #[serde(default)]
    pub expected_extension: Option<String>,
    #[serde(default)]
    pub attestor_pubkey: Option<String>,
}

impl FileDeliverySla {
    pub fn profile_id_matches(&self) -> bool {
        self.profile_id == PROFILE_ID
    }
}
