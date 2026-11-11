//! Delivering Kiwi's own Android app (architecture.md "Kiwi's Android app"): the project
//! publishes it unsigned, and each Kiwi signs it with its own Android key and serves it from
//! `<data>/android-app/`.
//!
//! `automation/kiwi-app.json` pins the release Kiwi downloads (URL, SHA-256, size). An owner
//! who builds the app places unsigned APKs in `<data>/android-app/unsigned/` instead; the
//! newest of them wins. The Android runner's `kiwi-app` command signs the chosen APK and
//! writes `kiwi.apk` and `kiwi.json`, which records the input key it was given. Kiwi runs it
//! on Kiwi Builds' worker (one job at a time) while Kiwi Builds is on and Kiwi has an Android
//! device or an Android build, whenever that key or Kiwi's key changed.

use std::{
    path::Path,
    sync::{Arc, LazyLock},
    time::{Duration, UNIX_EPOCH},
};

use chrono::Utc;
use kiwi_source::CancellationToken;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    Kiwi,
    android::{self, AndroidApp, META_FILE},
    automation::{self, Invocation, Recipe, recipe},
    db::{Platform, Time},
    devices, owner,
};

/// The worker's id for this job; it names the runner's request file.
pub const JOB_ID: &str = "kiwi-app";
/// `automation/kiwi-app.json`, which Kiwi also writes next to the runner.
pub const PIN: &str = include_str!("../../../automation/kiwi-app.json");
/// The application id of Kiwi's Android app.
pub const PACKAGE: &str = "io.github.digitalpals.kiwi";
/// A failed attempt is repeated after this long unless its input changes first.
const RETRY: chrono::Duration = chrono::Duration::hours(1);
/// Downloading the release and the pinned tools, then signing.
const TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The release of Kiwi's Android app that `kiwi-app.json` pins.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version_name: String,
    pub version_code: u64,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    schema: u32,
    package: String,
    release: Option<Release>,
}

/// Reads a pin file; the runner checks the release (`android.py` `check_pin`) in more detail.
pub fn parse_pin(text: &str) -> Result<Option<Release>, String> {
    let pin: Pin = serde_json::from_str(text).map_err(|e| format!("kiwi-app.json: {e}"))?;
    if pin.schema != 1 || pin.package != PACKAGE {
        return Err(format!("kiwi-app.json must be schema 1 for {PACKAGE}"));
    }
    if let Some(release) = &pin.release
        && (!release.url.starts_with("https://github.com/")
            || !automation::valid_sha(&release.sha256)
            || release.size == 0
            || release.version_code == 0
            || release.version_name.is_empty())
    {
        return Err("kiwi-app.json: the pinned release is not valid".into());
    }
    Ok(pin.release)
}

static PINNED: LazyLock<Option<Release>> = LazyLock::new(|| {
    parse_pin(PIN).unwrap_or_else(|e| {
        tracing::error!("{e}");
        None
    })
});

/// The release this version of Kiwi pins, `None` before the first one.
pub fn pinned() -> Option<&'static Release> {
    PINNED.as_ref()
}

/// The unsigned APKs the owner placed in `dir` as (name, size, modified in ns), by name.
fn local_apks(dir: &Path) -> Vec<(String, u64, u128)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, u64, u128)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !name.to_ascii_lowercase().ends_with(".apk") {
                return None;
            }
            let meta = std::fs::metadata(entry.path())
                .ok()
                .filter(|m| m.is_file())?;
            let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
            Some((name, meta.len(), modified.as_nanos()))
        })
        .collect();
    found.sort();
    found
}

/// What the app Kiwi serves from `dir` is made from: a key over the pinned release and the
/// unsigned APKs in `dir/unsigned/`. `None` when there is nothing to sign.
pub fn input_key(dir: &Path, release: Option<&Release>) -> Option<String> {
    let local = local_apks(&dir.join("unsigned"));
    if local.is_empty() && release.is_none() {
        return None;
    }
    let input = json!({ "release": release.map(|r| &r.sha256), "local": local });
    Some(hex::encode(Sha256::digest(input.to_string())))
}

/// The fields of `kiwi.json` only the runner writes.
#[derive(Debug, Default, Deserialize)]
struct Made {
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    signer: Option<String>,
}

fn made(dir: &Path) -> Made {
    std::fs::read(dir.join(META_FILE))
        .ok()
        .filter(|text| text.len() <= 64 * 1024)
        .and_then(|text| serde_json::from_slice(&text).ok())
        .unwrap_or_default()
}

