//! Signing identities, provisioning profiles and IPA re-signing with Apple's `codesign`.
//!
//! Parsing, matching and preparation are pure Rust and work everywhere; listing
//! identities and signing need macOS and return [`Error::Unsupported`] elsewhere.

mod archive;
mod bundle;
mod cert;
mod der;
mod entitlements;
mod identity;
mod macho;
mod process;
mod profile;
mod sign;
#[cfg(test)]
mod testutil;

use std::{io, path::Path};

use sha2::{Digest, Sha256};

pub use identity::{Identity, list_identities};
pub use profile::{
    Profile, ProfileMatch, default_profile_dirs, load_profiles, parse_profile,
    profile_entitlement_keys, select_profile,
};
pub use sign::{SignReport, SignRequest, SignStep, sign_ipa};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors carry user-readable messages; they are shown in the app as is.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Signing was cancelled")]
    Cancelled,
    #[error("Signing is only available on macOS")]
    Unsupported,
    #[error("{0}")]
    InvalidIpa(String),
    #[error("{0}")]
    InvalidProfile(String),
    #[error("{0}")]
    NoProfile(String),
    #[error("{0}")]
    InvalidRequest(String),
    /// An external tool (`security`, `codesign`, `zip`) failed.
    #[error("{0}")]
    Tool(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
}

impl Error {
    pub(crate) fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub(crate) fn io_path(action: &str, path: &Path) -> impl FnOnce(io::Error) -> Self + use<> {
        let context = format!("{action} {}", path.display());
        move |source| Self::io(context, source)
    }
}

/// Bundle id Kiwi installs an app as: `"{original}.{team_id}"`.
pub fn signed_bundle_id(original: &str, team_id: &str) -> String {
    format!("{original}.{team_id}")
}

/// URL scheme Kiwi injects to open an app: `"kiwi-"` + 12 hex digits of SHA-256.
pub fn url_scheme_for(signed_bundle_id: &str) -> String {
    let digest = Sha256::digest(signed_bundle_id.as_bytes());
    format!("kiwi-{}", &hex::encode(digest)[..12])
}

/// Deepest array/dictionary nesting accepted in a plist from an IPA; deserializing
/// recurses per level, so a hostile file could otherwise overflow the stack and abort the
/// server (which would then retry the build and crash again).
const MAX_PLIST_DEPTH: usize = 64;
/// Most values accepted in one plist (binary plists can reference one object many times).
const MAX_PLIST_VALUES: usize = 1_000_000;
/// Largest Info.plist or entitlements blob read from an IPA.
pub(crate) const MAX_PLIST_BYTES: u64 = 4 * 1024 * 1024;

/// Checks a plist's nesting and size with the (non-recursive) event reader before it is
/// deserialized.
pub(crate) fn check_plist(bytes: &[u8]) -> Result<(), String> {
    use plist::stream::{Event, Reader};
    let mut depth = 0usize;
    for (count, event) in Reader::new(io::Cursor::new(bytes)).enumerate() {
        if count >= MAX_PLIST_VALUES {
            return Err(format!("more than {MAX_PLIST_VALUES} values"));
        }
        match event.map_err(|e| e.to_string())? {
            Event::StartArray(_) | Event::StartDictionary(_) => {
                depth += 1;
                if depth > MAX_PLIST_DEPTH {
                    return Err(format!("nested more than {MAX_PLIST_DEPTH} levels deep"));
                }
            }
            Event::EndCollection => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn require_macos() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(Error::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_id_and_scheme() {
        let id = signed_bundle_id("com.google.ios.youtube", "A1B2C3D4E5");
        assert_eq!(id, "com.google.ios.youtube.A1B2C3D4E5");
        assert_eq!(url_scheme_for(&id), "kiwi-bc4f7942bcd3");
        assert_ne!(
            url_scheme_for(&id),
            url_scheme_for("com.example.A1B2C3D4E5")
        );
    }
}
