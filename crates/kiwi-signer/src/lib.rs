//! Signing identities, provisioning profiles and IPA re-signing.
//!
//! Two backends sign: on macOS the login keychain and Apple's `codesign`
//! ([`Backend::Keychain`]), elsewhere an uploaded `.p12` identity and rcodesign's
//! library, `apple-codesign` ([`Backend::Files`]). Parsing, matching, extraction,
//! preparation and the dropped-entitlements report are pure Rust and shared by both.

mod archive;
mod bundle;
mod cert;
mod codesign;
mod der;
mod entitlements;
mod files;
mod identity;
mod macho;
#[cfg(not(target_os = "macos"))]
mod p12;
mod package;
mod process;
mod profile;
#[cfg(not(target_os = "macos"))]
mod rcodesign;
mod sign;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod testutil;

use std::{io, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use files::{MAX_P12_BYTES, MAX_PROFILE_BYTES, SigningDir};
pub use identity::{Identity, list_identities};
pub use profile::{
    Profile, ProfileMatch, default_profile_dirs, load_profiles, parse_profile,
    profile_entitlement_keys, select_profile,
};
pub use sign::{SignReport, SignRequest, SignStep, sign_ipa, sign_ipa_with_files};

/// How this host signs iOS apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// The login keychain and Apple's `codesign` (macOS).
    Keychain,
    /// An uploaded `.p12` identity and rcodesign's library (every other system).
    Files,
}

impl Backend {
    /// The backend of the system Kiwi was built for.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Keychain
        } else {
            Self::Files
        }
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::current()
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors carry user-readable messages; they are shown in the app as is.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Signing was cancelled")]
    Cancelled,
    /// The operation belongs to the other [`Backend`].
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    InvalidIpa(String),
    #[error("{0}")]
    InvalidProfile(String),
    /// An uploaded `.p12` identity (or its password) is unusable.
    #[error("{0}")]
    InvalidIdentity(String),
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

/// Fails unless this host signs with the keychain and `codesign`.
pub(crate) fn require_macos() -> Result<()> {
    if Backend::current() == Backend::Keychain {
        Ok(())
    } else {
        Err(Error::Unsupported(
            "The keychain and codesign are only available on macOS; Kiwi signs with an uploaded identity here",
        ))
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

    #[test]
    fn backend_follows_the_platform() {
        let expected = if cfg!(target_os = "macos") {
            "\"keychain\""
        } else {
            "\"files\""
        };
        assert_eq!(
            serde_json::to_string(&Backend::current()).unwrap(),
            expected
        );
        assert_eq!(Backend::default(), Backend::current());
    }
}
