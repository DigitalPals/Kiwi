//! Android devices beyond the browser (architecture.md "Android devices"): Kiwi's own
//! Android app, which Kiwi serves to them, and the installed packages that app reports.

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Kiwi,
    catalog::CatalogApp,
    db::{InstallRow, Time},
    devices::{INSTALL_GRACE, reconcile},
    error::{ApiError, ApiResult},
};

/// The app's file and its description in `<data>/android-app/`.
pub const APK_FILE: &str = "kiwi.apk";
pub const META_FILE: &str = "kiwi.json";

/// Kiwi's Android app as `kiwi.json` describes it. Whatever provides the app writes the
/// file next to `kiwi.apk`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AndroidApp {
    pub package: String,
    pub version_name: String,
    pub version_code: u64,
    pub sha256: String,
    pub size: u64,
}

/// `Me.android_app` (api.md).
#[derive(Debug, Serialize)]
pub struct AndroidAppJson {
    pub package: String,
    pub version_name: String,
    pub version_code: u64,
    pub size: u64,
    pub install_url: String,
}

impl AndroidApp {
    pub fn json(&self, public_url: &str) -> AndroidAppJson {
        AndroidAppJson {
            package: self.package.clone(),
            version_name: self.version_name.clone(),
            version_code: self.version_code,
            size: self.size,
            install_url: format!("{}/android/{APK_FILE}", public_url.trim_end_matches('/')),
        }
    }

    /// The name a browser saves the app under, e.g. `Kiwi-1.2.0.apk`.
    pub fn file_name(&self) -> String {
        let version: String = self
            .version_name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            .take(48)
            .collect();
        match version.trim_matches('.') {
            "" => "Kiwi.apk".into(),
            v => format!("Kiwi-{v}.apk"),
        }
    }
}

