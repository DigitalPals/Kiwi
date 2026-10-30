//! Builds: download (at most 2 at a time) and sign (1 at a time) in background tasks, with
//! progress in the database and reuse of matching ready builds.

use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use kiwi_signer::{SignRequest, SignStep};
use kiwi_source::{CancellationToken, DownloadProgress, Release, Url};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::task::TaskTracker;

use crate::{
    Kiwi,
    actions::same_release,
    catalog::CatalogApp,
    db::{BuildRow, BuildStatus, Db, DeviceRow},
    devices::device_profile,
    error::{ApiError, ApiResult},
    ota::{icon_path, is_icon_image},
};

pub struct Jobs {
    downloads: Semaphore,
    signing: Semaphore,
    pub tracker: TaskTracker,
    prepare: Mutex<()>,
    /// Last error auto-preparing each app (source id, bundle id), so the minute scheduler
    /// logs it once rather than every tick.
    auto_prepare_errors: std::sync::Mutex<HashMap<(String, String), String>>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            downloads: Semaphore::new(2),
            signing: Semaphore::new(1),
            tracker: TaskTracker::new(),
            prepare: Mutex::new(()),
            auto_prepare_errors: Default::default(),
        }
    }
}

impl Jobs {
    /// Records the outcome of auto-preparing an app (`error` is `None` on success). True
    /// when the error is new for that app and worth logging.
    pub fn auto_prepare_result(
        &self,
        source_id: &str,
        bundle_id: &str,
        error: Option<&str>,
    ) -> bool {
        let mut errors = self
            .auto_prepare_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let key = (source_id.to_owned(), bundle_id.to_owned());
        match error {
            None => {
                errors.remove(&key);
                false
            }
            Some(error) => errors.insert(key, error.to_owned()).as_deref() != Some(error),
        }
    }
}

fn new_build_id() -> String {
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).expect("system random number generator");
    hex::encode(bytes)
}

/// A file name for the signed IPA.
fn file_name(app_name: &str) -> String {
    let name: String = app_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_ ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    match name.trim() {
        "" => "App".into(),
        n => n.into(),
    }
}

/// Why a build failed; `retryable` failures (downloads, restarts) may be re-queued by the
/// scheduler, others (validation, signing) wait for the user.
struct Failure {
    message: String,
    retryable: bool,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            retryable: false,
        }
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e6)
}

/// Prepares the requested version/build/revision, or the target release.
pub async fn prepare(
    kiwi: &Arc<Kiwi>,
    source_id: &str,
    bundle_id: &str,
    version: Option<&str>,
    build: Option<&str>,
    revision: Option<u64>,
    device: &DeviceRow,
) -> ApiResult<BuildRow> {
    let app = kiwi
        .catalog
        .app(source_id, bundle_id)
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let a = app.app();
    if let Some(reason) = &a.unavailable {
        return Err(ApiError::bad_request(reason.clone()));
    }
    let profile = device_profile(device);
    let index = match version {
        Some(version) => a
            .releases
            .iter()
            .position(|r| {
                r.version == version
                    && build.is_none_or(|b| r.build.as_deref() == Some(b))
                    && revision.is_none_or(|n| r.revision == Some(n))
            })
            .ok_or_else(|| ApiError::not_found(format!("{} has no release {version}.", a.name)))?,
        None => a.newest_eligible(profile.as_ref()).ok_or_else(|| {
            ApiError::bad_request(format!(
                "No release of {} is compatible with this device.",
                a.name
            ))
        })?,
    };
    if let Some(reason) = profile
        .as_ref()
        .and_then(|p| a.releases[index].incompatibility(p))
    {
        return Err(ApiError::bad_request(reason));
    }
    prepare_release(kiwi, &app, index).await
}

