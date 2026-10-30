//! Signing identities and profiles behind the [`Signer`] trait, so tests on Linux can
//! inject a fake signer.

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use kiwi_signer::{Identity, Profile, SignReport, SignRequest, SignStep};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::db::Time;

pub trait Signer: Send + Sync + 'static {
    fn identities(&self) -> Result<Vec<Identity>, String>;
    fn profiles(&self) -> Vec<(PathBuf, Result<Profile, String>)>;
    /// Blocking.
    fn sign(
        &self,
        req: &SignRequest,
        cancel: &AtomicBool,
        progress: &dyn Fn(SignStep),
    ) -> Result<SignReport, String>;
}

/// The keychain, Xcode's profile directories and `codesign` (macOS only).
pub struct AppleSigner;

impl Signer for AppleSigner {
    fn identities(&self) -> Result<Vec<Identity>, String> {
        kiwi_signer::list_identities().map_err(|e| e.to_string())
    }
    fn profiles(&self) -> Vec<(PathBuf, Result<Profile, String>)> {
        kiwi_signer::load_profiles(&kiwi_signer::default_profile_dirs())
            .into_iter()
            .map(|(path, r)| (path, r.map_err(|e| e.to_string())))
            .collect()
    }
    fn sign(
        &self,
        req: &SignRequest,
        cancel: &AtomicBool,
        progress: &dyn Fn(SignStep),
    ) -> Result<SignReport, String> {
        kiwi_signer::sign_ipa(req, cancel, progress).map_err(|e| e.to_string())
    }
}

/// Longest a signing scan may take before it is reported as an error (`security` itself is
/// stopped after 30 s per call; this also covers a hung profile directory).
pub const SIGNING_SCAN_TIMEOUT: Duration = Duration::from_secs(90);

/// Scans identities and profiles on a blocking thread, one scan at a time (`lock`). After
/// `timeout` it gives up and reports the keychain as unavailable, keeping the `previous`
/// profiles. A scan that timed out keeps `lock` until it really ends, so later scans time
/// out too instead of piling up blocked threads.
pub async fn scan_signing(
    signer: Arc<dyn Signer>,
    lock: Arc<Mutex<()>>,
    previous: &SigningInfo,
    timeout: Duration,
) -> SigningInfo {
    let scan = async move {
        let guard = lock.lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            SigningInfo::scan(&*signer)
        })
        .await
    };
    match tokio::time::timeout(timeout, scan).await {
        Ok(Ok(info)) => info,
        Ok(Err(e)) => SigningInfo {
            identity_error: Some(format!("Signing scan failed: {e}")),
            ..Default::default()
        },
        Err(_) => {
            tracing::warn!("reading signing identities did not finish in {timeout:?}");
            SigningInfo {
                identity_error: Some(
                    "Reading the keychain timed out. If the Mac shows a keychain prompt, answer it, then rescan.".into(),
                ),
                profiles: previous.profiles.clone(),
                profile_errors: previous.profile_errors.clone(),
                ..Default::default()
            }
        }
    }
}

/// A snapshot of the identities and profiles.
#[derive(Debug, Clone, Default)]
pub struct SigningInfo {
    pub identities: Vec<Identity>,
    pub identity_error: Option<String>,
    pub profiles: Vec<Profile>,
    pub profile_errors: Vec<String>,
}

/// How a bundle id would be signed now.
#[derive(Debug, Clone)]
pub struct Plan {
    pub identity: Identity,
    pub profile: Profile,
    /// Earliest of the profile's and the certificate's expiry.
    pub expires_at: Time,
}

impl Plan {
    /// Changes whenever a ready build signed for this plan should not be reused.
    pub fn signing_key(&self, strip_watch: bool) -> String {
        format!(
            "{}|{}|{}|strip_watch={strip_watch}",
            self.identity.sha1,
            self.profile.uuid,
            self.profile.expires_at.timestamp()
        )
    }
}

