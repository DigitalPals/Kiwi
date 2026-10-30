//! App actions per device (architecture.md "App actions"). Pure: inputs are loaded by the caller.

use chrono::Duration;
use kiwi_source::{App, DeviceProfile, Release, ReleaseChange, Url, release_change};
use serde::Serialize;

use crate::db::{BuildRow, BuildStatus, InstallRow, Time};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Get,
    Preparing,
    Install,
    Installing,
    Open,
    Update,
    Retry,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateReason {
    NewVersion,
    Resign,
}

/// Installed Kiwi builds expiring within this window are offered a re-sign.
pub const RESIGN_WINDOW: Duration = Duration::days(30);
/// A manifest fetch or download start counts as "installing" for this long.
pub const INSTALLING_WINDOW: Duration = Duration::minutes(10);

pub struct Inputs<'a> {
    pub app: &'a App,
    /// `None` when the OS version is unknown (all releases are eligible).
    pub device: Option<&'a DeviceProfile>,
    pub installed: Option<&'a InstallRow>,
    /// Builds of this app, newest first.
    pub builds: &'a [BuildRow],
    /// The device has an install of this app in progress.
    pub installing: bool,
    /// Expiry a new signature would get; `None` when signing is unavailable.
    pub resign_expiry: Option<Time>,
    pub now: Time,
}

#[derive(Debug)]
pub struct Decision<'a> {
    pub action: Action,
    pub update_reason: Option<UpdateReason>,
    pub target: Option<usize>,
    /// Latest build of the target release.
    pub build: Option<&'a BuildRow>,
    /// Ready build the install link points at.
    pub install_build: Option<&'a BuildRow>,
    pub open_url: Option<String>,
}

/// The release an install corresponds to, for `release_change`.
pub fn installed_release(app: &App, installed: &InstallRow) -> Release {
    if let Some(release) = &installed.release {
        return release.clone();
    }
    let same = |r: &&Release| {
        r.version == installed.version && (installed.build.is_none() || r.build == installed.build)
    };
    app.releases.iter().find(same).cloned().unwrap_or(Release {
        version: installed.version.clone(),
        build: installed.build.clone(),
        revision: None,
        date: None,
        notes: None,
        download_url: Url::parse("https://kiwi.invalid/installed").expect("valid URL"),
        size: None,
        sha256: None,
        min_os: None,
        max_os: None,
        platforms: Vec::new(),
    })
}

pub fn same_release(build: &BuildRow, release: &Release) -> bool {
    build.release.same_release(release) && build.release.same_artifact(release)
}

fn needs_resign(installed: &InstallRow, resign_expiry: Option<Time>, now: Time) -> bool {
    match (
        installed.origin.as_str(),
        installed.expires_at,
        resign_expiry,
    ) {
        ("kiwi", Some(expires), Some(fresh)) => expires - now < RESIGN_WINDOW && fresh > expires,
        _ => false,
    }
}