/// An Android application id: dot-separated names of letters, digits and underscores,
/// each starting with a letter, at least two of them.
pub fn valid_package(package: &str) -> bool {
    package.len() <= 255
        && package.split('.').count() >= 2
        && package.split('.').all(|part| {
            part.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}

fn valid_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The largest integer JavaScript represents exactly.
const MAX_VERSION_CODE: u64 = (1 << 53) - 1;

/// Reads `kiwi.json` and checks it against `kiwi.apk`: `Ok(None)` when either is missing,
/// an error when they disagree or the description is not valid.
pub fn load(dir: &Path) -> Result<Option<AndroidApp>, String> {
    let (meta, apk) = (dir.join(META_FILE), dir.join(APK_FILE));
    let text = match std::fs::read(&meta) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{META_FILE}: {e}")),
    };
    if text.len() > 64 * 1024 {
        return Err(format!("{META_FILE} is too large"));
    }
    let mut app: AndroidApp =
        serde_json::from_slice(&text).map_err(|e| format!("{META_FILE}: {e}"))?;
    app.sha256 = app.sha256.to_ascii_lowercase();
    app.version_name = app.version_name.trim().to_owned();
    if !valid_package(&app.package) {
        return Err(format!("{META_FILE}: invalid package {:?}", app.package));
    }
    if app.version_name.is_empty()
        || app.version_name.chars().count() > 64
        || app.version_name.chars().any(char::is_control)
    {
        return Err(format!("{META_FILE}: invalid version_name"));
    }
    if !(1..=MAX_VERSION_CODE).contains(&app.version_code) {
        return Err(format!("{META_FILE}: invalid version_code"));
    }
    if !valid_sha256(&app.sha256) {
        return Err(format!("{META_FILE}: sha256 must be 64 hex digits"));
    }
    let mut file = match std::fs::File::open(&apk) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{APK_FILE}: {e}")),
    };
    let len = file
        .metadata()
        .map_err(|e| format!("{APK_FILE}: {e}"))?
        .len();
    if len == 0 || len != app.size {
        return Err(format!(
            "{APK_FILE} has {len} bytes, {META_FILE} says {}",
            app.size
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 256 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(e) => return Err(format!("{APK_FILE}: {e}")),
        }
    }
    if hex::encode(hasher.finalize()) != app.sha256 {
        return Err(format!(
            "{APK_FILE} does not match the sha256 in {META_FILE}"
        ));
    }
    Ok(Some(app))
}

/// Size and modification time of a file, to notice when it changes.
type Stamp = Option<(u64, SystemTime)>;

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// What was found for the stamps of `kiwi.json` and `kiwi.apk`.
type Checked = ((Stamp, Stamp), Option<AndroidApp>);

/// The checked app, read again only when one of its two files changes (checking it
/// hashes the whole APK).
#[derive(Default)]
pub struct AndroidAppCache(tokio::sync::Mutex<Option<Checked>>);

impl AndroidAppCache {
    /// The app Kiwi can serve from `dir`, or `None`.
    pub async fn get(&self, dir: &Path) -> Option<AndroidApp> {
        let mut cached = self.0.lock().await;
        let stamps = (stamp(&dir.join(META_FILE)), stamp(&dir.join(APK_FILE)));
        if let Some((seen, app)) = cached.as_ref()
            && *seen == stamps
        {
            return app.clone();
        }
        let app = match (&stamps, dir.to_owned()) {
            ((None, _) | (_, None), _) => None,
            (_, dir) => match tokio::task::spawn_blocking(move || load(&dir)).await {
                Ok(Ok(app)) => app,
                Ok(Err(e)) => {
                    tracing::warn!("not offering Kiwi's Android app: {e}");
                    None
                }
                Err(e) => {
                    tracing::warn!("checking Kiwi's Android app failed: {e}");
                    None
                }
            },
        };
        *cached = Some((stamps, app.clone()));
        app
    }
}

// ---- installed packages ----

/// One installed package, as Kiwi's Android app reports it (`PUT /api/me/packages`).
/// Unknown fields are ignored so a newer app can report more.
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub package: String,
    #[serde(default)]
    pub version_name: Option<String>,
    pub version_code: i64,
    /// SHA-256 of each signing certificate (DER), in hex.
    #[serde(default)]
    pub signer_sha256: Vec<String>,
    #[serde(default)]
    pub installer: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageReport {
    pub packages: Vec<Package>,
}

/// A report lists at most this many packages.
pub const MAX_PACKAGES: usize = 5000;

/// `<data>/automation/android/signer.json`: the SHA-256 of the certificate of the key the
/// Android runner signs with, written whenever the runner loads the key.
pub fn signer_file(data: &Path) -> PathBuf {
    data.join("automation/android/signer.json")
}

/// The certificate SHA-256 (hex, lowercase) in the runner's `signer.json`, if any.
pub fn recorded_signer(data: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct Signer {
        sha256: String,
    }
    std::fs::read(signer_file(data))
        .ok()
        .and_then(|text| serde_json::from_slice::<Signer>(&text).ok())
        .map(|s| s.sha256.to_ascii_lowercase())
        .filter(|s| valid_sha256(s))
}

/// The SHA-256 (hex) of the certificate Kiwi signs Android apps with: from the runner's
/// `signer.json`, else from the provenance of a published Android build (Kiwis that built
/// before the runner wrote the file). `None` before the first Android build.
pub fn kiwi_signer(kiwi: &Kiwi, android_apps: &[CatalogApp]) -> Option<String> {
    #[derive(Deserialize)]
    struct Provenance {
        signer: String,
    }
    let data = &kiwi.config.data_dir;
    let normal = |s: String| Some(s.to_ascii_lowercase()).filter(|s| valid_sha256(s));
    recorded_signer(data).or_else(|| {
        android_apps
            .iter()
            .flat_map(|app| &app.app().releases)
            .filter_map(|release| release.sha256.as_deref())
            .filter(|sha| valid_sha256(sha))
            .find_map(|sha| {
                let path = data.join(format!("automation/artifacts/{sha}.json"));
                let text = std::fs::read(path).ok()?;
                normal(serde_json::from_slice::<Provenance>(&text).ok()?.signer)
            })
    })
}

/// The Android apps in Kiwi's catalog (Kiwi Builds' Android releases).
pub fn catalog_apps(kiwi: &Kiwi) -> Vec<CatalogApp> {
    kiwi.catalog
        .apps(None, None)
        .into_iter()
        .filter(|a| crate::builds::android_app(a.app()))
        .collect()
}

/// What a package report changes: install rows to save, and the packages of Kiwi's
/// Android apps installed with another signature.
#[derive(Debug, Default, PartialEq)]
pub struct Reconciled {
    pub installs: Vec<InstallRow>,
    pub foreign: Vec<String>,
}

/// Reconciles a device's installs with the packages its Android app reports, like a
/// devicectl sync ([`reconcile`]): a package of one of Kiwi's Android apps signed with
/// `signer` is a verified install; one signed with another key is foreign (Kiwi's build
/// cannot be installed over it); an install whose package is gone is marked removed. Without
/// a known `signer` nothing is verified or foreign, and only removals are recorded.
pub fn reconcile_packages(
    device_id: &str,
    installs: &[InstallRow],
    packages: &[Package],
    android_apps: &[CatalogApp],
    signer: Option<&str>,
    now: Time,
) -> Reconciled {
    let ours = |p: &Package| {
        signer.is_some_and(|signer| {
            p.signer_sha256
                .iter()
                .any(|s| s.replace(':', "").eq_ignore_ascii_case(signer))
        })
    };
    let relevant: HashSet<&str> = android_apps
        .iter()
        .map(|a| a.app().bundle_id.as_str())
        .chain(installs.iter().map(|i| i.signed_bundle_id.as_str()))
        .collect();
    let present: Vec<&Package> = packages
        .iter()
        .filter(|p| relevant.contains(p.package.as_str()))
        .collect();
    let apps: Vec<kiwi_device::DeviceApp> = present
        .iter()
        .filter(|p| signer.is_none() || ours(p))
        .map(|p| kiwi_device::DeviceApp {
            bundle_id: p.package.clone(),
            name: None,
            version: p.version_name.clone().filter(|v| !v.trim().is_empty()),
            build: Some(p.version_code.to_string()),
            removable: None,
            path: None,
        })
        .collect();
    let mut rows = reconcile(device_id, installs, &apps, android_apps, None, now);
    if signer.is_none() {
        // A package may be Kiwi's or not: only an install whose package is gone changes.
        rows.retain(|row| row.removed_at.is_some());
    }
    let mut foreign: Vec<String> = match signer {
        Some(_) => present
            .iter()
            .filter(|p| !ours(p))
            .map(|p| p.package.clone())
            .collect(),
        None => Vec::new(),
    };
    foreign.sort();
    foreign.dedup();
    Reconciled {
        installs: rows,
        foreign,
    }
}

/// The builds whose last download on a device Kiwi's Android app made (its `Kiwi-Android/`
/// User-Agent). The app installs what it downloads at once and reports the packages after
/// the install, so a report of it that lacks the package means the install failed. A
/// browser download waits until the owner opens the APK in Android's installer, which can
/// be after a report.
pub fn app_downloads(kiwi: &Kiwi, device_id: &str) -> ApiResult<HashSet<String>> {
    Ok(kiwi
        .db
        .device_attempts(device_id)?
        .into_iter()
        .map(|(_, attempt)| attempt)
        .filter(|a| {
            a.download_completed_at.is_some()
                && a.user_agent
                    .as_deref()
                    .is_some_and(kiwi_device::is_native_android)
        })
        .map(|a| a.build_id)
        .collect())
}

/// An install a package report made at `reported_at` speaks for once its grace period is
/// over: an unverified download by Kiwi's Android app (see [`app_downloads`]) that
/// completed before the report.
fn settles(install: &InstallRow, reported_at: Time, app_builds: &HashSet<String>) -> bool {
    install.origin == "kiwi"
        && !install.verified
        && install.removed_at.is_none()
        && install.installed_at <= reported_at
        && install
            .build_id
            .as_ref()
            .is_some_and(|b| app_builds.contains(b))
}

/// [`settles`], and its grace period is not over at `now`.
fn awaits_settling(
    install: &InstallRow,
    reported_at: Time,
    app_builds: &HashSet<String>,
    now: Time,
) -> bool {
    settles(install, reported_at, app_builds) && now - install.installed_at <= INSTALL_GRACE
}

/// Installs whose download by Kiwi's Android app ([`app_downloads`], `app_builds`)
/// completed in the grace period before a package report that did not list their package:
/// once the grace period is over, the report counts for them too (Android's install can
/// fail after the download, and the app reports at once). Returns the rows to save;
/// [`reconcile_packages`] decides what changes.
#[allow(clippy::too_many_arguments)]
pub fn settle_report(
    device_id: &str,
    installs: &[InstallRow],
    reported_at: Time,
    packages: &[Package],
    android_apps: &[CatalogApp],
    signer: Option<&str>,
    app_builds: &HashSet<String>,
    now: Time,
) -> Vec<InstallRow> {
    let waiting: Vec<InstallRow> = installs
        .iter()
        .filter(|i| settles(i, reported_at, app_builds) && now - i.installed_at > INSTALL_GRACE)
        .cloned()
        .collect();
    if waiting.is_empty() {
        return Vec::new();
    }
    let ids: HashSet<&str> = waiting
        .iter()
        .map(|i| i.signed_bundle_id.as_str())
        .collect();
    reconcile_packages(device_id, &waiting, packages, android_apps, signer, now)
        .installs
        .into_iter()
        .filter(|row| ids.contains(row.signed_bundle_id.as_str()))
        .collect()
}

/// A package report [`settle_reports`] still needs: when it was made and the packages it
/// listed of the installs waiting for it.
#[derive(Debug, Clone)]
struct KeptReport {
    at: Time,
    packages: Vec<Package>,
}

/// Package reports in memory, for [`settle_reports`]: of each device only the last one,
/// only while an install is waiting for it ([`awaits_settling`]), and only the packages of
/// those installs.
#[derive(Default)]
pub struct PackageReports(Mutex<HashMap<String, KeptReport>>);

impl PackageReports {
    /// Keeps what settling needs of a device's report made at `at`, replacing the one
    /// before; nothing when no install waits for it.
    fn keep(&self, device_id: &str, at: Time, packages: Vec<Package>, waiting: &[InstallRow]) {
        let mut reports = self.0.lock().unwrap();
        if waiting.is_empty() {
            reports.remove(device_id);
            return;
        }
        let packages = packages
            .into_iter()
            .filter(|p| waiting.iter().any(|i| i.signed_bundle_id == p.package))
            .collect();
        reports.insert(device_id.to_owned(), KeptReport { at, packages });
    }

    /// Forgets the device's report made at `at` (not a newer one that replaced it).
    fn forget(&self, device_id: &str, at: Time) {
        let mut reports = self.0.lock().unwrap();
        if reports.get(device_id).is_some_and(|r| r.at == at) {
            reports.remove(device_id);
        }
    }

    /// The devices with a report kept, and how many packages each keeps.
    pub fn kept(&self) -> Vec<(String, usize)> {
        let mut kept: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(device, report)| (device.clone(), report.packages.len()))
            .collect();
        kept.sort();
        kept
    }
}