/// Returns a running or reusable ready build of the release, or queues a new one.
pub async fn prepare_release(
    kiwi: &Arc<Kiwi>,
    app: &CatalogApp,
    index: usize,
) -> ApiResult<BuildRow> {
    let a = app.app();
    let release = &a.releases[index];
    let now = Utc::now();
    let signing = kiwi.signing_info();
    let unavailable = || {
        ApiError::signing_unavailable(
            signing
                .error(now)
                .unwrap_or_else(|| "Signing is unavailable.".into()),
        )
    };
    let team = kiwi.team_id().ok_or_else(unavailable)?;
    let signed = kiwi_signer::signed_bundle_id(&a.bundle_id, &team);
    let plan = signing
        .plan(&signed, now)
        .map_err(ApiError::signing_unavailable)?;
    let key = plan.signing_key(kiwi.settings().strip_watch_apps);

    let _guard = kiwi.jobs.prepare.lock().await;
    for b in kiwi.db.app_builds(&app.source.id, &a.bundle_id)? {
        if !same_release(&b, release) || b.signed_bundle_id != signed {
            continue;
        }
        let file_ok = b.ipa_path.as_ref().is_some_and(|p| Path::new(p).is_file());
        if b.status.is_active()
            || (b.usable(now) && b.signing_key.as_ref() == Some(&key) && file_ok)
        {
            return Ok(b);
        }
    }
    let row = BuildRow {
        id: new_build_id(),
        source_id: app.source.id.clone(),
        bundle_id: a.bundle_id.clone(),
        app_name: a.name.clone(),
        icon_url: a.icon_url.as_ref().map(Url::to_string),
        version: release.version.clone(),
        build: release.build.clone(),
        release: release.clone(),
        artifact_sha256: None,
        url_scheme: Some(kiwi_signer::url_scheme_for(&signed)),
        signed_bundle_id: signed,
        identity_sha1: Some(plan.identity.sha1.clone()),
        profile_uuids: Vec::new(),
        signing_key: Some(key),
        expires_at: None,
        ipa_path: None,
        size: None,
        status: BuildStatus::Queued,
        step: Some("Waiting".into()),
        progress: None,
        error: None,
        retryable: false,
        dropped_entitlements: Vec::new(),
        created_at: now,
        finished_at: None,
        app_version: None,
    };
    kiwi.db.save_build(&row)?;
    tracing::info!(build = %row.id, app = %a.name, version = %row.version, "build queued");
    let task = run(kiwi.clone(), row.clone(), a.bundle_id.clone());
    kiwi.jobs.tracker.spawn(task);
    Ok(row)
}

async fn run(kiwi: Arc<Kiwi>, mut row: BuildRow, bundle_id: String) {
    let cancel = kiwi.cancel.child_token();
    let started = std::time::Instant::now();
    let result = execute(&kiwi, &mut row, &bundle_id, &cancel).await;
    row.finished_at = Some(Utc::now());
    row.step = None;
    match result {
        Ok(()) => {
            row.status = BuildStatus::Ready;
            row.progress = Some(1.0);
            tracing::info!(build = %row.id, secs = started.elapsed().as_secs(), "build ready");
        }
        Err(e) => {
            let e = if kiwi.cancel.is_cancelled() {
                Failure {
                    message: "Interrupted by restart".into(),
                    retryable: true,
                }
            } else {
                e
            };
            tracing::warn!(build = %row.id, retryable = e.retryable, "build failed: {}", e.message);
            row.status = BuildStatus::Failed;
            row.error = Some(e.message);
            row.retryable = e.retryable;
            row.ipa_path = None;
            let _ = tokio::fs::remove_dir_all(kiwi.paths.builds.join(&row.id)).await;
        }
    }
    if !save_finished(&kiwi.db, &row, SAVE_RETRY).await && row.status == BuildStatus::Ready {
        let _ = tokio::fs::remove_dir_all(kiwi.paths.builds.join(&row.id)).await;
    }
    kiwi.kick.notify_one();
}

const SAVE_RETRY: Duration = Duration::from_secs(1);

/// Saves a finished build. When that fails (a full disk, say) it is retried once after
/// `retry_after`, then the build is marked failed (and retryable) with a minimal update, so
/// it does not stay queued or running until the next restart. False when the row could not
/// be saved as is.
async fn save_finished(db: &Db, row: &BuildRow, retry_after: Duration) -> bool {
    let Err(e) = db.save_build(row) else {
        return true;
    };
    tracing::warn!(build = %row.id, "could not save build, retrying: {e}");
    tokio::time::sleep(retry_after).await;
    let Err(e) = db.save_build(row) else {
        return true;
    };
    tracing::error!(build = %row.id, "could not save build: {e}");
    let message = format!("Could not save the build: {e}");
    if let Err(e) = db.fail_build(&row.id, &message, Utc::now()) {
        tracing::error!(build = %row.id, "could not mark the build failed: {e}");
    }
    false
}

