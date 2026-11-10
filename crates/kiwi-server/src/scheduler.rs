//! The minute scheduler: source refresh, App Store lookups, update detection, auto-prepare,
//! notifications, devicectl sync and cleanup.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant, SystemTime},
};

use chrono::Utc;

use crate::{
    Kiwi,
    actions::{Decision, UpdateReason, same_release},
    builds,
    db::{BuildRow, BuildStatus, DeviceRow, Time},
    devices,
    error::ApiResult,
    push::{self, Payload},
    settings::Settings,
    sources, store,
    views::View,
};

const TICK: Duration = Duration::from_secs(60);
const SIGNING_RESCAN: Duration = Duration::from_secs(10 * 60);
/// How often devicectl is looked for again while it is missing (e.g. Xcode installed later).
const DEVICECTL_RECHECK: Duration = Duration::from_secs(10 * 60);
const HOURLY: Duration = Duration::from_secs(60 * 60);
pub const BUILD_RETENTION: chrono::Duration = chrono::Duration::days(7);
pub const DOWNLOAD_RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const WORK_RETENTION: Duration = Duration::from_secs(6 * 60 * 60);
const EXPIRY_THRESHOLDS: [i64; 3] = [1, 7, 30];
/// Builds of one release the scheduler starts within [`RETRY_WINDOW`], including the
/// failed ones it retries.
pub const RETRY_LIMIT: usize = 3;
pub const RETRY_WINDOW: chrono::Duration = chrono::Duration::hours(24);
/// Cookie devices without installs, attempts or subscriptions are deleted when unseen this long.
pub const LOCAL_DEVICE_RETENTION: chrono::Duration = chrono::Duration::hours(24);

#[derive(Default)]
pub struct Timers {
    signing: Option<Instant>,
    devicectl: Option<Instant>,
    expiry: Option<Instant>,
    sync: Option<Instant>,
    cleanup: Option<Instant>,
}

fn due(last: &mut Option<Instant>, every: Duration) -> bool {
    if last.is_some_and(|t| t.elapsed() < every) {
        return false;
    }
    *last = Some(Instant::now());
    true
}

