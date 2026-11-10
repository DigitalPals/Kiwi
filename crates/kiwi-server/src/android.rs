//! Android devices beyond the browser (architecture.md "Android devices"): Kiwi's own
//! Android app, which Kiwi serves to them, and the installed packages that app reports.

use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Kiwi,
    catalog::CatalogApp,
    db::{InstallRow, Time},
    devices::reconcile,
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

/// The SHA-256 (hex) of the certificate Kiwi signs Android apps with: from the runner's
/// `signer.json`, else from the provenance of a published Android build (Kiwis that built
/// before the runner wrote the file). `None` before the first Android build.
pub fn kiwi_signer(kiwi: &Kiwi, android_apps: &[CatalogApp]) -> Option<String> {
    #[derive(Deserialize)]
    struct Signer {
        sha256: String,
    }
    #[derive(Deserialize)]
    struct Provenance {
        signer: String,
    }
    let data = &kiwi.config.data_dir;
    let normal = |s: String| Some(s.to_ascii_lowercase()).filter(|s| valid_sha256(s));
    let from_file = std::fs::read(signer_file(data))
        .ok()
        .and_then(|text| serde_json::from_slice::<Signer>(&text).ok())
        .and_then(|s| normal(s.sha256));
    from_file.or_else(|| {
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

/// Saves a package report of an Android device (`PUT /api/me/packages`).
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
    let android_apps: Vec<CatalogApp> = kiwi
        .catalog
        .apps(None, None)
        .into_iter()
        .filter(|a| crate::builds::android_app(a.app()))
        .collect();
    let signer = kiwi_signer(kiwi, &android_apps);
    let installs = kiwi.db.installs(device_id, true)?;
    let result = reconcile_packages(
        device_id,
        &installs,
        &packages,
        &android_apps,
        signer.as_deref(),
        now,
    );
    kiwi.db
        .save_package_report(device_id, &result.installs, &result.foreign, now)?;
    if !result.installs.is_empty() {
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
        install
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