fn set_progress(kiwi: &Kiwi, id: &str, status: BuildStatus, step: &str, progress: Option<f64>) {
    if let Err(e) = kiwi.db.set_build_progress(id, status, Some(step), progress) {
        tracing::warn!(build = %id, "could not save progress: {e}");
    }
}

async fn acquire<'a>(
    semaphore: &'a Semaphore,
    cancel: &CancellationToken,
) -> Result<tokio::sync::SemaphorePermit<'a>, Failure> {
    tokio::select! {
        _ = cancel.cancelled() => Err(String::from("Cancelled").into()),
        permit = semaphore.acquire() => permit.map_err(|e| e.to_string().into()),
    }
}

async fn execute(
    kiwi: &Arc<Kiwi>,
    row: &mut BuildRow,
    bundle_id: &str,
    cancel: &CancellationToken,
) -> Result<(), Failure> {
    let permit = acquire(&kiwi.jobs.downloads, cancel).await?;
    set_progress(
        kiwi,
        &row.id,
        BuildStatus::Downloading,
        "Downloading",
        Some(0.0),
    );
    let (input, sha256) = artifact(kiwi, row, bundle_id, cancel).await?;
    let cached = input.clone();
    row.artifact_sha256 = Some(sha256);
    if let Some(url) = &row.icon_url {
        cache_icon(kiwi, url, cancel).await;
    }
    drop(permit);

    set_progress(
        kiwi,
        &row.id,
        BuildStatus::Signing,
        "Waiting to sign",
        Some(0.5),
    );
    let _permit = acquire(&kiwi.jobs.signing, cancel).await?;
    let signing = kiwi.rescan_signing().await;
    let plan = signing.plan(&row.signed_bundle_id, Utc::now())?;
    let strip_watch = kiwi.settings().strip_watch_apps;
    row.identity_sha1 = Some(plan.identity.sha1.clone());
    row.signing_key = Some(plan.signing_key(strip_watch));
    let dir = kiwi.paths.builds.join(&row.id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let output = dir.join(format!("{}.ipa", file_name(&row.app_name)));
    let request = SignRequest {
        input_ipa: input,
        output_ipa: output.clone(),
        work_dir: kiwi.paths.work.clone(),
        identity: plan.identity,
        profiles: signing.profiles.clone(),
        bundle_id: row.signed_bundle_id.clone(),
        url_scheme: row.url_scheme.clone(),
        strip_watch,
    };
    let flag = Arc::new(AtomicBool::new(false));
    let last_step = Arc::new(std::sync::Mutex::new(None));
    let watcher = tokio::spawn({
        let (flag, cancel) = (flag.clone(), cancel.clone());
        async move {
            cancel.cancelled().await;
            flag.store(true, Ordering::Relaxed);
        }
    });
    let report = tokio::task::spawn_blocking({
        let kiwi = kiwi.clone();
        let id = row.id.clone();
        let last_step = last_step.clone();
        move || {
            let progress = |step: SignStep| {
                *last_step.lock().unwrap_or_else(|e| e.into_inner()) = Some(step);
                let (status, fraction) = match step {
                    SignStep::Extracting => (BuildStatus::Signing, 0.55),
                    SignStep::Preparing => (BuildStatus::Signing, 0.6),
                    SignStep::Signing { done, total } => (
                        BuildStatus::Signing,
                        0.6 + 0.3 * done as f64 / total.max(1) as f64,
                    ),
                    SignStep::Verifying => (BuildStatus::Signing, 0.92),
                    SignStep::Packaging => (BuildStatus::Packaging, 0.95),
                };
                set_progress(&kiwi, &id, status, &step.to_string(), Some(fraction));
            };
            kiwi.signer.sign(&request, &flag, &progress)
        }
    })
    .await;
    watcher.abort();
    let extracting =
        *last_step.lock().unwrap_or_else(|e| e.into_inner()) == Some(SignStep::Extracting);
    if matches!(report, Ok(Err(_))) && extracting && !cancel.is_cancelled() {
        // The cached download could not be extracted: download it again on Retry.
        tracing::warn!(build = %row.id, "deleting cached download {}", cached.display());
        let _ = tokio::fs::remove_file(&cached).await;
    }
    let report = report.map_err(|e| format!("Signing task failed: {e}"))??;
    if report.bundle_id != row.signed_bundle_id {
        return Err(format!(
            "Signed bundle id {} does not match {}",
            report.bundle_id, row.signed_bundle_id
        )
        .into());
    }
    let size = tokio::fs::metadata(&output)
        .await
        .map_err(|e| format!("Signed IPA missing: {e}"))?
        .len();
    row.size = Some(size as i64);
    row.expires_at = Some(report.expires_at);
    row.profile_uuids = report.profile_uuids;
    row.dropped_entitlements = report.dropped_entitlements;
    row.app_version = Some(report.version).filter(|v| !v.trim().is_empty());
    row.ipa_path = Some(output.to_string_lossy().into_owned());
    Ok(())
}

/// A previously downloaded copy of the release, by its declared SHA-256 or an earlier build.
fn cached_artifact(kiwi: &Kiwi, release: &Release) -> Option<(PathBuf, String)> {
    let path = |sha: &str| kiwi.paths.downloads.join(format!("{sha}.ipa"));
    if let Some(sha) = &release.sha256 {
        return path(sha).is_file().then(|| (path(sha), sha.clone()));
    }
    kiwi.db
        .builds()
        .ok()?
        .into_iter()
        .filter(|b| {
            b.release.same_release(release) && b.release.download_url == release.download_url
        })
        .find_map(|b| b.artifact_sha256.filter(|sha| path(sha).is_file()))
        .map(|sha| (path(&sha), sha))
}

/// Whether the file at `path` still has the SHA-256 it is cached under (a crash or a full
/// disk can leave it truncated). Blocking: reads the whole file.
fn cached_download_intact(path: &Path, sha256: &str) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hash.update(&buffer[..n]),
            Err(_) => return false,
        }
    }
    hex::encode(hash.finalize()) == sha256
}

