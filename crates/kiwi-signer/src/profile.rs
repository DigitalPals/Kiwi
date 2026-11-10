use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{Error, Identity, Result, cert::sha1_hex, check_plist, der::cms_content};

/// A parsed `.mobileprovision`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub uuid: String,
    pub name: String,
    /// File the profile was loaded from (copied into bundles as `embedded.mobileprovision`).
    pub path: PathBuf,
    pub team_id: String,
    /// `application-identifier` entitlement, e.g. `"A1B2C3D4E5.*"`.
    pub app_id_pattern: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub devices: Vec<String>,
    pub entitlements: plist::Dictionary,
    /// Upper-case hex SHA-1s of `DeveloperCertificates`.
    pub certificate_sha1s: Vec<String>,
    pub platforms: Vec<String>,
    /// `get-task-allow`: a development profile.
    pub development: bool,
}

/// How a profile's App ID covers a bundle id. `Exact` sorts above `Wildcard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProfileMatch {
    Wildcard,
    Exact,
}

impl Profile {
    pub fn is_wildcard(&self) -> bool {
        self.app_id_pattern.ends_with('*')
    }

    /// App ID prefix of the pattern (the team id for all modern App IDs).
    pub fn app_id_prefix(&self) -> &str {
        self.app_id_pattern
            .split_once('.')
            .map_or(&self.team_id, |(prefix, _)| prefix)
    }

    /// Whether this profile's App ID covers `bundle_id` (without the team prefix).
    pub fn match_bundle(&self, bundle_id: &str) -> Option<ProfileMatch> {
        let (_, pattern) = self.app_id_pattern.split_once('.')?;
        if pattern == bundle_id {
            Some(ProfileMatch::Exact)
        } else if let Some(prefix) = pattern.strip_suffix('*')
            && bundle_id.len() > prefix.len()
            && bundle_id.starts_with(prefix)
        {
            Some(ProfileMatch::Wildcard)
        } else {
            None
        }
    }

    /// Checks team, certificate, expiry and platform; the error says why not.
    pub fn usable_with(&self, identity: &Identity, now: DateTime<Utc>) -> Result<(), String> {
        if let Some(team) = &identity.team_id
            && *team != self.team_id
        {
            return Err(format!(
                "'{}' is for team {}, but the signing identity is from team {team}",
                self.name, self.team_id
            ));
        }
        if !self
            .certificate_sha1s
            .iter()
            .any(|sha1| sha1.eq_ignore_ascii_case(&identity.sha1))
        {
            return Err(format!(
                "'{}' does not include the certificate of {}",
                self.name, identity.name
            ));
        }
        if self.expires_at <= now {
            return Err(format!(
                "'{}' expired on {}",
                self.name,
                self.expires_at.format("%Y-%m-%d")
            ));
        }
        if !self.platforms.is_empty() && !self.platforms.iter().any(|p| p == "iOS") {
            return Err(format!("'{}' is not an iOS profile", self.name));
        }
        Ok(())
    }
}

/// Xcode's profile directory and the legacy MobileDevice one.
pub fn default_profile_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
        return Vec::new();
    };
    let library = Path::new(&home).join("Library");
    vec![
        library.join("Developer/Xcode/UserData/Provisioning Profiles"),
        library.join("MobileDevice/Provisioning Profiles"),
    ]
}

/// Loads every `*.mobileprovision` in `dirs` (missing directories are skipped). A
/// profile present in several directories is returned once, from the first.
pub fn load_profiles(dirs: &[PathBuf]) -> Vec<(PathBuf, Result<Profile>)> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    for dir in dirs {
        let mut paths: Vec<PathBuf> = match fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|e| Some(e.ok()?.path()))
                .filter(|p| {
                    p.extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("mobileprovision"))
                })
                .collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                results.push((dir.clone(), Err(Error::io_path("Could not read", dir)(e))));
                continue;
            }
        };
        paths.sort();
        for path in paths {
            let result = fs::read(&path)
                .map_err(Error::io_path("Could not read", &path))
                .and_then(|bytes| parse_profile(&bytes))
                .map(|profile| Profile {
                    path: path.clone(),
                    ..profile
                });
            if let Ok(profile) = &result
                && !seen.insert(profile.uuid.clone())
            {
                continue;
            }
            results.push((path, result));
        }
    }
    results
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawProfile {
    #[serde(rename = "UUID")]
    uuid: String,
    name: String,
    team_identifier: Vec<String>,
    creation_date: plist::Date,
    expiration_date: plist::Date,
    #[serde(default)]
    provisioned_devices: Vec<String>,
    entitlements: plist::Dictionary,
    #[serde(default)]
    developer_certificates: Vec<plist::Data>,
    #[serde(default)]
    platform: Vec<String>,
}