/// Whether the served `app` was made from `key` and signed with Kiwi's current key (when
/// the runner has recorded one).
fn current(app: Option<&AndroidApp>, made: &Made, key: &str, signer: Option<&str>) -> bool {
    app.is_some_and(|app| app.package == PACKAGE)
        && made.input.as_deref() == Some(key)
        && signer.is_none_or(|signer| {
            made.signer
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(signer))
        })
}

/// A failed attempt, shown as a warning until an attempt succeeds.
#[derive(Debug, Clone)]
pub struct Failure {
    key: String,
    at: Time,
    pub message: String,
}

/// The delivery's state on Kiwi Builds' worker.
#[derive(Debug, Default)]
pub struct Status {
    /// The input key of an attempt waiting for the worker.
    pending: Option<String>,
    running: bool,
    failure: Option<Failure>,
}

impl Status {
    /// Neither waiting for the worker nor running.
    pub fn is_idle(&self) -> bool {
        self.pending.is_none() && !self.running
    }
}

/// Whether Kiwi has a reason to offer its Android app: an Android device of the owner's
/// (other people's devices get nothing from Kiwi), or an Android app in Kiwi Builds.
fn wanted(kiwi: &Kiwi) -> bool {
    let builds = kiwi
        .automation
        .snapshot()
        .apps
        .iter()
        .any(|(id, app)| app.added && recipe(id).is_ok_and(Recipe::android));
    builds
        || kiwi.db.devices().is_ok_and(|devices| {
            devices.iter().any(|d| {
                devices::effective_platform(d) == Platform::Android && owner::is_owner(kiwi, d)
            })
        })
}

/// Queues signing the app when what Kiwi serves is not made from the current input. Runs at
/// startup and on the scheduler's cadence.
pub async fn tick(kiwi: &Arc<Kiwi>) {
    if !kiwi.settings().kiwi_builds || !wanted(kiwi) {
        return;
    }
    let dir = &kiwi.paths.android_app;
    let Some(key) = input_key(dir, pinned()) else {
        // Nothing to sign (the owner removed their APKs): nothing is failing either.
        kiwi.automation.kiwi_app.lock().unwrap().failure = None;
        return;
    };
    let app = kiwi.android_app.get(dir).await;
    let signer = android::recorded_signer(&kiwi.config.data_dir);
    let up_to_date = current(app.as_ref(), &made(dir), &key, signer.as_deref());
    {
        let mut status = kiwi.automation.kiwi_app.lock().unwrap();
        if up_to_date {
            status.failure = None;
            return;
        }
        if status.running
            || status.pending.is_some()
            || status
                .failure
                .as_ref()
                .is_some_and(|f| f.key == key && Utc::now() - f.at < RETRY)
        {
            return;
        }
        status.pending = Some(key);
    }
    automation::start_next(kiwi);
}

/// The waiting attempt's input key, now that the worker takes it.
pub(crate) fn take_pending(kiwi: &Kiwi) -> Option<String> {
    let mut status = kiwi.automation.kiwi_app.lock().unwrap();
    let key = status.pending.take()?;
    status.running = true;
    Some(key)
}

/// Drops a waiting attempt and stops a running one (Kiwi Builds was turned off).
pub(crate) fn cancel(kiwi: &Kiwi) {
    {
        let mut status = kiwi.automation.kiwi_app.lock().unwrap();
        status.pending = None;
        status.failure = None;
    }
    automation::cancel_running(kiwi, JOB_ID);
}

/// The last attempt's error, until an attempt succeeds.
pub fn failure(kiwi: &Kiwi) -> Option<Failure> {
    kiwi.automation.kiwi_app.lock().unwrap().failure.clone()
}

/// Runs the Android runner's `kiwi-app` command for `key` and checks what it wrote.
pub(crate) async fn work(kiwi: Arc<Kiwi>, key: String, token: CancellationToken) {
    tracing::info!("signing Kiwi's Android app");
    let call = Invocation {
        program: kiwi.automation.android_runner.read().unwrap().clone(),
        command: "kiwi-app",
        id: JOB_ID,
        recipe: None,
        builder: "The Android builder",
        timeout: TIMEOUT,
    };
    let result = automation::invoke(
        &kiwi,
        call,
        &json!({ "input": key }),
        &token,
        |stage| {
            tracing::debug!("Kiwi's Android app: {stage}");
            Ok(())
        },
        |_| Ok(()),
    )
    .await;
    let outcome = match result {
        Ok(events) => check(&kiwi, &key, &events).await,
        Err((message, _)) => Err(message),
    };
    {
        let mut status = kiwi.automation.kiwi_app.lock().unwrap();
        status.running = false;
        match outcome {
            Ok(app) => {
                tracing::info!(
                    version = %app.version_name,
                    code = app.version_code,
                    "Kiwi's Android app is signed and offered"
                );
                status.failure = None;
            }
            Err(_) if token.is_cancelled() => {
                tracing::info!("signing Kiwi's Android app was cancelled");
            }
            Err(message) => {
                tracing::warn!("could not prepare Kiwi's Android app: {message}");
                status.failure = Some(Failure {
                    key,
                    at: Utc::now(),
                    message,
                });
            }
        }
    }
    automation::finish_running(&kiwi, JOB_ID);
    kiwi.kick.notify_one();
    automation::start_next(&kiwi);
}