/// The validated IPA in `downloads/<sha256>.ipa`, downloading it when not cached (or when
/// the cached copy is damaged). Download (network, HTTP) failures are retryable; validation
/// failures are not.
async fn artifact(
    kiwi: &Arc<Kiwi>,
    row: &BuildRow,
    bundle_id: &str,
    cancel: &CancellationToken,
) -> Result<(PathBuf, String), Failure> {
    if row.source_id == crate::automation::SOURCE_ID {
        let (source, sha) = crate::automation::restore_artifact(kiwi, &row.release, cancel).await?;
        // Signing failures may invalidate their input cache. Keep the independently
        // retained local release intact so it can be prepared again later.
        let path = kiwi.paths.downloads.join(format!("{sha}.ipa"));
        tokio::fs::copy(source, &path)
            .await
            .map_err(|e| e.to_string())?;
        return Ok((path, sha));
    }
    if let Some((path, sha)) = cached_artifact(kiwi, &row.release) {
        let intact = tokio::task::spawn_blocking({
            let (path, sha) = (path.clone(), sha.clone());
            move || cached_download_intact(&path, &sha)
        })
        .await
        .unwrap_or(false);
        if intact {
            tracing::info!(build = %row.id, "using cached download {sha}");
            // Cleanup ages downloads by modification time.
            if let Ok(file) = std::fs::File::options().append(true).open(&path) {
                let _ = file.set_modified(std::time::SystemTime::now());
            }
            return Ok((path, sha));
        }
        tracing::warn!(build = %row.id, "cached download {sha} is damaged; downloading it again");
        let _ = tokio::fs::remove_file(&path).await;
    }
    let progress = Arc::new(DownloadProgress::default());
    let ticker = tokio::spawn({
        let (kiwi, progress, id) = (kiwi.clone(), progress.clone(), row.id.clone());
        async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let (received, total) = progress.snapshot();
                let (step, fraction) = if total > 0 {
                    (
                        format!(
                            "Downloading {} of {} MB",
                            megabytes(received),
                            megabytes(total)
                        ),
                        Some(0.5 * received.min(total) as f64 / total as f64),
                    )
                } else {
                    (format!("Downloading {} MB", megabytes(received)), None)
                };
                set_progress(&kiwi, &id, BuildStatus::Downloading, &step, fraction);
            }
        }
    });
    let result = kiwi
        .sources
        .download(
            &row.release,
            bundle_id,
            None,
            &kiwi.paths.downloads,
            cancel,
            &progress,
        )
        .await;
    ticker.abort();
    let _ = ticker.await;
    let artifact = result.map_err(|e| Failure {
        retryable: e.stage == "Download",
        message: e.to_string(),
    })?;
    let sha = artifact.sha256.clone();
    let path = kiwi.paths.downloads.join(format!("{sha}.ipa"));
    artifact.persist(&path).map_err(|e| e.to_string())?;
    Ok((path, sha))
}