/// Parses a `.mobileprovision` (the plist inside its CMS envelope, nested at most 64
/// levels deep; the signature is not verified). `path` is left empty.
pub fn parse_profile(bytes: &[u8]) -> Result<Profile> {
    let invalid =
        |reason: &str| Error::InvalidProfile(format!("Invalid provisioning profile: {reason}"));
    let content = cms_content(bytes).ok_or_else(|| invalid("not a signed profile"))?;
    // Deserializing recurses per nesting level: check the depth first.
    check_plist(&content).map_err(|e| invalid(&e))?;
    let raw: RawProfile = plist::from_bytes(&content).map_err(|e| invalid(&e.to_string()))?;
    let team_id = raw
        .team_identifier
        .into_iter()
        .next()
        .ok_or_else(|| invalid("no team identifier"))?;
    let app_id_pattern = raw
        .entitlements
        .get("application-identifier")
        .and_then(plist::Value::as_string)
        .ok_or_else(|| invalid("no application-identifier entitlement"))?
        .to_owned();
    let development = raw
        .entitlements
        .get("get-task-allow")
        .and_then(plist::Value::as_boolean)
        .unwrap_or(false);
    Ok(Profile {
        uuid: raw.uuid,
        name: raw.name,
        path: PathBuf::new(),
        team_id,
        app_id_pattern,
        created_at: date(raw.creation_date),
        expires_at: date(raw.expiration_date),
        devices: raw.provisioned_devices,
        certificate_sha1s: raw
            .developer_certificates
            .iter()
            .map(|der| sha1_hex(der.as_ref()))
            .collect(),
        platforms: raw.platform,
        development,
        entitlements: raw.entitlements,
    })
}

fn date(date: plist::Date) -> DateTime<Utc> {
    std::time::SystemTime::from(date).into()
}

/// Entitlement keys a profile grants, sorted.
pub fn profile_entitlement_keys(p: &Profile) -> Vec<String> {
    let mut keys: Vec<String> = p.entitlements.keys().cloned().collect();
    keys.sort();
    keys
}

