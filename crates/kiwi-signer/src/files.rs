//! Kiwi's own signing files in `<data>/signing`: the uploaded `.p12` identity with its
//! password (the files backend) and uploaded provisioning profiles (every backend).
//!
//! The directory is private (0700), every file in it is 0600 and replaced atomically. The
//! identity and its password are secrets: they are never logged or returned, and the
//! buffers that held them are zeroed when dropped.

use std::{
    fs::{self, DirBuilder, File},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use zeroize::Zeroizing;

use crate::{Error, Identity, Profile, Result, parse_profile};

const IDENTITY_FILE: &str = "identity.p12";
const PASSWORD_FILE: &str = "identity.password";
const PROFILES_DIR: &str = "profiles";

/// Largest `.p12` Kiwi accepts; an exported identity is a few KiB.
pub const MAX_P12_BYTES: usize = 64 * 1024;
/// Largest provisioning profile Kiwi accepts; a team profile with every device is far below.
pub const MAX_PROFILE_BYTES: usize = 1024 * 1024;
#[cfg_attr(target_os = "macos", allow(dead_code))]
const MAX_PASSWORD_BYTES: usize = 1024;

/// `<data>/signing`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningDir {
    root: PathBuf,
}

impl SigningDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Where uploaded profiles are kept, as `<uuid>.mobileprovision`.
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join(PROFILES_DIR)
    }

    /// Whether `profile` was loaded from the uploaded profiles.
    pub fn is_uploaded(&self, profile: &Profile) -> bool {
        profile.path.parent() == Some(self.profiles_dir().as_path())
    }

    /// Checks a provisioning profile and stores it, replacing an upload with the same UUID.
    pub fn add_profile(&self, bytes: &[u8], now: DateTime<Utc>) -> Result<Profile> {
        if bytes.len() > MAX_PROFILE_BYTES {
            return Err(Error::InvalidProfile(format!(
                "The provisioning profile is larger than {} MiB",
                MAX_PROFILE_BYTES >> 20
            )));
        }
        let profile = parse_profile(bytes)?;
        if !valid_uuid(&profile.uuid) {
            return Err(Error::InvalidProfile(
                "The provisioning profile has an invalid UUID".into(),
            ));
        }
        if profile.expires_at <= now {
            return Err(Error::InvalidProfile(format!(
                "'{}' expired on {}",
                profile.name,
                profile.expires_at.format("%Y-%m-%d")
            )));
        }
        if !profile.platforms.is_empty() && !profile.platforms.iter().any(|p| p == "iOS") {
            return Err(Error::InvalidProfile(format!(
                "'{}' is not an iOS profile",
                profile.name
            )));
        }
        self.create_dirs()?;
        let path = self
            .profiles_dir()
            .join(format!("{}.mobileprovision", profile.uuid));
        write_private(&path, bytes)?;
        Ok(Profile { path, ..profile })
    }

    /// Deletes an uploaded profile; `false` when no upload has this UUID.
    pub fn remove_profile(&self, uuid: &str) -> Result<bool> {
        if !valid_uuid(uuid) {
            return Ok(false);
        }
        let path = self.profiles_dir().join(format!("{uuid}.mobileprovision"));
        remove(&path)
    }

    /// The uploaded identity, `None` when there is none (and always on macOS, which signs with
    /// the keychain). Fails when the stored identity can't be opened any more.
    pub fn identity(&self) -> Result<Option<Identity>> {
        #[cfg(target_os = "macos")]
        {
            Ok(None)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(self.signing_key()?.map(|key| key.identity))
        }
    }

    /// Checks a `.p12` identity with its password and stores both, replacing the previous
    /// identity. The certificate must be a code signing certificate with a team id that has
    /// not expired, and the key must belong to it.
    pub fn set_identity(&self, p12: &[u8], password: &str, now: DateTime<Utc>) -> Result<Identity> {
        #[cfg(target_os = "macos")]
        {
            let _ = (p12, password, now);
            Err(Error::Unsupported(
                "Kiwi on macOS signs with the login keychain. Add the certificate to the keychain (for example with Xcode) instead of uploading it.",
            ))
        }
        #[cfg(not(target_os = "macos"))]
        {
            if password.len() > MAX_PASSWORD_BYTES {
                return Err(Error::InvalidIdentity("The password is too long".into()));
            }
            let key = crate::p12::open(p12, password)?;
            if let Some(expires) = key.identity.expires_at
                && expires <= now
            {
                return Err(Error::InvalidIdentity(format!(
                    "The certificate expired on {}",
                    expires.format("%Y-%m-%d")
                )));
            }
            self.create_dirs()?;
            write_private(&self.root.join(PASSWORD_FILE), password.as_bytes())?;
            write_private(&self.root.join(IDENTITY_FILE), p12)?;
            Ok(key.identity)
        }
    }

    /// Deletes the uploaded identity and its password; `false` when there was none.
    pub fn remove_identity(&self) -> Result<bool> {
        let removed = remove(&self.root.join(IDENTITY_FILE))?;
        remove(&self.root.join(PASSWORD_FILE))?;
        Ok(removed)
    }

    /// The stored identity with its key, for signing.
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn signing_key(&self) -> Result<Option<crate::p12::SigningKey>> {
        let Some(p12) = read_secret(&self.root.join(IDENTITY_FILE), MAX_P12_BYTES)? else {
            return Ok(None);
        };
        let mut password =
            read_secret(&self.root.join(PASSWORD_FILE), MAX_PASSWORD_BYTES)?.unwrap_or_default();
        let password = String::from_utf8(std::mem::take(&mut *password)).map_err(|e| {
            zeroize::Zeroize::zeroize(&mut e.into_bytes());
            Error::InvalidIdentity(
                "The stored signing identity's password is not valid UTF-8. Upload the identity again."
                    .into(),
            )
        })?;
        let password = Zeroizing::new(password);
        crate::p12::open(&p12, &password).map(Some).map_err(|e| {
            Error::InvalidIdentity(format!(
                "The uploaded signing identity can't be opened ({e}). Upload it again."
            ))
        })
    }

    /// Creates `<data>/signing` and its `profiles` directory, private to Kiwi's user.
    fn create_dirs(&self) -> Result<()> {
        for dir in [self.root.clone(), self.profiles_dir()] {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
                .and_then(|()| fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)))
                .map_err(Error::io_path("Could not create", &dir))?;
        }
        Ok(())
    }
}