pub async fn run(kiwi: Arc<Kiwi>) {
    let mut timers = Timers::default();
    tokio::select! {
        _ = kiwi.cancel.cancelled() => return,
        _ = tokio::time::sleep(Duration::from_secs(3)) => {}
    }
    loop {
        tick(&kiwi, &mut timers).await;
        tokio::select! {
            _ = kiwi.cancel.cancelled() => break,
            _ = tokio::time::sleep(TICK) => {}
            // Builds finishing, downloads completing and refreshes wake the scheduler early.
            _ = kiwi.kick.notified() => {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

pub async fn tick(kiwi: &Arc<Kiwi>, timers: &mut Timers) {
    crate::automation::tick(kiwi);
    crate::owner::tick(kiwi).await;
    let settings = kiwi.settings();
    refresh_due_sources(kiwi, &settings).await;
    if let Err(e) = store::refresh(kiwi).await {
        tracing::warn!("App Store lookup failed: {e}");
    }
    if due(&mut timers.signing, SIGNING_RESCAN) {
        // Bounded: a blocked keychain is reported as an error after a while.
        kiwi.rescan_signing().await;
    }
    if !kiwi.devicectl_available.load(Ordering::Relaxed)
        && due(&mut timers.devicectl, DEVICECTL_RECHECK)
        && kiwi.devices.devicectl_available().await
    {
        tracing::info!("devicectl is available now");
        kiwi.devicectl_available.store(true, Ordering::Relaxed);
    }
    if let Err(e) = retry_builds(kiwi).await {
        tracing::warn!("retrying builds failed: {e}");
    }
    if let Err(e) = process_updates(kiwi, &settings).await {
        tracing::warn!("update processing failed: {e}");
    }
    if settings.notify_expiry
        && due(&mut timers.expiry, HOURLY)
        && let Err(e) = expiry_warnings(kiwi).await
    {
        tracing::warn!("expiry warnings failed: {e}");
    }
    let sync_every = Duration::from_secs(u64::from(settings.devicectl_sync_minutes) * 60);
    if settings.devicectl_sync
        && due(&mut timers.sync, sync_every)
        && let Err(e) = devices::sync(kiwi).await
    {
        tracing::warn!("devicectl sync failed: {e}");
    }
    if due(&mut timers.cleanup, HOURLY)
        && let Err(e) = cleanup(kiwi).await
    {
        tracing::warn!("cleanup failed: {e}");
    }
}

pub async fn refresh_due_sources(kiwi: &Arc<Kiwi>, settings: &Settings) {
    let now = Utc::now();
    let interval = chrono::Duration::minutes(i64::from(settings.refresh_interval_minutes));
    let ids: Vec<String> = kiwi
        .catalog
        .sources()
        .iter()
        .filter(|s| {
            s.id != crate::automation::SOURCE_ID
                && s.enabled
                && s.last_attempt_at.is_none_or(|t| now - t >= interval)
        })
        .map(|s| s.id.clone())
        .collect();
    if !ids.is_empty() {
        sources::refresh_many(kiwi, ids).await;
    }
}

/// When the scheduler re-queues `failed`, the newest build of its release (`builds`: the
/// app's builds, newest first): only failures by a restart or a download/network error,
/// within a day of failing, while fewer than [`RETRY_LIMIT`] builds of the release started
/// in the last day, 1 then 5 minutes after the failure. `None`: not retried automatically
/// (signing and validation errors wait for the user).
pub fn retry_at(builds: &[BuildRow], failed: &BuildRow, now: Time) -> Option<Time> {
    if failed.status != BuildStatus::Failed || !failed.retryable {
        return None;
    }
    let finished = failed.finished_at.unwrap_or(failed.created_at);
    if now - finished > RETRY_WINDOW {
        return None;
    }
    let attempts = builds
        .iter()
        .filter(|b| {
            b.signed_bundle_id == failed.signed_bundle_id
                && same_release(b, &failed.release)
                && now - b.created_at < RETRY_WINDOW
        })
        .count()
        .max(1);
    if attempts >= RETRY_LIMIT {
        return None;
    }
    Some(finished + chrono::Duration::minutes(5i64.pow(attempts as u32 - 1)))
}

/// Re-queues failed builds that are due for a retry ([`retry_at`]), whether they were
/// prepared by the user or automatically.
pub async fn retry_builds(kiwi: &Arc<Kiwi>) -> ApiResult<()> {
    let now = Utc::now();
    let builds = kiwi.db.builds()?;
    let mut by_app: HashMap<(&str, &str), Vec<&BuildRow>> = HashMap::new();
    for b in &builds {
        by_app
            .entry((b.source_id.as_str(), b.bundle_id.as_str()))
            .or_default()
            .push(b);
    }
    for ((source_id, bundle_id), list) in by_app {
        let Some(app) = kiwi.catalog.app(source_id, bundle_id) else {
            continue;
        };
        let a = app.app();
        if !app.source.enabled || a.unavailable.is_some() {
            continue;
        }
        let list: Vec<BuildRow> = list.into_iter().cloned().collect();
        for (i, failed) in list.iter().enumerate() {
            if retry_at(&list, failed, now).is_none_or(|at| at > now) {
                continue;
            }
            // Each failed build is retried once (its retry may fail again and be retried).
            kiwi.db.clear_retryable(&failed.id)?;
            let newer = list[..i].iter().any(|b| {
                b.signed_bundle_id == failed.signed_bundle_id && same_release(b, &failed.release)
            });
            let index = a.releases.iter().position(|r| same_release(failed, r));
            let (false, Some(index)) = (newer, index) else {
                continue;
            };
            match builds::prepare_release(kiwi, &app, index).await {
                Ok(b) => {
                    tracing::info!(build = %b.id, failed = %failed.id, app = %a.name, "retrying build")
                }
                Err(e) => tracing::info!(app = %a.name, "cannot retry build: {e}"),
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Ready,
    Available,
}

/// When to notify about an update: once its build is ready, or right away when nothing is
/// being prepared for it. `None` while a build is running, about to be queued, or about to
/// be retried (`retrying`).
pub fn notification_phase(d: &Decision<'_>, auto_prepare: bool, retrying: bool) -> Option<Phase> {
    if d.install_build.is_some() {
        return Some(Phase::Ready);
    }
    match d.build.map(|b| b.status) {
        Some(s) if s.is_active() => None,
        Some(BuildStatus::Failed) if retrying => None,
        Some(BuildStatus::Failed) => Some(Phase::Available),
        _ if auto_prepare => None,
        _ => Some(Phase::Available),
    }
}

/// An update of one app on one device.
pub struct Update<'a> {
    pub device_id: &'a str,
    pub signed_bundle_id: &'a str,
    pub name: &'a str,
    /// Target release.
    pub version: &'a str,
    pub build: Option<&'a str>,
    pub revision: Option<u64>,
    /// What the device has, when known.
    pub installed_version: Option<&'a str>,
    pub installed_build: Option<&'a str>,
    pub installed_revision: Option<u64>,
    pub reason: UpdateReason,
    /// Expiry of the installed build (a re-sign replaces it).
    pub installed_expiry: Option<Time>,
}

/// Dedupe key of an update notification: one per device, signed id, version (a re-sign is
/// keyed by the expiry it replaces) and phase, so "ready" can follow "available".
pub fn update_notice_key(u: &Update<'_>, phase: Phase) -> String {
    let mut key = match u.reason {
        UpdateReason::NewVersion => format!(
            "update:{}:{}:{}:{}",
            u.device_id,
            u.signed_bundle_id,
            u.version,
            u.build.unwrap_or("")
        ),
        UpdateReason::Resign => format!(
            "resign:{}:{}:{}",
            u.device_id,
            u.signed_bundle_id,
            u.installed_expiry.map_or(0, |t| t.timestamp())
        ),
    };
    if u.reason == UpdateReason::NewVersion
        && let Some(revision) = u.revision
    {
        key.push_str(&format!(":kiwi:{revision}"));
    }
    match phase {
        Phase::Available => key,
        Phase::Ready => format!("{key}:ready"),
    }
}

/// "1.0 → 2.0" (with builds when only they differ), or "Version 2.0" when the installed
/// version is unknown.
fn version_change(u: &Update<'_>) -> String {
    let target = u.version;
    if u.installed_version == Some(target)
        && u.installed_build == u.build
        && let (Some(from), Some(to)) = (u.installed_revision, u.revision)
    {
        return format!("{target} · Kiwi {from} → {target} · Kiwi {to}");
    }
    match u.installed_version.filter(|v| !v.trim().is_empty()) {
        None => format!("Version {target}"),
        Some(installed) => match (u.installed_build, u.build) {
            (Some(from), Some(to)) if installed == target && from != to => {
                format!("{installed} ({from}) → {target} ({to})")
            }
            _ => format!("{installed} → {target}"),
        },
    }
}

/// Dedupe key and payload of an update notification. Tapping it opens My Apps.
pub fn update_notice(u: &Update<'_>, phase: Phase, badge: usize) -> (String, Payload) {
    let Update {
        signed_bundle_id,
        name,
        reason,
        ..
    } = *u;
    let (title, body) = match (reason, phase) {
        (UpdateReason::NewVersion, Phase::Ready) => (
            format!("{name} update ready"),
            format!("{} · Tap to install", version_change(u)),
        ),
        (UpdateReason::NewVersion, Phase::Available) => {
            (format!("{name} update available"), version_change(u))
        }
        (UpdateReason::Resign, Phase::Ready) => (
            format!("{name} re-signed"),
            "Install before the current build expires".into(),
        ),
        (UpdateReason::Resign, Phase::Available) => (
            format!("{name} expires soon"),
            "Re-sign it in Kiwi to keep using it".into(),
        ),
    };
    let payload = Payload {
        title,
        body,
        url: "/apps".into(),
        badge,
        tag: format!("update:{signed_bundle_id}"),
    };
    (update_notice_key(u, phase), payload)
}

/// Finds updates for every device, queues builds when `auto_prepare_updates`, and notifies:
/// "ready" once a build is ready; "available" when nothing will be prepared automatically
/// (auto-prepare is off, it failed, or the build failed and is not retried).
pub async fn process_updates(kiwi: &Arc<Kiwi>, settings: &Settings) -> ApiResult<()> {
    // Only the owner's devices; other people's can't use Kiwi.
    for device in owner_devices(kiwi)? {
        let public = kiwi.default_public_url();
        let updates = {
            let view = View::load(kiwi, &device, &public)?;
            let updates = view.updates();
            let mut queue = Vec::new();
            for (app, _) in &updates {
                let d = view.decide(app, view.installed_for(app));
                let idle = d.build.is_none_or(|b| b.status == BuildStatus::Ready);
                if settings.auto_prepare_updates
                    && d.install_build.is_none()
                    && idle
                    && let Some(target) = d.target
                {
                    queue.push((app.clone(), target));
                }
            }
            (updates.len(), queue)
        };
        let (count, queue) = updates;
        let mut unprepared = HashSet::new();
        for (app, target) in queue {
            let a = app.app();
            match builds::prepare_release(kiwi, &app, target).await {
                Ok(b) => {
                    kiwi.jobs
                        .auto_prepare_result(&app.source.id, &a.bundle_id, None);
                    tracing::info!(build = %b.id, app = %a.name, "auto-preparing update")
                }
                Err(e) => {
                    // Retried every tick (e.g. while signing is unavailable); logged once
                    // per error.
                    let message = e.to_string();
                    if kiwi
                        .jobs
                        .auto_prepare_result(&app.source.id, &a.bundle_id, Some(&message))
                    {
                        tracing::info!(app = %a.name, "cannot auto-prepare update: {message}");
                    }
                    unprepared.insert((app.source.id.clone(), a.bundle_id.clone()));
                }
            }
        }
        if !settings.notify_updates || count == 0 || kiwi.db.subscriptions(&device.id)?.is_empty() {
            continue;
        }
        let notices = {
            let view = View::load(kiwi, &device, &public)?;
            let mut notices = Vec::new();
            for (app, reason) in view.updates() {
                let a = app.app();
                let Some(installed) = view.installed_for(&app) else {
                    continue;
                };
                let d = view.decide(&app, Some(installed));
                let builds = view.app_builds(&app.source.id, &a.bundle_id);
                let retrying = d
                    .build
                    .is_some_and(|b| retry_at(builds, b, view.now).is_some());
                let phase = if unprepared.contains(&(app.source.id.clone(), a.bundle_id.clone())) {
                    Some(Phase::Available)
                } else {
                    notification_phase(&d, settings.auto_prepare_updates, retrying)
                };
                let (Some(target), Some(phase)) = (d.target, phase) else {
                    continue;
                };
                let release = &a.releases[target];
                let signed = view.signed_id(a);
                let update = Update {
                    device_id: &device.id,
                    signed_bundle_id: &signed,
                    name: &a.name,
                    version: &release.version,
                    build: release.build.as_deref(),
                    revision: release.revision,
                    installed_version: Some(&installed.version),
                    installed_build: installed.build.as_deref(),
                    installed_revision: installed.release.as_ref().and_then(|r| r.revision),
                    reason,
                    installed_expiry: installed.expires_at,
                };
                // "Available" after "ready" would be stale.
                let ready = update_notice_key(&update, Phase::Ready);
                if phase == Phase::Available && kiwi.db.notification_sent(&ready)? {
                    continue;
                }
                notices.push(update_notice(&update, phase, count));
            }
            notices
        };
        for (key, payload) in notices {
            push::notify_once(kiwi, &device.id, &key, &payload).await?;
        }
    }
    Ok(())
}

/// The smallest threshold (30/7/1 days) an expiry has crossed.
pub fn expiry_threshold(expires_at: Time, now: Time) -> Option<i64> {
    if expires_at <= now {
        return None;
    }
    let days_left = (expires_at - now).num_minutes() as f64 / (24.0 * 60.0);
    EXPIRY_THRESHOLDS
        .into_iter()
        .find(|t| days_left <= *t as f64)
}

fn owner_devices(kiwi: &Kiwi) -> ApiResult<Vec<DeviceRow>> {
    let mut devices = kiwi.db.devices()?;
    devices.retain(|d| crate::owner::is_owner(kiwi, d));
    Ok(devices)
}

pub async fn expiry_warnings(kiwi: &Arc<Kiwi>) -> ApiResult<()> {
    let now = Utc::now();
    for device in owner_devices(kiwi)? {
        if kiwi.db.subscriptions(&device.id)?.is_empty() {
            continue;
        }
        let badge = View::load(kiwi, &device, &kiwi.default_public_url())?
            .updates()
            .len();
        for install in kiwi.db.installs(&device.id, false)? {
            let Some(expires) = install.expires_at.filter(|_| install.origin == "kiwi") else {
                continue;
            };
            let Some(threshold) = expiry_threshold(expires, now) else {
                continue;
            };
            let key = format!(
                "expiry:{}:{}:{}:{threshold}",
                device.id,
                install.signed_bundle_id,
                expires.timestamp()
            );
            let days = (expires - now).num_days().max(0) + 1;
            let days = if days == 1 {
                "1 day".into()
            } else {
                format!("{days} days")
            };
            let payload = Payload {
                title: "Kiwi".into(),
                body: format!(
                    "{} expires in {days}. Open Kiwi to re-sign it.",
                    install.name
                ),
                url: "/apps".into(),
                badge,
                tag: format!("expiry:{}", install.signed_bundle_id),
            };
            push::notify_once(kiwi, &device.id, &key, &payload).await?;
        }
    }
    Ok(())
}

/// Builds older than 7 days that no current install references and that are not the newest
/// (or newest ready) build of an app still in a catalog (`in_catalog(source id, bundle
/// id)`), unless that build has expired.
pub fn builds_to_delete<'a>(
    builds: &'a [BuildRow],
    referenced: &HashSet<String>,
    in_catalog: impl Fn(&str, &str) -> bool,
    now: Time,
) -> Vec<&'a BuildRow> {
    let mut newest: HashMap<(&str, &str), &BuildRow> = HashMap::new();
    let mut newest_ready: HashMap<(&str, &str), &BuildRow> = HashMap::new();
    for b in builds {
        let key = (b.source_id.as_str(), b.bundle_id.as_str());
        let slots = [
            Some(&mut newest),
            (b.status == BuildStatus::Ready).then_some(&mut newest_ready),
        ];
        for map in slots.into_iter().flatten() {
            let slot = map.entry(key).or_insert(b);
            if b.created_at > slot.created_at {
                *slot = b;
            }
        }
    }
    let kept: HashSet<&str> = newest
        .values()
        .chain(newest_ready.values())
        .filter(|b| in_catalog(&b.source_id, &b.bundle_id) && b.expires_at.is_none_or(|e| e > now))
        .map(|b| b.id.as_str())
        .collect();
    builds
        .iter()
        .filter(|b| {
            !b.status.is_active()
                && !referenced.contains(&b.id)
                && !kept.contains(b.id.as_str())
                && now - b.finished_at.unwrap_or(b.created_at) > BUILD_RETENTION
        })
        .collect()
}

/// Cached downloads (`<sha256>.ipa`) unused for 14 days and not needed by a running build.
pub fn downloads_to_delete(
    files: &[(String, SystemTime)],
    in_use: &HashSet<String>,
    now: SystemTime,
) -> Vec<String> {
    files
        .iter()
        .filter(|(name, modified)| {
            let sha = name.strip_suffix(".ipa").unwrap_or(name);
            !in_use.contains(sha)
                && now.duration_since(*modified).unwrap_or_default() > DOWNLOAD_RETENTION
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn entries(dir: &Path) -> Vec<(String, SystemTime)> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| {
                    let modified = e.metadata().ok()?.modified().ok()?;
                    Some((e.file_name().to_string_lossy().into_owned(), modified))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub async fn cleanup(kiwi: &Arc<Kiwi>) -> ApiResult<()> {
    crate::automation::cleanup(kiwi).await?;
    let now = Utc::now();
    let builds = kiwi.db.builds()?;
    let referenced: HashSet<String> = kiwi
        .db
        .all_installs()?
        .into_iter()
        .filter_map(|i| i.build_id)
        .collect();
    let in_catalog =
        |source_id: &str, bundle_id: &str| kiwi.catalog.app(source_id, bundle_id).is_some();
    for b in builds_to_delete(&builds, &referenced, in_catalog, now) {
        tracing::info!(build = %b.id, "deleting old build");
        let _ = tokio::fs::remove_dir_all(kiwi.paths.builds.join(&b.id)).await;
        kiwi.db.delete_build(&b.id)?;
    }
    let in_use: HashSet<String> = builds
        .iter()
        .filter(|b| b.status.is_active())
        .filter_map(|b| {
            b.artifact_sha256
                .clone()
                .or_else(|| b.release.sha256.clone())
        })
        .collect();
    let files: Vec<_> = entries(&kiwi.paths.downloads)
        .into_iter()
        .filter(|(name, _)| name.ends_with(".ipa") && name.len() == 68)
        .collect();
    let active = builds.iter().any(|b| b.status.is_active());
    for name in downloads_to_delete(&files, &in_use, SystemTime::now()) {
        tracing::info!("deleting cached download {name}");
        let _ = tokio::fs::remove_file(kiwi.paths.downloads.join(name)).await;
    }
    // Leftovers of interrupted signing; running jobs use fresh directories.
    if !active {
        for (name, modified) in entries(&kiwi.paths.work) {
            if SystemTime::now()
                .duration_since(modified)
                .unwrap_or_default()
                > WORK_RETENTION
            {
                let path = kiwi.paths.work.join(name);
                let _ = tokio::fs::remove_dir_all(&path).await;
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    }
    kiwi.db
        .prune_notifications(now - chrono::Duration::days(180))?;
    let stale = kiwi
        .db
        .delete_stale_local_devices(now - LOCAL_DEVICE_RETENTION)?;
    if stale > 0 {
        tracing::info!("deleted {stale} unused browser device(s)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{build_for, build_row, release};
    use chrono::Duration as D;

    #[test]
    fn notification_phases() {
        let now = Utc::now();
        let ready = build_for(&release("2.0", None), BuildStatus::Ready, now);
        let running = build_for(&release("2.0", None), BuildStatus::Signing, now);
        let failed = build_for(&release("2.0", None), BuildStatus::Failed, now);
        let decision = |build: Option<&BuildRow>, install: Option<&BuildRow>, retrying| {
            notification_phase(
                &Decision {
                    action: crate::actions::Action::Update,
                    update_reason: Some(UpdateReason::NewVersion),
                    target: Some(0),
                    build,
                    install_build: install,
                    open_url: None,
                },
                true,
                retrying,
            )
        };
        assert_eq!(
            decision(Some(&ready), Some(&ready), false),
            Some(Phase::Ready)
        );
        assert_eq!(decision(Some(&running), None, false), None);
        assert_eq!(decision(Some(&failed), None, false), Some(Phase::Available));
        assert_eq!(
            decision(Some(&failed), None, true),
            None,
            "waits for the retry"
        );
        assert_eq!(decision(None, None, false), None);
        let manual = Decision {
            action: crate::actions::Action::Update,
            update_reason: Some(UpdateReason::NewVersion),
            target: Some(0),
            build: None,
            install_build: None,
            open_url: None,
        };
        assert_eq!(
            notification_phase(&manual, false, false),
            Some(Phase::Available)
        );
    }

    #[test]
    fn notices_are_keyed_per_device_signed_id_and_version() {
        let now = Utc::now();
        fn update<'a>(
            device_id: &'a str,
            version: &'a str,
            reason: UpdateReason,
            installed_expiry: Option<Time>,
        ) -> Update<'a> {
            Update {
                device_id,
                signed_bundle_id: "a.b.T",
                name: "App",
                version,
                build: None,
                revision: None,
                installed_version: Some("1.0"),
                installed_build: None,
                installed_revision: None,
                reason,
                installed_expiry,
            }
        }
        let expiry = Some(now + D::days(20));
        let notice = |device: &str, version: &str, phase| {
            update_notice(
                &update(device, version, UpdateReason::NewVersion, expiry),
                phase,
                2,
            )
        };
        let (k1, p1) = notice("d1", "2.0", Phase::Ready);
        let (k2, p2) = notice("d1", "2.0", Phase::Available);
        assert_ne!(k1, k2, "ready can follow available");
        assert_eq!(k1, format!("{k2}:ready"));
        assert_ne!(k1, notice("d2", "2.0", Phase::Ready).0);
        assert_ne!(k1, notice("d1", "2.1", Phase::Ready).0);
        assert_eq!(
            (p1.title.as_str(), p1.body.as_str()),
            ("App update ready", "1.0 → 2.0 · Tap to install")
        );
        assert_eq!(
            (p2.title.as_str(), p2.body.as_str()),
            ("App update available", "1.0 → 2.0")
        );
        assert_eq!(
            (p1.badge, p1.url.as_str(), p1.tag.as_str()),
            (2, "/apps", "update:a.b.T")
        );
        // Keys are unchanged, so notices sent before still count as sent.
        assert_eq!(k1, "update:d1:a.b.T:2.0::ready");
        let resign = |expiry, phase| {
            update_notice(&update("d1", "1.0", UpdateReason::Resign, expiry), phase, 1)
        };
        let (r1, r2) = (
            resign(expiry, Phase::Ready).0,
            resign(Some(now + D::days(400)), Phase::Ready).0,
        );
        assert_ne!(r1, r2, "each expiry gets its own re-sign notice");
        let (_, ready) = resign(expiry, Phase::Ready);
        assert_eq!(
            (
                ready.title.as_str(),
                ready.body.as_str(),
                ready.url.as_str()
            ),
            (
                "App re-signed",
                "Install before the current build expires",
                "/apps"
            )
        );
        let (_, soon) = resign(expiry, Phase::Available);
        assert_eq!(
            (soon.title.as_str(), soon.body.as_str()),
            ("App expires soon", "Re-sign it in Kiwi to keep using it")
        );
    }

    #[test]
    fn patch_revisions_have_separate_notices_and_visible_version_changes() {
        let mut u = Update {
            device_id: "d",
            signed_bundle_id: "youtube.T",
            name: "YouMod",
            version: "21.43.5",
            build: Some("21.43.5"),
            revision: Some(4),
            installed_version: Some("21.43.5"),
            installed_build: Some("21.43.5"),
            installed_revision: Some(2),
            reason: UpdateReason::NewVersion,
            installed_expiry: None,
        };
        let (key, notice) = update_notice(&u, Phase::Ready, 1);
        assert_eq!(
            notice.body,
            "21.43.5 · Kiwi 2 → 21.43.5 · Kiwi 4 · Tap to install"
        );
        u.revision = Some(5);
        assert_ne!(key, update_notice_key(&u, Phase::Ready));
    }

    #[test]
    fn update_notices_show_the_version_change() {
        let body = |installed: Option<&str>, installed_build, version, build| {
            let u = Update {
                device_id: "d",
                signed_bundle_id: "a.b.T",
                name: "App",
                version,
                build,
                revision: None,
                installed_version: installed,
                installed_build,
                installed_revision: None,
                reason: UpdateReason::NewVersion,
                installed_expiry: None,
            };
            update_notice(&u, Phase::Available, 1).1.body
        };
        assert_eq!(body(Some("1.0"), Some("7"), "2.0", Some("9")), "1.0 → 2.0");
        assert_eq!(body(None, None, "2.0", None), "Version 2.0");
        assert_eq!(body(Some(" "), None, "2.0", None), "Version 2.0");
        assert_eq!(
            body(Some("2.0"), Some("1"), "2.0", Some("2")),
            "2.0 (1) → 2.0 (2)"
        );
        assert_eq!(body(Some("2.0"), None, "2.0", Some("2")), "2.0 → 2.0");
    }

    #[test]
    fn failed_builds_are_retried_with_backoff_and_a_daily_cap() {
        let now = Utc::now();
        let failed_at = |minutes: i64, retryable: bool| {
            let mut b = build_for(&release("2.0", None), BuildStatus::Failed, now);
            b.created_at = now - D::minutes(minutes + 1);
            b.finished_at = Some(now - D::minutes(minutes));
            b.retryable = retryable;
            b
        };
        // First failure: 1 minute later.
        let first = failed_at(0, true);
        assert_eq!(
            retry_at(std::slice::from_ref(&first), &first, now),
            Some(now + D::minutes(1))
        );
        // Signing and validation errors are not retried.
        let signing = failed_at(0, false);
        assert_eq!(
            retry_at(std::slice::from_ref(&signing), &signing, now),
            None
        );
        // Second attempt: 5 minutes after it failed; a third is the last of the day.
        let second = failed_at(2, true);
        let builds = vec![second.clone(), failed_at(10, true)];
        assert_eq!(retry_at(&builds, &second, now), Some(now + D::minutes(3)));
        let third = failed_at(1, true);
        let builds = vec![third.clone(), failed_at(10, true), failed_at(20, true)];
        assert_eq!(retry_at(&builds, &third, now), None);
        // Attempts older than a day do not count; failures older than a day are not retried.
        let mut old = failed_at(10, true);
        old.created_at = now - D::hours(30);
        let builds = vec![third.clone(), old.clone(), failed_at(20, true)];
        assert!(retry_at(&builds, &third, now).is_some());
        let stale = failed_at(25 * 60, true);
        assert_eq!(retry_at(std::slice::from_ref(&stale), &stale, now), None);
        // Builds of other releases do not count.
        let other = build_for(&release("1.0", None), BuildStatus::Failed, now);
        let builds = vec![second.clone(), other.clone(), other];
        assert!(retry_at(&builds, &second, now).is_some());
        // Ready builds are never retried.
        let ready = build_for(&release("2.0", None), BuildStatus::Ready, now);
        assert_eq!(retry_at(std::slice::from_ref(&ready), &ready, now), None);
    }

    #[test]
    fn expiry_thresholds() {
        let now = Utc::now();
        assert_eq!(expiry_threshold(now + D::days(40), now), None);
        assert_eq!(expiry_threshold(now + D::days(30), now), Some(30));
        assert_eq!(expiry_threshold(now + D::days(8), now), Some(30));
        assert_eq!(expiry_threshold(now + D::days(7), now), Some(7));
        assert_eq!(expiry_threshold(now + D::hours(30), now), Some(7));
        assert_eq!(expiry_threshold(now + D::hours(20), now), Some(1));
        assert_eq!(expiry_threshold(now - D::hours(1), now), None);
    }

    #[test]
    fn cleanup_selection() {
        let now = Utc::now();
        let row = |id: &str, status, days: i64| {
            let mut b = build_row(id, status, now - D::days(days));
            b.finished_at = Some(b.created_at);
            b
        };
        let mut builds = vec![
            row("running", BuildStatus::Downloading, 20),
            row("newest", BuildStatus::Failed, 8),
            row("ready-newest", BuildStatus::Ready, 9),
            row("installed", BuildStatus::Ready, 10),
            row("stale", BuildStatus::Ready, 11),
            row("stale-failed", BuildStatus::Failed, 12),
        ];
        let referenced = HashSet::from(["installed".to_owned()]);
        let listed = |_: &str, _: &str| true;
        let ids = |list: Vec<&BuildRow>| list.into_iter().map(|b| b.id.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(builds_to_delete(&builds, &referenced, listed, now)),
            vec!["stale", "stale-failed"]
        );
        // Once the app left its source, only installed builds are kept.
        let gone = |_: &str, _: &str| false;
        assert_eq!(
            ids(builds_to_delete(&builds, &referenced, gone, now)),
            vec!["newest", "ready-newest", "stale", "stale-failed"]
        );
        // An expired newest ready build is not kept, an installed one is.
        let mut expired = builds.clone();
        for b in expired
            .iter_mut()
            .filter(|b| b.status == BuildStatus::Ready)
        {
            b.expires_at = Some(now - D::days(1));
        }
        assert_eq!(
            ids(builds_to_delete(&expired, &referenced, listed, now)),
            vec!["ready-newest", "stale", "stale-failed"]
        );
        // Newer builds make "newest" deletable; builds younger than 7 days stay.
        builds.push(row("young", BuildStatus::Failed, 3));
        builds.push(row("youngest", BuildStatus::Failed, 2));
        assert_eq!(
            ids(builds_to_delete(&builds, &referenced, listed, now)),
            vec!["newest", "stale", "stale-failed"]
        );
        // Another app's builds are independent.
        let mut other = row("other", BuildStatus::Ready, 30);
        other.bundle_id = "other.app".into();
        builds.push(other);
        let ids_now = ids(builds_to_delete(&builds, &referenced, listed, now));
        assert!(!ids_now.contains(&"other".to_owned()));
        let only_other_gone = |_: &str, bundle_id: &str| bundle_id != "other.app";
        let ids_gone = ids(builds_to_delete(&builds, &referenced, only_other_gone, now));
        assert!(ids_gone.contains(&"other".to_owned()));
        assert!(!ids_gone.contains(&"ready-newest".to_owned()));

        let sys = SystemTime::now();
        let day = Duration::from_secs(86_400);
        let files = vec![
            ("a.ipa".to_owned(), sys - day * 20),
            ("b.ipa".to_owned(), sys - day * 20),
            ("c.ipa".to_owned(), sys - day),
        ];
        assert_eq!(
            downloads_to_delete(&files, &HashSet::from(["b".to_owned()]), sys),
            vec!["a.ipa"]
        );
    }
}