/// Caches the app icon for the OTA manifest (best effort, size-limited by kiwi-source).
async fn cache_icon(kiwi: &Kiwi, url: &str, cancel: &CancellationToken) {
    let path = icon_path(&kiwi.paths.icons, url);
    if path.is_file() {
        return;
    }
    let Ok(parsed) = Url::parse(url) else { return };
    let fetched =
        tokio::time::timeout(Duration::from_secs(30), kiwi.sources.image(&parsed, cancel)).await;
    match fetched {
        Ok(Ok(bytes)) if is_icon_image(&bytes) => {
            let tmp = path.with_extension("tmp");
            if let Err(e) = tokio::fs::write(&tmp, &bytes).await {
                tracing::warn!("could not cache icon: {e}");
            } else if let Err(e) = tokio::fs::rename(&tmp, &path).await {
                tracing::warn!("could not cache icon: {e}");
            }
        }
        Ok(Ok(_)) => tracing::info!("icon {url} is not a PNG or JPEG; not cached"),
        Ok(Err(e)) => tracing::info!("icon {url} unavailable: {e}"),
        Err(_) => tracing::info!("icon {url} timed out"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_build_that_cannot_be_saved_is_marked_failed() {
        let db = Db::open_in_memory().unwrap();
        let now = Utc::now();
        let mut row = crate::testutil::build_row("b1", BuildStatus::Signing, now);
        db.save_build(&row).unwrap();
        // Like a full disk: whole-row writes fail, small updates do not.
        db.execute_batch(
            "CREATE TRIGGER full BEFORE INSERT ON builds
             BEGIN SELECT RAISE(FAIL, 'database or disk is full'); END;",
        )
        .unwrap();
        row.status = BuildStatus::Ready;
        row.ipa_path = Some("/data/builds/b1/Example.ipa".into());
        assert!(!save_finished(&db, &row, Duration::ZERO).await);
        let saved = db.build("b1").unwrap().unwrap();
        assert_eq!(saved.status, BuildStatus::Failed);
        assert!(saved.retryable);
        assert!(saved.ipa_path.is_none() && saved.finished_at.is_some());
        assert_eq!(
            saved.error.as_deref(),
            Some("Could not save the build: database or disk is full")
        );
        db.execute_batch("DROP TRIGGER full").unwrap();
        assert!(save_finished(&db, &row, Duration::ZERO).await);
        assert_eq!(db.build("b1").unwrap().unwrap().status, BuildStatus::Ready);
    }

    #[test]
    fn damaged_cached_downloads_are_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("download.ipa");
        std::fs::write(&path, b"ipa bytes").unwrap();
        let sha = hex::encode(Sha256::digest(b"ipa bytes"));
        assert!(cached_download_intact(&path, &sha));
        std::fs::write(&path, b"ipa").unwrap();
        assert!(!cached_download_intact(&path, &sha));
        assert!(!cached_download_intact(
            &dir.path().join("missing.ipa"),
            &sha
        ));
    }

    #[test]
    fn auto_prepare_errors_are_reported_once() {
        let jobs = Jobs::default();
        assert!(jobs.auto_prepare_result("s", "a", Some("no identity")));
        assert!(!jobs.auto_prepare_result("s", "a", Some("no identity")));
        assert!(jobs.auto_prepare_result("s", "b", Some("no identity")));
        assert!(jobs.auto_prepare_result("s", "a", Some("no profile")));
        assert!(!jobs.auto_prepare_result("s", "a", None));
        assert!(jobs.auto_prepare_result("s", "a", Some("no profile")));
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("YouTubePlus"), "YouTubePlus");
        assert_eq!(file_name("../../etc/passwd"), "______etc_passwd");
        assert_eq!(file_name("  "), "App");
        assert_eq!(file_name("Tom & Jerry"), "Tom _ Jerry");
        assert_eq!(file_name(&"x".repeat(100)).len(), 64);
        assert_eq!(new_build_id().len(), 24);
    }
}