/// UUIDs name files: letters, digits, `-`, `_` and `.` (not first) only.
fn valid_uuid(uuid: &str) -> bool {
    (1..=128).contains(&uuid.len())
        && !uuid.starts_with('.')
        && uuid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Writes `bytes` to a new 0600 file next to `path`, flushes it and renames it over `path`.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut file = tempfile::Builder::new()
        .prefix(".upload-")
        .permissions(fs::Permissions::from_mode(0o600))
        .tempfile_in(dir)
        .map_err(Error::io_path("Could not write to", dir))?;
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(Error::io_path("Could not write", path))?;
    file.persist(path)
        .map_err(|e| Error::io_path("Could not write", path)(e.error))?;
    Ok(())
}

/// Reads a secret file of at most `max` bytes; `None` when it does not exist.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn read_secret(path: &Path, max: usize) -> Result<Option<Zeroizing<Vec<u8>>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io_path("Could not read", path)(e)),
    };
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(Error::io_path("Could not read", path))?;
    if bytes.len() > max {
        return Err(Error::InvalidIdentity(format!(
            "{} is too large",
            path.display()
        )));
    }
    Ok(Some(bytes))
}

fn remove(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::io_path("Could not remove", path)(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{ProfileSpec, profile_bytes};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 10, 12, 0, 0).unwrap()
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn stores_and_removes_uploaded_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = SigningDir::new(tmp.path().join("signing"));
        let spec = ProfileSpec::wildcard();
        let profile = dir.add_profile(&profile_bytes(&spec), now()).unwrap();
        assert_eq!(profile.uuid, spec.uuid);
        assert_eq!(
            profile.path,
            dir.profiles_dir()
                .join(format!("{}.mobileprovision", spec.uuid))
        );
        assert!(dir.is_uploaded(&profile));
        assert_eq!(fs::read(&profile.path).unwrap(), profile_bytes(&spec));
        assert_eq!(mode(dir.path()), 0o700);
        assert_eq!(mode(&dir.profiles_dir()), 0o700);
        assert_eq!(mode(&profile.path), 0o600);

        // The same UUID replaces the upload; the loader sees it.
        dir.add_profile(&profile_bytes(&spec), now()).unwrap();
        let loaded = crate::load_profiles(&[dir.profiles_dir()]);
        assert_eq!(loaded.len(), 1);
        assert!(dir.is_uploaded(loaded[0].1.as_ref().unwrap()));

        assert!(!dir.remove_profile("../identity").unwrap());
        assert!(!dir.remove_profile("..").unwrap());
        assert!(!dir.remove_profile("unknown").unwrap());
        assert!(dir.remove_profile(&spec.uuid).unwrap());
        assert!(!profile.path.exists());
    }

    #[test]
    fn rejects_unusable_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = SigningDir::new(tmp.path().join("signing"));
        let err = |spec: &ProfileSpec| {
            dir.add_profile(&profile_bytes(spec), now())
                .unwrap_err()
                .to_string()
        };
        let expired = ProfileSpec {
            expires: "2026-01-01T00:00:00Z",
            ..ProfileSpec::wildcard()
        };
        assert_eq!(
            err(&expired),
            "'iOS Team Provisioning Profile: *' expired on 2026-01-01"
        );
        let mac = ProfileSpec {
            platforms: vec!["OSX"],
            ..ProfileSpec::wildcard()
        };
        assert!(err(&mac).ends_with("is not an iOS profile"));
        let sneaky = ProfileSpec {
            uuid: "../../identity".into(),
            ..ProfileSpec::wildcard()
        };
        assert_eq!(err(&sneaky), "The provisioning profile has an invalid UUID");
        let garbage = dir.add_profile(b"not a profile", now()).unwrap_err();
        assert!(matches!(garbage, Error::InvalidProfile(_)), "{garbage}");
        let huge = dir
            .add_profile(&vec![0; MAX_PROFILE_BYTES + 1], now())
            .unwrap_err();
        assert!(huge.to_string().contains("larger than"), "{huge}");
        assert!(!dir.path().exists(), "nothing was written");
    }

    #[test]
    fn missing_identity_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = SigningDir::new(tmp.path().join("signing"));
        assert_eq!(dir.identity().unwrap(), None);
        assert!(!dir.remove_identity().unwrap());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_signs_with_the_keychain() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = SigningDir::new(tmp.path().join("signing"));
        let err = dir.set_identity(b"p12", "secret", now()).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)));
    }

    #[test]
    fn reads_secrets_with_a_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret");
        assert!(read_secret(&path, 4).unwrap().is_none());
        write_private(&path, b"1234").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(read_secret(&path, 4).unwrap().unwrap().as_slice(), b"1234");
        assert!(read_secret(&path, 3).is_err());
        assert_eq!(
            fs::read_dir(tmp.path()).unwrap().count(),
            1,
            "no temporary files are left"
        );
    }
}