#[derive(Debug, Serialize)]
pub struct SigningJson {
    pub identities: Vec<IdentityJson>,
    pub profiles: Vec<ProfileJson>,
    pub active: Option<ActiveJson>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IdentityJson {
    pub sha1: String,
    pub name: String,
    pub team_id: Option<String>,
    pub expires_at: Option<Time>,
}

#[derive(Debug, Serialize)]
pub struct ProfileJson {
    pub uuid: String,
    pub name: String,
    pub team_id: String,
    pub app_id_pattern: String,
    pub wildcard: bool,
    pub development: bool,
    pub created_at: Time,
    pub expires_at: Time,
    pub devices: usize,
    pub entitlements: Vec<String>,
    pub usable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActiveJson {
    pub identity_sha1: String,
    pub profile_uuid: String,
    pub expires_at: Time,
}

/// Matches only a team-wide wildcard App ID (`TEAM.*`).
const WILDCARD_PROBE: &str = "kiwi.wildcard.probe";

impl SigningInfo {
    /// Blocking: lists identities and parses profiles.
    pub fn scan(signer: &dyn Signer) -> Self {
        let (identities, identity_error) = match signer.identities() {
            Ok(ids) => (ids, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let mut profiles = Vec::new();
        let mut profile_errors = Vec::new();
        for (path, result) in signer.profiles() {
            match result {
                Ok(p) => profiles.push(p),
                Err(e) => profile_errors.push(format!("{}: {e}", path.display())),
            }
        }
        Self {
            identities,
            identity_error,
            profiles,
            profile_errors,
        }
    }

    fn plan_with(&self, profiles: &[Profile], bundle_id: &str, now: Time) -> Result<Plan, String> {
        let mut first_error = None;
        let mut best: Option<Plan> = None;
        for identity in &self.identities {
            match kiwi_signer::select_profile(profiles, identity, bundle_id, now) {
                Ok(profile) => {
                    let expires_at = identity
                        .expires_at
                        .map_or(profile.expires_at, |e| e.min(profile.expires_at));
                    if best.as_ref().is_none_or(|b| expires_at > b.expires_at) {
                        best = Some(Plan {
                            identity: identity.clone(),
                            profile: profile.clone(),
                            expires_at,
                        });
                    }
                }
                Err(e) => {
                    first_error.get_or_insert(e.to_string());
                }
            }
        }
        best.ok_or_else(|| first_error.unwrap_or_else(|| self.no_identity_message()))
    }

    fn no_identity_message(&self) -> String {
        self.identity_error.clone().unwrap_or_else(|| {
            "No code signing identity was found in the login keychain. Sign in to Xcode and create an Apple Development certificate.".into()
        })
    }

    /// The identity and profile that would sign `signed_bundle_id`, latest expiry first.
    pub fn plan(&self, signed_bundle_id: &str, now: Time) -> Result<Plan, String> {
        self.plan_with(&self.profiles, signed_bundle_id, now)
    }

    /// The default choice: the best identity with a team-wide wildcard profile.
    pub fn active(&self, now: Time) -> Option<Plan> {
        let wildcards: Vec<Profile> = self
            .profiles
            .iter()
            .filter(|p| p.app_id_pattern == format!("{}.*", p.app_id_prefix()))
            .cloned()
            .collect();
        self.plan_with(&wildcards, WILDCARD_PROBE, now).ok()
    }

    /// Team of the active identity, else of any identity.
    pub fn team_id(&self, now: Time) -> Option<String> {
        self.active(now)
            .and_then(|p| p.identity.team_id.or(Some(p.profile.team_id)))
            .or_else(|| self.identities.iter().find_map(|i| i.team_id.clone()))
    }

    /// Declared entitlements the profile that would sign `signed_bundle_id` cannot grant.
    pub fn dropped_preview(
        &self,
        signed_bundle_id: &str,
        declared: &[String],
        now: Time,
    ) -> Vec<String> {
        let Ok(plan) = self.plan(signed_bundle_id, now) else {
            return Vec::new();
        };
        declared
            .iter()
            .filter(|e| !plan.profile.entitlements.contains_key(e.as_str()))
            .cloned()
            .collect()
    }

    /// Why signing is unavailable, if it is.
    pub fn error(&self, now: Time) -> Option<String> {
        if self.identities.is_empty() {
            Some(self.no_identity_message())
        } else if self.active(now).is_none() {
            Some("No usable wildcard provisioning profile was found. Open an app project in Xcode with automatic signing once, then rescan.".into())
        } else {
            None
        }
    }

    pub fn json(&self, now: Time) -> SigningJson {
        let active = self.active(now);
        let reference = active
            .as_ref()
            .map(|a| &a.identity)
            .or(self.identities.first());
        let mut profiles: Vec<ProfileJson> = self
            .profiles
            .iter()
            .map(|p| {
                let reason = match reference {
                    Some(identity) => p.usable_with(identity, now).err(),
                    None => Some("No signing identity".to_owned()),
                };
                ProfileJson {
                    uuid: p.uuid.clone(),
                    name: p.name.clone(),
                    team_id: p.team_id.clone(),
                    app_id_pattern: p.app_id_pattern.clone(),
                    wildcard: p.is_wildcard(),
                    development: p.development,
                    created_at: p.created_at,
                    expires_at: p.expires_at,
                    devices: p.devices.len(),
                    entitlements: kiwi_signer::profile_entitlement_keys(p),
                    usable: reason.is_none(),
                    reason,
                }
            })
            .collect();
        profiles.sort_by(|a, b| {
            (b.usable, b.wildcard, b.expires_at).cmp(&(a.usable, a.wildcard, a.expires_at))
        });
        SigningJson {
            identities: self
                .identities
                .iter()
                .map(|i| IdentityJson {
                    sha1: i.sha1.clone(),
                    name: i.name.clone(),
                    team_id: i.team_id.clone(),
                    expires_at: i.expires_at,
                })
                .collect(),
            profiles,
            active: active.map(|a| ActiveJson {
                identity_sha1: a.identity.sha1,
                profile_uuid: a.profile.uuid,
                expires_at: a.expires_at,
            }),
            error: self.error(now),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TEAM, identity, profile};
    use chrono::{Duration, Utc};

    #[test]
    fn plans_prefer_explicit_profiles_and_report_expiry() {
        let now = Utc::now();
        let mut explicit = profile(
            "explicit",
            &format!("{TEAM}.com.example.app.{TEAM}"),
            now + Duration::days(100),
        );
        explicit.entitlements.insert(
            "com.apple.security.application-groups".into(),
            plist::Value::Array(vec![]),
        );
        let info = SigningInfo {
            identities: vec![identity(now + Duration::days(300))],
            profiles: vec![
                profile("wild", &format!("{TEAM}.*"), now + Duration::days(200)),
                explicit,
            ],
            ..Default::default()
        };
        let signed = format!("com.example.app.{TEAM}");
        let plan = info.plan(&signed, now).unwrap();
        assert_eq!(plan.profile.uuid, "explicit");
        assert_eq!(plan.expires_at, now + Duration::days(100));
        let active = info.active(now).unwrap();
        assert_eq!(active.profile.uuid, "wild");
        assert_eq!(info.team_id(now).as_deref(), Some(TEAM));
        assert!(info.error(now).is_none());
        let declared = vec![
            "com.apple.security.application-groups".to_owned(),
            "com.apple.developer.healthkit".to_owned(),
        ];
        assert_eq!(
            info.dropped_preview(&signed, &declared, now),
            vec!["com.apple.developer.healthkit"]
        );
        assert_eq!(
            info.dropped_preview(&format!("other.app.{TEAM}"), &declared, now),
            declared
        );
        let json = info.json(now);
        assert_eq!(json.active.unwrap().profile_uuid, "wild");
        assert!(json.profiles.iter().all(|p| p.usable));
    }

    /// Identities block until `release` is set.
    #[derive(Default)]
    struct Stuck {
        release: std::sync::atomic::AtomicBool,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Signer for Stuck {
        fn identities(&self) -> Result<Vec<Identity>, String> {
            use std::sync::atomic::Ordering::SeqCst;
            self.calls.fetch_add(1, SeqCst);
            while !self.release.load(SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(vec![identity(Utc::now() + Duration::days(300))])
        }
        fn profiles(&self) -> Vec<(PathBuf, Result<Profile, String>)> {
            Vec::new()
        }
        fn sign(
            &self,
            _: &SignRequest,
            _: &AtomicBool,
            _: &dyn Fn(SignStep),
        ) -> Result<SignReport, String> {
            Err("unused".into())
        }
    }

    #[tokio::test]
    async fn hung_scans_time_out_without_piling_up() {
        use std::sync::atomic::Ordering::SeqCst;
        let now = Utc::now();
        let stuck = Arc::new(Stuck::default());
        let lock = Arc::default();
        let previous = SigningInfo {
            profiles: vec![profile(
                "wild",
                &format!("{TEAM}.*"),
                now + Duration::days(200),
            )],
            ..Default::default()
        };
        let timeout = std::time::Duration::from_millis(100);
        for _ in 0..2 {
            let info = scan_signing(stuck.clone(), Arc::clone(&lock), &previous, timeout).await;
            assert!(info.identities.is_empty());
            assert_eq!(info.profiles.len(), 1, "keeps the known profiles");
            assert!(info.error(now).unwrap().contains("keychain prompt"));
        }
        assert_eq!(stuck.calls.load(SeqCst), 1, "waits for the hung scan");
        stuck.release.store(true, SeqCst);
        let info = scan_signing(
            stuck.clone(),
            lock,
            &previous,
            std::time::Duration::from_secs(10),
        )
        .await;
        assert_eq!(info.identities.len(), 1);
        assert!(info.profiles.is_empty());
    }

    #[test]
    fn explains_missing_identity_and_profiles() {
        let now = Utc::now();
        let none = SigningInfo {
            identity_error: Some("Signing is only available on macOS".into()),
            ..Default::default()
        };
        assert_eq!(
            none.error(now).unwrap(),
            "Signing is only available on macOS"
        );
        assert_eq!(
            none.plan("a.b", now).unwrap_err(),
            "Signing is only available on macOS"
        );
        let expired = SigningInfo {
            identities: vec![identity(now + Duration::days(300))],
            profiles: vec![profile(
                "old",
                &format!("{TEAM}.*"),
                now - Duration::days(1),
            )],
            ..Default::default()
        };
        assert!(expired.error(now).unwrap().contains("wildcard"));
        assert!(expired.plan("a.b", now).unwrap_err().contains("expired"));
        let json = expired.json(now);
        assert!(!json.profiles[0].usable);
        assert!(
            json.profiles[0]
                .reason
                .as_ref()
                .unwrap()
                .contains("expired")
        );
    }
}