/// Chooses the profile to sign `bundle_id` with: usable profiles only, an exact App ID
/// before wildcards, the most specific wildcard, then the latest expiry.
pub fn select_profile<'a>(
    profiles: &'a [Profile],
    identity: &Identity,
    bundle_id: &str,
    now: DateTime<Utc>,
) -> Result<&'a Profile> {
    let mut reasons = Vec::new();
    let best = profiles
        .iter()
        .filter_map(|p| {
            let kind = p.match_bundle(bundle_id)?;
            match p.usable_with(identity, now) {
                Ok(()) => Some((kind, p.app_id_pattern.len(), p.expires_at, p.created_at, p)),
                Err(reason) => {
                    reasons.push(reason);
                    None
                }
            }
        })
        .max_by_key(|&(kind, len, expires, created, _)| (kind, len, expires, created));
    if let Some((.., profile)) = best {
        return Ok(profile);
    }
    let app_id = match &identity.team_id {
        Some(team) => format!("{team}.{bundle_id}"),
        None => bundle_id.to_owned(),
    };
    let mut message = format!("No provisioning profile for {app_id} covers this signing identity");
    if !reasons.is_empty() {
        message = format!("{message} ({})", reasons.join("; "));
    } else if profiles.is_empty() {
        message.push_str(" (no provisioning profiles were found; open the project in Xcode once)");
    }
    Err(Error::NoProfile(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{ProfileSpec, TEAM, identity, profile, profile_bytes};
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
    }

    #[test]
    fn parses_profile() {
        let spec = ProfileSpec::wildcard();
        let p = parse_profile(&profile_bytes(&spec)).unwrap();
        assert_eq!(p.uuid, spec.uuid);
        assert_eq!(p.name, "iOS Team Provisioning Profile: *");
        assert_eq!(p.team_id, TEAM);
        assert_eq!(p.app_id_pattern, format!("{TEAM}.*"));
        assert!(p.is_wildcard());
        assert!(p.development);
        assert_eq!(p.app_id_prefix(), TEAM);
        assert_eq!(
            p.devices,
            vec![
                "00000000-0000000000000001",
                "0000000000000000000000000000000000000002"
            ]
        );
        assert_eq!(p.platforms, vec!["iOS", "xrOS"]);
        assert_eq!(
            p.certificate_sha1s,
            vec![identity().sha1, sha1_hex(b"other")]
        );
        assert_eq!(
            p.expires_at,
            Utc.with_ymd_and_hms(2027, 10, 26, 18, 52, 6).unwrap()
        );
        assert_eq!(
            p.created_at,
            Utc.with_ymd_and_hms(2026, 10, 26, 18, 52, 6).unwrap()
        );
        assert_eq!(
            profile_entitlement_keys(&p),
            vec![
                "application-identifier",
                "com.apple.developer.team-identifier",
                "get-task-allow",
                "keychain-access-groups"
            ]
        );
    }

    #[test]
    fn rejects_deeply_nested_profiles() {
        let bytes = crate::testutil::nested_profile(60_000);
        assert!(bytes.len() <= crate::MAX_PROFILE_BYTES);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.mobileprovision");
        fs::write(&path, &bytes).unwrap();
        // With the stack of tokio's threads, where uploads and scans run.
        let (parsed, loaded) = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let parsed = parse_profile(&bytes).map(|_| ()).map_err(|e| e.to_string());
                let loaded: Vec<_> = load_profiles(&[dir.path().to_path_buf()])
                    .into_iter()
                    .map(|(path, r)| (path, r.map(|_| ()).map_err(|e| e.to_string())))
                    .collect();
                (parsed, loaded)
            })
            .unwrap()
            .join()
            .unwrap();
        let message = "Invalid provisioning profile: nested more than 64 levels deep";
        assert_eq!(parsed, Err(message.into()));
        assert_eq!(loaded, [(path, Err(message.into()))]);
    }

    #[test]
    fn rejects_invalid_profiles() {
        let err = parse_profile(b"<?xml version=\"1.0\"?><plist/>").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid provisioning profile: not a signed profile"
        );
        let not_plist = crate::testutil::cms(b"garbage");
        assert!(matches!(
            parse_profile(&not_plist),
            Err(Error::InvalidProfile(_))
        ));
        let mut spec = ProfileSpec::wildcard();
        spec.app_id = None;
        let err = parse_profile(&profile_bytes(&spec)).unwrap_err();
        assert!(err.to_string().contains("application-identifier"), "{err}");
    }

    #[test]
    fn matches_bundle_ids() {
        let wildcard = profile(&ProfileSpec::wildcard());
        let exact = profile(&ProfileSpec::explicit("com.example.app"));
        let prefix = profile(&ProfileSpec::explicit("com.example.*"));
        assert_eq!(
            wildcard.match_bundle("com.example.app"),
            Some(ProfileMatch::Wildcard)
        );
        assert_eq!(
            exact.match_bundle("com.example.app"),
            Some(ProfileMatch::Exact)
        );
        assert_eq!(exact.match_bundle("com.example.app.widget"), None);
        assert_eq!(exact.match_bundle("com.example"), None);
        assert_eq!(
            prefix.match_bundle("com.example.app.widget"),
            Some(ProfileMatch::Wildcard)
        );
        assert_eq!(prefix.match_bundle("com.example"), None);
        assert_eq!(prefix.match_bundle("com.examples.app"), None);
        assert!(ProfileMatch::Exact > ProfileMatch::Wildcard);
    }

    #[test]
    fn usability_checks() {
        let id = identity();
        let p = profile(&ProfileSpec::wildcard());
        assert_eq!(p.usable_with(&id, now()), Ok(()));

        let other_team = Identity {
            team_id: Some("OTHERTEAM1".into()),
            ..id.clone()
        };
        assert!(
            p.usable_with(&other_team, now())
                .unwrap_err()
                .contains("team OTHERTEAM1")
        );

        let no_team = Identity {
            team_id: None,
            ..id.clone()
        };
        assert_eq!(p.usable_with(&no_team, now()), Ok(()));

        let other_cert = Identity {
            sha1: sha1_hex(b"unknown"),
            ..id.clone()
        };
        assert!(
            p.usable_with(&other_cert, now())
                .unwrap_err()
                .contains("does not include the certificate")
        );

        let later = p.expires_at + Duration::seconds(1);
        assert_eq!(
            p.usable_with(&id, later).unwrap_err(),
            "'iOS Team Provisioning Profile: *' expired on 2027-10-26"
        );

        let mut tv = ProfileSpec::wildcard();
        tv.platforms = vec!["tvOS"];
        assert!(
            profile(&tv)
                .usable_with(&id, now())
                .unwrap_err()
                .contains("not an iOS profile")
        );
    }

    #[test]
    fn selects_exact_before_wildcard() {
        let id = identity();
        let wildcard = profile(&ProfileSpec::wildcard());
        let mut prefix_spec = ProfileSpec::explicit("com.example.*");
        prefix_spec.uuid = "prefix".into();
        let prefix = profile(&prefix_spec);
        let mut exact_spec = ProfileSpec::explicit("com.example.app");
        exact_spec.expires = "2026-12-01T00:00:00Z";
        let exact = profile(&exact_spec);
        let mut expired_spec = ProfileSpec::explicit("com.example.app");
        expired_spec.uuid = "expired".into();
        expired_spec.expires = "2026-01-01T00:00:00Z";
        let expired = profile(&expired_spec);

        let all = vec![
            wildcard.clone(),
            expired.clone(),
            prefix.clone(),
            exact.clone(),
        ];
        let pick = |profiles: &[Profile], bundle: &str| {
            select_profile(profiles, &id, bundle, now()).map(|p| p.uuid.clone())
        };
        assert_eq!(pick(&all, "com.example.app").unwrap(), exact.uuid);
        assert_eq!(pick(&all, "com.example.other").unwrap(), "prefix");
        assert_eq!(pick(&all, "org.other").unwrap(), wildcard.uuid);
        assert_eq!(
            pick(&[wildcard.clone(), expired.clone()], "com.example.app").unwrap(),
            wildcard.uuid
        );

        let mut newer_spec = ProfileSpec::wildcard();
        newer_spec.uuid = "newer".into();
        newer_spec.expires = "2028-01-01T00:00:00Z";
        let newer = profile(&newer_spec);
        assert_eq!(
            pick(&[wildcard.clone(), newer], "org.other").unwrap(),
            "newer"
        );
    }

    #[test]
    fn selection_errors_are_readable() {
        let id = identity();
        let mut expired = ProfileSpec::explicit("com.example.app");
        expired.expires = "2026-01-01T00:00:00Z";
        let err = select_profile(&[profile(&expired)], &id, "com.example.app", now()).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "No provisioning profile for {TEAM}.com.example.app covers this signing identity \
                 ('iOS Team Provisioning Profile: com.example.app' expired on 2026-01-01)"
            )
        );
        let err = select_profile(
            &[profile(&ProfileSpec::explicit("com.example.app"))],
            &id,
            "org.other",
            now(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("No provisioning profile for {TEAM}.org.other covers this signing identity")
        );
        assert!(
            select_profile(&[], &id, "x", now())
                .unwrap_err()
                .to_string()
                .contains("no provisioning profiles")
        );
    }

    #[test]
    fn loads_profile_directories() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let wildcard = ProfileSpec::wildcard();
        fs::write(first.join("b.mobileprovision"), profile_bytes(&wildcard)).unwrap();
        fs::write(first.join("a.mobileprovision"), b"junk").unwrap();
        fs::write(first.join("notes.txt"), b"ignored").unwrap();
        fs::write(second.join("dup.mobileprovision"), profile_bytes(&wildcard)).unwrap();
        fs::write(
            second.join("c.MOBILEPROVISION"),
            profile_bytes(&ProfileSpec::explicit("com.x")),
        )
        .unwrap();

        let loaded = load_profiles(&[first.clone(), dir.path().join("missing"), second.clone()]);
        let names: Vec<_> = loaded
            .iter()
            .map(|(p, r)| (p.file_name().unwrap().to_owned(), r.is_ok()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("a.mobileprovision".into(), false),
                ("b.mobileprovision".into(), true),
                ("c.MOBILEPROVISION".into(), true)
            ]
        );
        let profile = loaded[1].1.as_ref().unwrap();
        assert_eq!(profile.path, first.join("b.mobileprovision"));
    }

    #[test]
    fn default_dirs_are_under_home() {
        let dirs = default_profile_dirs();
        if std::env::var_os("HOME").is_some() {
            assert_eq!(dirs.len(), 2);
            assert!(dirs[0].ends_with("Library/Developer/Xcode/UserData/Provisioning Profiles"));
            assert!(dirs[1].ends_with("Library/MobileDevice/Provisioning Profiles"));
        }
    }
}
