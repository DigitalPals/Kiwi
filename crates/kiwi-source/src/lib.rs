//! Public, unauthenticated AltStore-format sources: fetching, parsing, release selection, and
//! validated IPA downloads.
//!
//! Ported from `plume_source` in the Plume Impactor fork (same author, MIT), without its
//! account scoping and JSON file stores: Kiwi persists [`Source`] in SQLite.
mod artifact;
mod catalog;
mod network;
mod source;
mod urls;

pub use artifact::*;
pub use catalog::*;
pub use network::*;
pub use source::*;
pub use tokio_util::sync::CancellationToken;
pub use url::Url;
pub use urls::*;

#[derive(Debug, thiserror::Error)]
#[error("{stage}: {detail}")]
pub struct Error {
    pub stage: &'static str,
    pub detail: String,
}

impl Error {
    pub fn new(stage: &'static str, detail: impl std::fmt::Display) -> Self {
        Self {
            stage,
            detail: detail.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
