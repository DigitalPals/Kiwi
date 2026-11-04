use crate::{Catalog, Url, Validators};
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
    /// Set when the "source" is an IPA link or a download page rather than an AltStore
    /// source; its catalog is then built from the IPA. `etag` and `last_modified` belong to
    /// the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<IpaLink>,
}

/// Where an app added by link comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpaLink {
    /// The IPA the current release was downloaded from.
    pub ipa: Url,
    /// The download page it was found on, checked for a new IPA link on refresh.
    #[serde(default)]
    pub page: Option<Url>,
    /// The IPA's validators when it was downloaded (from `HEAD`), to notice a new file at
    /// the same URL.
    #[serde(default)]
    pub validators: Validators,
}
fn yes() -> bool {
    true
}