pub fn decide<'a>(i: &Inputs<'a>) -> Decision<'a> {
    let open_url = i
        .installed
        .filter(|inst| inst.origin == "kiwi")
        .and_then(|inst| inst.url_scheme.as_ref())
        .map(|scheme| format!("{scheme}://"));
    let Some(target) = i.app.newest_eligible(i.device) else {
        return Decision {
            action: if i.installed.is_some() {
                Action::Open
            } else {
                Action::Unavailable
            },
            update_reason: None,
            target: None,
            build: None,
            install_build: None,
            open_url,
        };
    };
    let release = &i.app.releases[target];
    let build = i.builds.iter().find(|b| same_release(b, release));
    let update_reason = i.installed.and_then(|inst| {
        match release_change(i.app, target, &installed_release(i.app, inst)) {
            ReleaseChange::Newer => Some(UpdateReason::NewVersion),
            _ if needs_resign(inst, i.resign_expiry, i.now) => Some(UpdateReason::Resign),
            _ => None,
        }
    });
    let ready = build.filter(|b| {
        b.usable(i.now)
            && (update_reason != Some(UpdateReason::Resign)
                || b.expires_at > i.installed.and_then(|inst| inst.expires_at))
    });
    let mut action = match build.map(|b| b.status) {
        Some(status) if status.is_active() => Action::Preparing,
        Some(BuildStatus::Failed) => Action::Retry,
        _ if i.installed.is_some() && update_reason.is_some() => Action::Update,
        _ if i.installed.is_some() => Action::Open,
        _ if ready.is_some() => Action::Install,
        _ => Action::Get,
    };
    if i.installing && matches!(action, Action::Install | Action::Update) {
        action = Action::Installing;
    }
    let linkable = matches!(
        action,
        Action::Install | Action::Update | Action::Installing
    );
    Decision {
        action,
        update_reason,
        target: Some(target),
        build,
        install_build: ready.filter(|_| linkable),
        open_url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{app, build_for, install_of};
    use chrono::Utc;

    struct Case {
        app: App,
        device: Option<DeviceProfile>,
        installed: Option<InstallRow>,
        builds: Vec<BuildRow>,
        installing: bool,
        resign_expiry: Option<Time>,
        now: Time,
    }

    impl Case {
        fn new() -> Self {
            Self {
                app: app(&[("2.0", Some("16.0")), ("1.0", None)]),
                device: None,
                installed: None,
                builds: Vec::new(),
                installing: false,
                resign_expiry: None,
                now: Utc::now(),
            }
        }
        fn decide(&self) -> Decision<'_> {
            decide(&Inputs {
                app: &self.app,
                device: self.device.as_ref(),
                installed: self.installed.as_ref(),
                builds: &self.builds,
                installing: self.installing,
                resign_expiry: self.resign_expiry,
                now: self.now,
            })
        }
        fn release(&self, i: usize) -> Release {
            self.app.releases[i].clone()
        }
    }

    fn ios(v: &str) -> Option<DeviceProfile> {
        Some(DeviceProfile {
            os_version: v.into(),
            platform: "ios".into(),
            family: 1,
        })
    }

    #[test]
    fn get_preparing_retry_install() {
        let mut c = Case::new();
        let d = c.decide();
        assert_eq!((d.action, d.target), (Action::Get, Some(0)));
        assert!(d.build.is_none() && d.open_url.is_none());

        c.builds = vec![build_for(&c.release(0), BuildStatus::Downloading, c.now)];
        assert_eq!(c.decide().action, Action::Preparing);
        c.builds[0].status = BuildStatus::Failed;
        assert_eq!(c.decide().action, Action::Retry);
        c.builds[0].status = BuildStatus::Ready;
        let d = c.decide();
        assert_eq!(d.action, Action::Install);
        assert_eq!(d.install_build.unwrap().id, c.builds[0].id);

        // A build of another release does not count for the target.
        c.builds = vec![build_for(&c.release(1), BuildStatus::Ready, c.now)];
        assert_eq!(c.decide().action, Action::Get);
        // Expired builds cannot be installed.
        c.builds = vec![build_for(&c.release(0), BuildStatus::Ready, c.now)];
        c.builds[0].expires_at = Some(c.now - Duration::hours(1));
        assert_eq!(c.decide().action, Action::Get);
    }

    #[test]
    fn device_os_selects_target_and_unavailable() {
        let mut c = Case::new();
        c.device = ios("15.0");
        assert_eq!(c.decide().target, Some(1));
        c.app.releases[1].min_os = Some("15.1".into());
        assert_eq!(c.decide().action, Action::Unavailable);
        c.installed = Some(install_of(&c.release(1), "kiwi", c.now));
        assert_eq!(c.decide().action, Action::Open);
        c.app.unavailable = Some("Paid".into());
        c.device = None;
        c.installed = None;
        assert_eq!(c.decide().action, Action::Unavailable);
    }

    #[test]
    fn open_update_and_installing() {
        let mut c = Case::new();
        c.installed = Some(install_of(&c.release(0), "kiwi", c.now));
        let d = c.decide();
        assert_eq!(d.action, Action::Open);
        assert_eq!(d.open_url.as_deref(), Some("kiwi-0123456789ab://"));
        assert_eq!(d.update_reason, None);

        c.installed = Some(install_of(&c.release(1), "kiwi", c.now));
        let d = c.decide();
        assert_eq!(
            (d.action, d.update_reason),
            (Action::Update, Some(UpdateReason::NewVersion))
        );
        assert!(d.install_build.is_none());

        c.builds = vec![build_for(&c.release(0), BuildStatus::Ready, c.now)];
        let d = c.decide();
        assert_eq!(d.action, Action::Update);
        assert!(d.install_build.is_some());
        c.installing = true;
        let d = c.decide();
        assert_eq!(d.action, Action::Installing);
        assert!(d.install_build.is_some());

        // Installing never overrides open or get.
        c.installed = Some(install_of(&c.release(0), "kiwi", c.now));
        assert_eq!(c.decide().action, Action::Open);
        c.installed = None;
        c.builds.clear();
        assert_eq!(c.decide().action, Action::Get);
    }

    #[test]
    fn older_and_device_installs() {
        let mut c = Case::new();
        // Installed newer than anything eligible: open.
        c.device = ios("15.0");
        c.installed = Some(install_of(&c.release(0), "kiwi", c.now));
        assert_eq!(c.decide().action, Action::Open);
        // Seen on the device without Kiwi: no URL scheme, no open_url.
        c.device = None;
        let mut device_install = install_of(&c.release(1), "device", c.now);
        device_install.release = None;
        device_install.url_scheme = None;
        c.installed = Some(device_install);
        let d = c.decide();
        assert_eq!(d.action, Action::Update);
        assert!(d.open_url.is_none());
        c.installed.as_mut().unwrap().version = "2.0".into();
        assert_eq!(c.decide().action, Action::Open);
    }

    #[test]
    fn resign_when_expiring_and_a_later_signature_is_possible() {
        let mut c = Case::new();
        let mut inst = install_of(&c.release(0), "kiwi", c.now);
        inst.expires_at = Some(c.now + Duration::days(20));
        c.installed = Some(inst);
        assert_eq!(c.decide().action, Action::Open);
        c.resign_expiry = Some(c.now + Duration::days(20));
        assert_eq!(c.decide().action, Action::Open);
        c.resign_expiry = Some(c.now + Duration::days(300));
        let d = c.decide();
        assert_eq!(
            (d.action, d.update_reason),
            (Action::Update, Some(UpdateReason::Resign))
        );
        // The build that is installed does not extend the expiry.
        let mut old = build_for(&c.release(0), BuildStatus::Ready, c.now);
        old.expires_at = Some(c.now + Duration::days(20));
        c.builds = vec![old];
        let d = c.decide();
        assert_eq!(d.action, Action::Update);
        assert!(d.install_build.is_none());
        let mut fresh = build_for(&c.release(0), BuildStatus::Ready, c.now);
        fresh.expires_at = Some(c.now + Duration::days(300));
        c.builds.insert(0, fresh);
        assert!(c.decide().install_build.is_some());
        // Not expiring yet.
        c.installed.as_mut().unwrap().expires_at = Some(c.now + Duration::days(40));
        assert_eq!(c.decide().action, Action::Open);
        // Device installs are never re-signed.
        c.installed.as_mut().unwrap().expires_at = Some(c.now + Duration::days(2));
        c.installed.as_mut().unwrap().origin = "device".into();
        assert_eq!(c.decide().action, Action::Open);
    }
}