/// Applies each device's last package report to the installs whose grace period ended since
/// (see [`settle_report`]). The scheduler runs it every minute. A report no install waits
/// for any more, or of a device that was deleted, is forgotten.
pub fn settle_reports(kiwi: &Kiwi, now: Time) -> ApiResult<()> {
    let reports: Vec<(String, KeptReport)> = kiwi
        .package_reports
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|(device, report)| (device.clone(), report.clone()))
        .collect();
    let mut android_apps = None;
    let mut changed = false;
    for (device_id, report) in reports {
        if kiwi.db.device(&device_id)?.is_none() {
            kiwi.package_reports.forget(&device_id, report.at);
            continue;
        }
        let app_builds = app_downloads(kiwi, &device_id)?;
        let apps = android_apps.get_or_insert_with(|| catalog_apps(kiwi));
        let signer = kiwi_signer(kiwi, apps);
        let mut waiting = false;
        let rows = kiwi.db.change_installs(&device_id, |installs| {
            waiting = installs
                .iter()
                .any(|i| awaits_settling(i, report.at, &app_builds, now));
            settle_report(
                &device_id,
                installs,
                report.at,
                &report.packages,
                apps,
                signer.as_deref(),
                &app_builds,
                now,
            )
        })?;
        for row in &rows {
            tracing::info!(device = %device_id, package = %row.signed_bundle_id, removed = row.removed_at.is_some(), "settled an install with the last package report");
            changed = true;
        }
        if !waiting {
            kiwi.package_reports.forget(&device_id, report.at);
        }
    }
    if changed {
        kiwi.kick.notify_one();
    }
    Ok(())
}