/// The app the runner reported is the one Kiwi now serves, made from `key`.
async fn check(kiwi: &Kiwi, key: &str, events: &[serde_json::Value]) -> Result<AndroidApp, String> {
    let reported = events
        .iter()
        .find(|e| e["event"] == "kiwi_app")
        .ok_or("The Android builder reported no app.")?;
    let dir = &kiwi.paths.android_app;
    let app = kiwi
        .android_app
        .get(dir)
        .await
        .ok_or("The Android builder wrote no app Kiwi can offer.")?;
    if app.package != PACKAGE
        || reported["sha256"].as_str() != Some(app.sha256.as_str())
        || made(dir).input.as_deref() != Some(key)
    {
        return Err("The app the Android builder wrote is not the one it reported.".into());
    }
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release() -> Release {
        Release {
            version_name: "1.2.0".into(),
            version_code: 1_002_000,
            url: "https://github.com/DigitalPals/Kiwi/releases/download/android-v1.2.0/kiwi-android-1.2.0-unsigned.apk".into(),
            sha256: "ab".repeat(32),
            size: 2_000_000,
        }
    }

    #[test]
    fn the_committed_pin_is_valid() {
        assert_eq!(parse_pin(PIN).unwrap().as_ref(), pinned());
        let text = |release: serde_json::Value| {
            json!({"schema": 1, "package": PACKAGE, "release": release}).to_string()
        };
        let good = json!({
            "version_name": "1.2.0", "version_code": 1_002_000, "url": release().url,
            "sha256": "ab".repeat(32), "size": 2_000_000,
        });
        assert_eq!(parse_pin(&text(good.clone())), Ok(Some(release())));
        assert_eq!(parse_pin(&text(serde_json::Value::Null)), Ok(None));
        for (key, value) in [
            ("url", json!("https://example.com/kiwi.apk")),
            ("sha256", json!("AB".repeat(32))),
            ("size", json!(0)),
            ("notes", json!("unknown field")),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(parse_pin(&text(bad)).is_err(), "{key}");
        }
        assert!(parse_pin(r#"{"schema":1,"package":"com.example.app","release":null}"#).is_err());
    }

    #[test]
    fn the_input_follows_the_release_and_the_owners_apks() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("android-app");
        assert_eq!(input_key(&dir, None), None, "nothing to sign");
        let released = input_key(&dir, Some(&release())).unwrap();
        let mut other = release();
        other.sha256 = "cd".repeat(32);
        assert_ne!(input_key(&dir, Some(&other)), Some(released.clone()));
        // An APK the owner placed changes the input, also without a release.
        std::fs::create_dir_all(dir.join("unsigned")).unwrap();
        std::fs::write(dir.join("unsigned/notes.txt"), "ignored").unwrap();
        assert_eq!(input_key(&dir, None), None);
        std::fs::write(dir.join("unsigned/kiwi.apk"), "PK one").unwrap();
        let local = input_key(&dir, None).unwrap();
        assert_ne!(input_key(&dir, Some(&release())).unwrap(), released);
        std::fs::write(dir.join("unsigned/kiwi.apk"), "PK second build").unwrap();
        assert_ne!(input_key(&dir, None).unwrap(), local);
    }

    #[test]
    fn the_served_app_is_current_for_its_input_and_key() {
        let app = AndroidApp {
            package: PACKAGE.into(),
            version_name: "1.2.0".into(),
            version_code: 1_002_000,
            sha256: "ab".repeat(32),
            size: 2,
        };
        let made = Made {
            input: Some("key".into()),
            signer: Some("C0FFEE".into()),
        };
        assert!(current(Some(&app), &made, "key", Some("c0ffee")));
        assert!(current(Some(&app), &made, "key", None));
        assert!(!current(None, &made, "key", None), "nothing valid served");
        assert!(!current(Some(&app), &made, "other", None), "new input");
        assert!(!current(Some(&app), &made, "key", Some("beef")), "new key");
        assert!(!current(Some(&app), &Made::default(), "key", None));
        let other = AndroidApp {
            package: "com.example.app".into(),
            ..app
        };
        assert!(!current(Some(&other), &made, "key", None));
    }
}
