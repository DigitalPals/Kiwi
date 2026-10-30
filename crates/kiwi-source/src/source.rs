use crate::{Catalog, Url};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable source id: SHA-256 of the canonical URL.
pub fn source_key(url: &Url) -> String {
    hex::encode(Sha256::digest(url.as_str().as_bytes()))
}

/// A fetched source with its conditional-request state. The server stores it as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub key: String,
    pub url: Url,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub catalog: Catalog,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub last_success: Option<DateTime<Utc>>,
    #[serde(default)]
    pub error: Option<String>,
}
fn yes() -> bool {
    true
}