/// Saves a package report of an Android device (`PUT /api/me/packages`). The installs are
/// read, reconciled and written in one transaction, so a download completing meanwhile is
/// not overwritten.
pub fn record_packages(kiwi: &Kiwi, device_id: &str, report: PackageReport) -> ApiResult<()> {
    if report.packages.len() > MAX_PACKAGES {
        return Err(ApiError::bad_request(format!(
            "A report can list at most {MAX_PACKAGES} packages."
        )));
    }
    let now = chrono::Utc::now();
    let packages: Vec<Package> = report
        .packages
        .into_iter()
        .filter(|p| valid_package(&p.package))
        .map(|mut p| {
            p.version_name = p.version_name.map(|v| v.trim().chars().take(64).collect());
            p
        })
        .collect();
    let android_apps = catalog_apps(kiwi);
    let signer = kiwi_signer(kiwi, &android_apps);
    let app_builds = app_downloads(kiwi, device_id)?;
    let mut waiting = Vec::new();
    let saved = kiwi.db.save_package_report(device_id, now, |installs| {
        let result = reconcile_packages(
            device_id,
            installs,
            &packages,
            &android_apps,
            signer.as_deref(),
            now,
        );
        // The installs as this report leaves them that a later settling may change.
        waiting = installs
            .iter()
            .map(|i| {
                result
                    .installs
                    .iter()
                    .find(|r| r.signed_bundle_id == i.signed_bundle_id)
                    .unwrap_or(i)
            })
            .filter(|i| awaits_settling(i, now, &app_builds, now))
            .cloned()
            .collect();
        (result.installs, result.foreign)
    })?;
    kiwi.package_reports
        .keep(device_id, now, packages, &waiting);
    if !saved.is_empty() {
        kiwi.kick.notify_one();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{install_of, release};
    use chrono::Utc;
    use std::sync::Arc;

    const SIGNER: &str = "c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00";
    const OTHER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const PACKAGE: &str = "app.morphe.android.youtube";

    fn android_catalog() -> Vec<CatalogApp> {
        let mut app = crate::testutil::app(&[("21.30.216", None)]);
        app.bundle_id = PACKAGE.into();
        app.name = "YouTube".into();
        for r in &mut app.releases {
            r.platforms = vec!["Android".into()];
            r.build = Some("1561187872".into());
        }
        let source = Arc::new(crate::testutil::source_row("kiwi-youtube", vec![app]));
        vec![CatalogApp { source, index: 0 }]
    }

    fn package(name: &str, signer: &str) -> Package {
        Package {
            package: name.into(),
            version_name: Some("21.30.216".into()),
            version_code: 1561187872,
            signer_sha256: vec![signer.to_uppercase()],
            installer: Some("com.android.packageinstaller".into()),
        }
    }

    fn kiwi_install(age_minutes: i64) -> InstallRow {
        let now = Utc::now();
        let mut install = install_of(
            &release("21.30.216", None),
            "kiwi",
            now - chrono::Duration::minutes(age_minutes),
        );
        install.signed_bundle_id = PACKAGE.into();
        install.bundle_id = PACKAGE.into();
        install.source_id = "kiwi-youtube".into();
        install.build_id = Some(BUILD.into());
        install
    }

    const BUILD: &str = "build-youtube";

    /// The builds Kiwi's Android app downloaded: [`BUILD`], or none (a browser did).
    fn app_builds(by_app: bool) -> HashSet<String> {
        by_app.then(|| BUILD.to_owned()).into_iter().collect()
    }

    #[test]
    fn packages_signed_by_kiwi_are_verified_installs() {
        let now = Utc::now();
        let catalog = android_catalog();
        let packages = [
            package(PACKAGE, SIGNER),
            package("com.example.other", OTHER),
        ];
        // A completed download becomes a verified install.
        let r = reconcile_packages(
            "ts:test",
            &[kiwi_install(60)],
            &packages,
            &catalog,
            Some(SIGNER),
            now,
        );
        assert!(r.foreign.is_empty());
        assert_eq!(r.installs.len(), 1);
        let row = &r.installs[0];
        assert!(row.verified && row.removed_at.is_none());
        assert_eq!(row.origin, "kiwi");
        assert_eq!(row.version, "21.30.216");
        assert!(
            row.release.is_some(),
            "same version: the release stays known"
        );
        // Without a record, the package is recorded as seen on the device.
        let r = reconcile_packages("ts:test", &[], &packages, &catalog, Some(SIGNER), now);
        assert_eq!(r.installs.len(), 1);
        let row = &r.installs[0];
        assert_eq!(
            (row.signed_bundle_id.as_str(), row.origin.as_str()),
            (PACKAGE, "device")
        );
        assert!(row.verified);
        assert_eq!(row.build.as_deref(), Some("1561187872"));
    }

    #[test]
    fn packages_signed_by_another_key_are_foreign() {
        let now = Utc::now();
        let catalog = android_catalog();
        let packages = [package(PACKAGE, OTHER)];
        // Nothing of Kiwi's is installed: the download did not become an install.
        let r = reconcile_packages(
            "ts:test",
            &[kiwi_install(60)],
            &packages,
            &catalog,
            Some(SIGNER),
            now,
        );
        assert_eq!(r.foreign, [PACKAGE]);
        assert_eq!(r.installs.len(), 1);
        assert!(r.installs[0].removed_at.is_some() && !r.installs[0].verified);
        // ...unless it completed just now: Android may still be installing it.
        let r = reconcile_packages(
            "ts:test",
            &[kiwi_install(2)],
            &packages,
            &catalog,
            Some(SIGNER),
            now,
        );
        assert_eq!(r.foreign, [PACKAGE]);
        assert!(r.installs.is_empty());
        // Without a record nothing is recorded, but the package is still foreign.
        let r = reconcile_packages("ts:test", &[], &packages, &catalog, Some(SIGNER), now);
        assert!(r.installs.is_empty());
        assert_eq!(r.foreign, [PACKAGE]);
    }

    #[test]
    fn verified_installs_whose_package_is_gone_are_removed() {
        let now = Utc::now();
        let catalog = android_catalog();
        let mut verified = kiwi_install(1);
        verified.verified = true;
        let r = reconcile_packages(
            "ts:test",
            std::slice::from_ref(&verified),
            &[package("com.example.other", OTHER)],
            &catalog,
            Some(SIGNER),
            now,
        );
        assert_eq!(r.installs.len(), 1);
        assert_eq!(r.installs[0].removed_at, Some(now));
        assert!(r.foreign.is_empty());
        // A reinstall brings it back.
        let r = reconcile_packages(
            "ts:test",
            &r.installs,
            &[package(PACKAGE, SIGNER)],
            &catalog,
            Some(SIGNER),
            now,
        );
        assert_eq!(r.installs[0].removed_at, None);
        assert!(r.installs[0].verified);
    }

    #[test]
    fn a_completed_download_missing_from_a_report_is_removed_after_the_grace_period() {
        let now = Utc::now();
        let catalog = android_catalog();
        let others = [package("com.example.other", OTHER)];
        for signer in [Some(SIGNER), None] {
            // Android's install failed after the download: the report lacks the package.
            let r = reconcile_packages(
                "ts:test",
                &[kiwi_install(11)],
                &others,
                &catalog,
                signer,
                now,
            );
            assert_eq!(r.installs.len(), 1, "{signer:?}");
            let row = &r.installs[0];
            assert_eq!(row.removed_at, Some(now));
            assert!(!row.verified);
            // Within the grace period Android may still be installing it.
            let r = reconcile_packages(
                "ts:test",
                &[kiwi_install(9)],
                &others,
                &catalog,
                signer,
                now,
            );
            assert!(r.installs.is_empty(), "{signer:?}");
        }
    }

    #[test]
    fn a_report_inside_the_grace_period_counts_once_it_is_over() {
        let reported = Utc::now();
        let catalog = android_catalog();
        let install = kiwi_install(3); // completed 3 minutes before the report
        let others = [package("com.example.other", OTHER)];
        let by_app = app_builds(true);
        let settle = |installs: &[InstallRow], packages: &[Package], minutes: i64| {
            settle_report(
                "ts:test",
                installs,
                reported,
                packages,
                &catalog,
                Some(SIGNER),
                &by_app,
                reported + chrono::Duration::minutes(minutes),
            )
        };
        // The report itself changed nothing...
        let r = reconcile_packages(
            "ts:test",
            std::slice::from_ref(&install),
            &others,
            &catalog,
            Some(SIGNER),
            reported,
        );
        assert!(r.installs.is_empty());
        // ...and still doesn't while the grace period runs.
        assert!(settle(std::slice::from_ref(&install), &others, 6).is_empty());
        // Once it is over, the package the report lacked counts as not installed.
        let rows = settle(std::slice::from_ref(&install), &others, 8);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].signed_bundle_id, PACKAGE);
        assert!(rows[0].removed_at.is_some() && !rows[0].verified);
        // A copy with another signature does not make it installed either.
        let foreign = [package(PACKAGE, OTHER)];
        assert!(
            settle(std::slice::from_ref(&install), &foreign, 8)[0]
                .removed_at
                .is_some()
        );
        // An older version of Kiwi's build that stayed installed is what the phone has.
        let mut older = package(PACKAGE, SIGNER);
        older.version_name = Some("21.29.0".into());
        let rows = settle(std::slice::from_ref(&install), &[older], 8);
        assert!(rows[0].verified && rows[0].removed_at.is_none());
        assert_eq!(rows[0].version, "21.29.0");
        // A report from before the download says nothing about it.
        let later = InstallRow {
            installed_at: reported + chrono::Duration::minutes(1),
            ..install.clone()
        };
        assert!(settle(&[later], &others, 30).is_empty());
        // Verified and removed installs are left to the next report.
        let verified = InstallRow {
            verified: true,
            ..install.clone()
        };
        assert!(settle(&[verified], &others, 30).is_empty());
        let removed = InstallRow {
            removed_at: Some(reported),
            ..install
        };
        assert!(settle(&[removed], &others, 30).is_empty());
    }

    #[test]
    fn a_browser_download_is_not_settled_with_an_earlier_report() {
        // The owner downloads the APK in the browser; Kiwi's app reports before they open
        // it in Android's installer, so the report lacks the package.
        let reported = Utc::now();
        let catalog = android_catalog();
        let install = kiwi_install(3);
        let others = [package("com.example.other", OTHER)];
        let later = reported + chrono::Duration::minutes(30);
        let settle = |by_app: bool| {
            settle_report(
                "ts:test",
                std::slice::from_ref(&install),
                reported,
                &others,
                &catalog,
                Some(SIGNER),
                &app_builds(by_app),
                later,
            )
        };
        // The grace period is long over, and the install still stands: only a report made
        // after it decides.
        assert!(settle(false).is_empty());
        // Downloaded by the app, which installs at once: the report counts.
        assert!(settle(true)[0].removed_at.is_some());
        // An install without a build (seen on the device) is not Kiwi's download.
        let seen = InstallRow {
            build_id: None,
            ..install.clone()
        };
        let rows = settle_report(
            "ts:test",
            &[seen],
            reported,
            &others,
            &catalog,
            Some(SIGNER),
            &app_builds(true),
            later,
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn only_reports_an_install_waits_for_are_kept() {
        let reports = PackageReports::default();
        let now = Utc::now();
        let mut many: Vec<Package> = (0..MAX_PACKAGES)
            .map(|i| package(&format!("com.example.app{i}"), OTHER))
            .collect();
        many.push(package(PACKAGE, SIGNER));
        // Nothing waits: nothing is kept.
        reports.keep("ts:a", now, many.clone(), &[]);
        assert!(reports.kept().is_empty());
        // Only the package of the waiting install is kept.
        reports.keep("ts:a", now, many.clone(), &[kiwi_install(1)]);
        assert_eq!(reports.kept(), [("ts:a".to_owned(), 1)]);
        // A later report without a waiting install replaces it with nothing.
        reports.keep("ts:a", now, many, &[]);
        assert!(reports.kept().is_empty());
        // Forgetting a report keeps a newer one.
        let later = now + chrono::Duration::seconds(1);
        reports.keep("ts:a", later, Vec::new(), &[kiwi_install(1)]);
        reports.forget("ts:a", now);
        assert_eq!(reports.kept(), [("ts:a".to_owned(), 0)]);
        reports.forget("ts:a", later);
        assert!(reports.kept().is_empty());
    }

    #[test]
    fn without_kiwis_signer_only_removals_count() {
        let now = Utc::now();
        let catalog = android_catalog();
        let r = reconcile_packages(
            "ts:test",
            &[kiwi_install(60)],
            &[package(PACKAGE, OTHER)],
            &catalog,
            None,
            now,
        );
        assert_eq!(r, Reconciled::default());
        let r = reconcile_packages("ts:test", &[kiwi_install(60)], &[], &catalog, None, now);
        assert_eq!(r.installs.len(), 1);
        assert!(r.installs[0].removed_at.is_some());
    }

    #[test]
    fn package_names() {
        for good in [PACKAGE, "io.github.digitalpals.kiwi", "com.a_b.C9"] {
            assert!(valid_package(good), "{good}");
        }
        for bad in [
            "",
            "kiwi",
            "com..kiwi",
            "com.9kiwi",
            "com.kiwi/../x",
            "com.kiwi.",
        ] {
            assert!(!valid_package(bad), "{bad}");
        }
    }

    fn write_app(dir: &Path, bytes: &[u8], meta: serde_json::Value) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(APK_FILE), bytes).unwrap();
        std::fs::write(dir.join(META_FILE), meta.to_string()).unwrap();
    }

    #[test]
    fn the_app_is_offered_only_when_its_description_matches_the_file() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("android-app");
        assert_eq!(load(&dir), Ok(None));
        let bytes = b"PK\x03\x04kiwi".to_vec();
        let sha = hex::encode(Sha256::digest(&bytes));
        let meta = serde_json::json!({
            "package": "io.github.digitalpals.kiwi", "version_name": "1.2.0",
            "version_code": 12, "sha256": sha.to_uppercase(), "size": bytes.len(),
            "built_by": "ignored",
        });
        write_app(&dir, &bytes, meta.clone());
        let app = load(&dir).unwrap().unwrap();
        assert_eq!(app.sha256, sha);
        assert_eq!(app.file_name(), "Kiwi-1.2.0.apk");
        let json = app.json("https://kiwi.test/");
        assert_eq!(json.install_url, "https://kiwi.test/android/kiwi.apk");
        assert_eq!((json.version_code, json.size), (12, bytes.len() as u64));

        let mut wrong = meta.clone();
        wrong["size"] = 3.into();
        write_app(&dir, &bytes, wrong);
        assert!(load(&dir).unwrap_err().contains("bytes"));
        let mut wrong = meta.clone();
        wrong["sha256"] = OTHER.into();
        write_app(&dir, &bytes, wrong);
        assert!(load(&dir).unwrap_err().contains("sha256"));
        for (key, value) in [
            ("package", serde_json::json!("kiwi")),
            ("version_code", serde_json::json!(0)),
            ("version_name", serde_json::json!(" ")),
            ("sha256", serde_json::json!("abc")),
        ] {
            let mut wrong = meta.clone();
            wrong[key] = value;
            write_app(&dir, &bytes, wrong);
            assert!(load(&dir).is_err(), "{key}");
        }
        write_app(&dir, &bytes, meta);
        std::fs::remove_file(dir.join(APK_FILE)).unwrap();
        assert_eq!(load(&dir), Ok(None));
    }

    #[tokio::test]
    async fn the_app_is_checked_again_when_a_file_changes() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("android-app");
        let cache = AndroidAppCache::default();
        assert!(cache.get(&dir).await.is_none());
        let bytes = b"PK first".to_vec();
        let meta = |bytes: &[u8], name: &str| {
            serde_json::json!({
                "package": "io.github.digitalpals.kiwi", "version_name": name,
                "version_code": 1, "sha256": hex::encode(Sha256::digest(bytes)),
                "size": bytes.len(),
            })
        };
        write_app(&dir, &bytes, meta(&bytes, "1.0"));
        assert_eq!(cache.get(&dir).await.unwrap().version_name, "1.0");
        // A new version replaces both files.
        let bytes = b"PK second version".to_vec();
        write_app(&dir, &bytes, meta(&bytes, "1.1"));
        assert_eq!(cache.get(&dir).await.unwrap().version_name, "1.1");
        std::fs::remove_file(dir.join(META_FILE)).unwrap();
        assert!(cache.get(&dir).await.is_none());
    }
}
