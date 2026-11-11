//! SQLite state (see architecture.md "State"). One connection behind a mutex: every query is
//! short, so callers run them inline, including from blocking signing threads.

use std::{collections::HashMap, path::Path, sync::Mutex};

use chrono::{DateTime, Utc};
use kiwi_source::Release;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

pub type Time = DateTime<Utc>;

/// Embedded migrations; `PRAGMA user_version` is the number applied.
const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE sources (
    id TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    position INTEGER NOT NULL,
    added_at TEXT NOT NULL,
    last_attempt_at TEXT,
    source TEXT NOT NULL
);
CREATE TABLE devices (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT,
    tailscale_name TEXT,
    os_version TEXT,
    os_version_source TEXT,
    model TEXT,
    devicectl_id TEXT,
    devicectl_name TEXT,
    devicectl_reachable INTEGER NOT NULL DEFAULT 0,
    last_sync_at TEXT,
    last_sync_error TEXT,
    udid TEXT,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL
);
CREATE TABLE builds (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    app_name TEXT NOT NULL,
    icon_url TEXT,
    version TEXT NOT NULL,
    build TEXT,
    release TEXT NOT NULL,
    artifact_sha256 TEXT,
    signed_bundle_id TEXT NOT NULL,
    url_scheme TEXT,
    identity_sha1 TEXT,
    profile_uuids TEXT NOT NULL DEFAULT '[]',
    signing_key TEXT,
    expires_at TEXT,
    ipa_path TEXT,
    size INTEGER,
    status TEXT NOT NULL,
    step TEXT,
    progress REAL,
    error TEXT,
    dropped_entitlements TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    finished_at TEXT
);
CREATE INDEX builds_app ON builds (source_id, bundle_id);
CREATE TABLE attempts (
    token TEXT PRIMARY KEY,
    build_id TEXT NOT NULL REFERENCES builds (id) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    manifest_at TEXT,
    download_started_at TEXT,
    download_completed_at TEXT,
    user_agent TEXT,
    UNIQUE (build_id, device_id)
);
CREATE INDEX attempts_device ON attempts (device_id);
CREATE TABLE installs (
    device_id TEXT NOT NULL,
    signed_bundle_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    name TEXT NOT NULL,
    icon_url TEXT,
    developer TEXT,
    build_id TEXT,
    version TEXT NOT NULL,
    build TEXT,
    release TEXT,
    url_scheme TEXT,
    expires_at TEXT,
    origin TEXT NOT NULL,
    verified INTEGER NOT NULL DEFAULT 0,
    installed_at TEXT NOT NULL,
    last_seen_at TEXT,
    removed_at TEXT,
    PRIMARY KEY (device_id, signed_bundle_id)
);
CREATE TABLE push_subscriptions (
    endpoint TEXT PRIMARY KEY,
    device_id TEXT NOT NULL,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_success_at TEXT,
    failures INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE notifications_sent (
    key TEXT PRIMARY KEY,
    sent_at TEXT NOT NULL
);
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#,
    r#"
ALTER TABLE builds ADD COLUMN retryable INTEGER NOT NULL DEFAULT 0;
UPDATE builds SET retryable = 1 WHERE status = 'failed' AND error = 'Interrupted by restart';
"#,
    r#"
ALTER TABLE sources ADD COLUMN browse TEXT NOT NULL DEFAULT 'all';
ALTER TABLE sources ADD COLUMN picked_at TEXT;
ALTER TABLE builds ADD COLUMN app_version TEXT;
CREATE TABLE app_prefs (
    source_id TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    shown INTEGER,
    original TEXT,
    PRIMARY KEY (source_id, bundle_id)
);
CREATE TABLE store_apps (
    country TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    track_id INTEGER,
    name TEXT,
    developer TEXT,
    version TEXT,
    released_at TEXT,
    icon_url TEXT,
    store_url TEXT,
    checked_at TEXT NOT NULL,
    PRIMARY KEY (country, bundle_id)
);
"#,
    r#"
CREATE TABLE people (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    renamed INTEGER NOT NULL DEFAULT 0,
    new_apps INTEGER NOT NULL DEFAULT 0,
    new_apps_since TEXT,
    created_at TEXT NOT NULL,
    last_seen TEXT NOT NULL
);
CREATE TABLE app_access (
    person_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    bundle_id TEXT NOT NULL,
    allowed INTEGER NOT NULL,
    changed_at TEXT NOT NULL,
    PRIMARY KEY (person_id, source_id, bundle_id)
);
CREATE TABLE registrations (
    device_id TEXT PRIMARY KEY,
    udid TEXT NOT NULL,
    product TEXT,
    os_build TEXT,
    device_name TEXT,
    status TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    decided_at TEXT,
    apple_id TEXT,
    registered_at TEXT,
    error TEXT
);
CREATE TABLE enrollments (
    token TEXT PRIMARY KEY,
    device_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);
ALTER TABLE devices ADD COLUMN person_id TEXT;
ALTER TABLE builds ADD COLUMN devices TEXT;
"#,
    // The family is gone: devices keep `person_id`, their Tailscale user, for the owner check.
    r#"
DROP TABLE enrollments;
DROP TABLE registrations;
DROP TABLE app_access;
DROP TABLE people;
ALTER TABLE builds DROP COLUMN devices;
"#,
    // Device platforms (architecture.md "Device identification"): the owner's choice, what
    // Tailscale or a User-Agent reported, and the first-run setup. Devices that were in use
    // already count as set up. Android packages installed with another signature.
    r#"
ALTER TABLE devices ADD COLUMN platform TEXT;
ALTER TABLE devices ADD COLUMN detected_platform TEXT;
ALTER TABLE devices ADD COLUMN onboarded_at TEXT;
UPDATE devices SET detected_platform = CASE
    WHEN model = 'Android' THEN 'android'
    WHEN model LIKE 'iPhone%' OR model LIKE 'iPad%' OR model LIKE 'iPod%'
        OR os_version_source IN ('safari', 'installer', 'devicectl') THEN 'ios'
    END;
UPDATE devices SET onboarded_at = first_seen
WHERE EXISTS (SELECT 1 FROM installs WHERE device_id = devices.id)
    OR EXISTS (SELECT 1 FROM attempts WHERE device_id = devices.id)
    OR EXISTS (SELECT 1 FROM push_subscriptions WHERE device_id = devices.id);
CREATE TABLE foreign_packages (
    device_id TEXT NOT NULL,
    package TEXT NOT NULL,
    seen_at TEXT NOT NULL,
    PRIMARY KEY (device_id, package)
);
"#,
];

pub struct Db {
    conn: Mutex<Connection>,
}

/// Which of a source's apps Browse shows by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowseMode {
    /// Every app, except the ones hidden one by one.
    #[default]
    All,
    /// Only the apps turned on one by one; new apps stay hidden.
    Picked,
}

impl BrowseMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Picked => "picked",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "picked" => Self::Picked,
            _ => Self::All,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceRow {
    pub id: String,
    pub url: String,
    pub enabled: bool,
    pub position: i64,
    pub added_at: Time,
    pub last_attempt_at: Option<Time>,
    pub browse: BrowseMode,
    /// When the source was last switched to [`BrowseMode::Picked`]: apps first seen later
    /// are new.
    pub picked_at: Option<Time>,
    pub source: kiwi_source::Source,
}

impl SourceRow {
    /// Whether Browse shows an app of this source with these choices.
    pub fn shows(&self, prefs: Option<&AppPrefs>) -> bool {
        prefs
            .and_then(|p| p.shown)
            .unwrap_or(self.browse == BrowseMode::All)
    }

    /// An app that appeared after the source was switched to [`BrowseMode::Picked`] and
    /// was not turned on or off yet.
    pub fn is_new(&self, prefs: Option<&AppPrefs>) -> bool {
        self.browse == BrowseMode::Picked
            && prefs.is_some_and(|p| {
                p.shown.is_none() && self.picked_at.is_some_and(|t| p.first_seen > t)
            })
    }
}

/// The App Store app an app is based on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OriginalChoice {
    /// The App Store app with the app's own bundle id, if there is one.
    #[default]
    Auto,
    /// Not based on an App Store app.
    None,
    /// The App Store app with this bundle id, chosen by hand.
    App(String),
}

/// The user's choices for one app of a source (`app_prefs`).
#[derive(Debug, Clone, PartialEq)]
pub struct AppPrefs {
    pub first_seen: Time,
    /// Shown in or hidden from Browse; `None` follows the source's [`BrowseMode`].
    pub shown: Option<bool>,
    pub original: OriginalChoice,
}

/// An app in one App Store storefront.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreApp {
    pub bundle_id: String,
    pub track_id: i64,
    pub name: String,
    pub developer: Option<String>,
    pub version: String,
    /// Release of the current version.
    pub released_at: Option<Time>,
    pub icon_url: Option<String>,
    pub store_url: String,
}

/// A cached App Store lookup of a bundle id (`store_apps`); `app` is `None` when no App
/// Store app uses it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreRow {
    pub bundle_id: String,
    pub app: Option<StoreApp>,
    pub checked_at: Time,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRow {
    pub id: String,
    pub kind: String,
    /// User-chosen name; the API falls back to the Tailscale name.
    pub name: Option<String>,
    pub tailscale_name: Option<String>,
    pub os_version: Option<String>,
    pub os_version_source: Option<String>,
    pub model: Option<String>,
    pub devicectl_id: Option<String>,
    pub devicectl_name: Option<String>,
    pub devicectl_reachable: bool,
    pub last_sync_at: Option<Time>,
    pub last_sync_error: Option<String>,
    pub udid: Option<String>,
    /// The Tailscale user the device belongs to; `None` for browser devices on the Mac and
    /// devices seen before Kiwi recorded users (both count as the owner's).
    pub person_id: Option<String>,
    /// The platform the owner chose for the device (`devices::platform`).
    pub platform: Option<Platform>,
    /// The platform Tailscale or a User-Agent reported first; `None` for desktop browsers.
    pub detected_platform: Option<Platform>,
    /// When the owner finished the first-run setup on the device.
    pub onboarded_at: Option<Time>,
    pub first_seen: Time,
    pub last_seen: Time,
}

/// What a device installs: iOS apps or Android apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ios,
    Android,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ios" => Some(Self::Ios),
            "android" => Some(Self::Android),
            _ => None,
        }
    }
}

impl rusqlite::types::ToSql for Platform {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl DeviceRow {
    pub fn new(id: String, kind: &str, now: Time) -> Self {
        Self {
            id,
            kind: kind.to_owned(),
            name: None,
            tailscale_name: None,
            os_version: None,
            os_version_source: None,
            model: None,
            devicectl_id: None,
            devicectl_name: None,
            devicectl_reachable: false,
            last_sync_at: None,
            last_sync_error: None,
            udid: None,
            person_id: None,
            platform: None,
            detected_platform: None,
            onboarded_at: None,
            first_seen: now,
            last_seen: now,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildStatus {
    Queued,
    Downloading,
    Signing,
    Packaging,
    Ready,
    Failed,
}

impl BuildStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Signing => "signing",
            Self::Packaging => "packaging",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "downloading" => Self::Downloading,
            "signing" => Self::Signing,
            "packaging" => Self::Packaging,
            "ready" => Self::Ready,
            _ => Self::Failed,
        }
    }
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Ready | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuildRow {
    pub id: String,
    pub source_id: String,
    pub bundle_id: String,
    pub app_name: String,
    pub icon_url: Option<String>,
    pub version: String,
    pub build: Option<String>,
    pub release: Release,
    pub artifact_sha256: Option<String>,
    pub signed_bundle_id: String,
    pub url_scheme: Option<String>,
    pub identity_sha1: Option<String>,
    pub profile_uuids: Vec<String>,
    /// Identity, main profile and signing settings the build was made with; a ready build
    /// is reused only while this is unchanged.
    pub signing_key: Option<String>,
    pub expires_at: Option<Time>,
    pub ipa_path: Option<String>,
    pub size: Option<i64>,
    pub status: BuildStatus,
    pub step: Option<String>,
    pub progress: Option<f64>,
    pub error: Option<String>,
    /// Failed by a restart or a download/network error: the scheduler may re-queue it.
    pub retryable: bool,
    pub dropped_entitlements: Vec<String>,
    pub created_at: Time,
    pub finished_at: Option<Time>,
    /// `CFBundleShortVersionString` of the signed IPA, which can differ from the release's
    /// version (some sources list the tweak's version).
    pub app_version: Option<String>,
}

impl BuildRow {
    /// Ready and not expired at `now`.
    pub fn usable(&self, now: Time) -> bool {
        self.status == BuildStatus::Ready && self.expires_at.is_none_or(|e| e > now)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttemptRow {
    pub token: String,
    pub build_id: String,
    pub device_id: String,
    pub created_at: Time,
    pub manifest_at: Option<Time>,
    pub download_started_at: Option<Time>,
    pub download_completed_at: Option<Time>,
    pub user_agent: Option<String>,
}

impl AttemptRow {
    /// A manifest fetch or download started within `window` and not completed since.
    pub fn in_progress(&self, now: Time, window: chrono::Duration) -> bool {
        let Some(activity) = self.manifest_at.max(self.download_started_at) else {
            return false;
        };
        now - activity < window
            && self
                .download_completed_at
                .is_none_or(|done| done < activity)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstallRow {
    pub device_id: String,
    pub signed_bundle_id: String,
    pub source_id: String,
    pub bundle_id: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub developer: Option<String>,
    pub build_id: Option<String>,
    pub version: String,
    pub build: Option<String>,
    pub release: Option<Release>,
    pub url_scheme: Option<String>,
    pub expires_at: Option<Time>,
    /// "kiwi" (download completed) or "device" (seen by devicectl).
    pub origin: String,
    pub verified: bool,
    pub installed_at: Time,
    pub last_seen_at: Option<Time>,
    pub removed_at: Option<Time>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionRow {
    pub endpoint: String,
    pub device_id: String,
    pub p256dh: String,
    pub auth: String,
    pub created_at: Time,
    pub last_success_at: Option<Time>,
    pub failures: i64,
}

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

fn from_json<T: DeserializeOwned>(row: &Row, column: &str) -> rusqlite::Result<T> {
    let text: String = row.get(column)?;
    serde_json::from_str(&text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn source_row(row: &Row) -> rusqlite::Result<SourceRow> {
    let enabled = row.get("enabled")?;
    let mut source: kiwi_source::Source = from_json(row, "source")?;
    // The column is authoritative; updates of `enabled` do not rewrite the JSON.
    source.enabled = enabled;
    let browse: String = row.get("browse")?;
    Ok(SourceRow {
        id: row.get("id")?,
        url: row.get("url")?,
        enabled,
        position: row.get("position")?,
        added_at: row.get("added_at")?,
        last_attempt_at: row.get("last_attempt_at")?,
        browse: BrowseMode::parse(&browse),
        picked_at: row.get("picked_at")?,
        source,
    })
}

fn app_prefs_row(row: &Row) -> rusqlite::Result<((String, String), AppPrefs)> {
    let original: Option<String> = row.get("original")?;
    let original = match original {
        None => OriginalChoice::Auto,
        Some(b) if b.is_empty() => OriginalChoice::None,
        Some(b) => OriginalChoice::App(b),
    };
    Ok((
        (row.get("source_id")?, row.get("bundle_id")?),
        AppPrefs {
            first_seen: row.get("first_seen")?,
            shown: row.get("shown")?,
            original,
        },
    ))
}

fn store_row(row: &Row) -> rusqlite::Result<StoreRow> {
    let track_id: Option<i64> = row.get("track_id")?;
    let bundle_id: String = row.get("bundle_id")?;
    let app = match track_id {
        Some(track_id) => Some(StoreApp {
            bundle_id: bundle_id.clone(),
            track_id,
            name: row.get::<_, Option<String>>("name")?.unwrap_or_default(),
            developer: row.get("developer")?,
            version: row.get::<_, Option<String>>("version")?.unwrap_or_default(),
            released_at: row.get("released_at")?,
            icon_url: row.get("icon_url")?,
            store_url: row
                .get::<_, Option<String>>("store_url")?
                .unwrap_or_default(),
        }),
        None => None,
    };
    Ok(StoreRow {
        bundle_id,
        app,
        checked_at: row.get("checked_at")?,
    })
}

fn device_row(row: &Row) -> rusqlite::Result<DeviceRow> {
    Ok(DeviceRow {
        id: row.get("id")?,
        kind: row.get("kind")?,
        name: row.get("name")?,
        tailscale_name: row.get("tailscale_name")?,
        os_version: row.get("os_version")?,
        os_version_source: row.get("os_version_source")?,
        model: row.get("model")?,
        devicectl_id: row.get("devicectl_id")?,
        devicectl_name: row.get("devicectl_name")?,
        devicectl_reachable: row.get("devicectl_reachable")?,
        last_sync_at: row.get("last_sync_at")?,
        last_sync_error: row.get("last_sync_error")?,
        udid: row.get("udid")?,
        person_id: row.get("person_id")?,
        platform: platform_column(row, "platform")?,
        detected_platform: platform_column(row, "detected_platform")?,
        onboarded_at: row.get("onboarded_at")?,
        first_seen: row.get("first_seen")?,
        last_seen: row.get("last_seen")?,
    })
}

fn platform_column(row: &Row, column: &str) -> rusqlite::Result<Option<Platform>> {
    let text: Option<String> = row.get(column)?;
    Ok(text.as_deref().and_then(Platform::parse))
}

fn build_row(row: &Row) -> rusqlite::Result<BuildRow> {
    let status: String = row.get("status")?;
    Ok(BuildRow {
        id: row.get("id")?,
        source_id: row.get("source_id")?,
        bundle_id: row.get("bundle_id")?,
        app_name: row.get("app_name")?,
        icon_url: row.get("icon_url")?,
        version: row.get("version")?,
        build: row.get("build")?,
        release: from_json(row, "release")?,
        artifact_sha256: row.get("artifact_sha256")?,
        signed_bundle_id: row.get("signed_bundle_id")?,
        url_scheme: row.get("url_scheme")?,
        identity_sha1: row.get("identity_sha1")?,
        profile_uuids: from_json(row, "profile_uuids")?,
        signing_key: row.get("signing_key")?,
        expires_at: row.get("expires_at")?,
        ipa_path: row.get("ipa_path")?,
        size: row.get("size")?,
        status: BuildStatus::parse(&status),
        step: row.get("step")?,
        progress: row.get("progress")?,
        error: row.get("error")?,
        retryable: row.get("retryable")?,
        dropped_entitlements: from_json(row, "dropped_entitlements")?,
        created_at: row.get("created_at")?,
        finished_at: row.get("finished_at")?,
        app_version: row.get("app_version")?,
    })
}

fn attempt_row(row: &Row) -> rusqlite::Result<AttemptRow> {
    Ok(AttemptRow {
        token: row.get("token")?,
        build_id: row.get("build_id")?,
        device_id: row.get("device_id")?,
        created_at: row.get("created_at")?,
        manifest_at: row.get("manifest_at")?,
        download_started_at: row.get("download_started_at")?,
        download_completed_at: row.get("download_completed_at")?,
        user_agent: row.get("user_agent")?,
    })
}

fn install_row(row: &Row) -> rusqlite::Result<InstallRow> {
    let release: Option<String> = row.get("release")?;
    Ok(InstallRow {
        device_id: row.get("device_id")?,
        signed_bundle_id: row.get("signed_bundle_id")?,
        source_id: row.get("source_id")?,
        bundle_id: row.get("bundle_id")?,
        name: row.get("name")?,
        icon_url: row.get("icon_url")?,
        developer: row.get("developer")?,
        build_id: row.get("build_id")?,
        version: row.get("version")?,
        build: row.get("build")?,
        release: release.and_then(|r| serde_json::from_str(&r).ok()),
        url_scheme: row.get("url_scheme")?,
        expires_at: row.get("expires_at")?,
        origin: row.get("origin")?,
        verified: row.get("verified")?,
        installed_at: row.get("installed_at")?,
        last_seen_at: row.get("last_seen_at")?,
        removed_at: row.get("removed_at")?,
    })
}

fn subscription_row(row: &Row) -> rusqlite::Result<SubscriptionRow> {
    Ok(SubscriptionRow {
        endpoint: row.get("endpoint")?,
        device_id: row.get("device_id")?,
        p256dh: row.get("p256dh")?,
        auth: row.get("auth")?,
        created_at: row.get("created_at")?,
        last_success_at: row.get("last_success_at")?,
        failures: row.get("failures")?,
    })
}

fn installs_in(
    c: &Connection,
    device_id: &str,
    include_removed: bool,
) -> rusqlite::Result<Vec<InstallRow>> {
    c.prepare(
        "SELECT * FROM installs WHERE device_id = ?1 AND (?2 OR removed_at IS NULL)
         ORDER BY name COLLATE NOCASE",
    )?
    .query_map(params![device_id, include_removed], install_row)?
    .collect()
}

fn save_install_in(c: &Connection, i: &InstallRow) -> rusqlite::Result<()> {
    c.execute(
        "INSERT OR REPLACE INTO installs (device_id, signed_bundle_id, source_id, bundle_id,
             name, icon_url, developer, build_id, version, build, release, url_scheme,
             expires_at, origin, verified, installed_at, last_seen_at, removed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            i.device_id,
            i.signed_bundle_id,
            i.source_id,
            i.bundle_id,
            i.name,
            i.icon_url,
            i.developer,
            i.build_id,
            i.version,
            i.build,
            i.release.as_ref().map(to_json),
            i.url_scheme,
            i.expires_at,
            i.origin,
            i.verified,
            i.installed_at,
            i.last_seen_at,
            i.removed_at
        ],
    )
    .map(drop)
}

fn migrate(conn: &mut Connection) -> Result<(), String> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version > MIGRATIONS.len() as i64 {
        return Err(format!(
            "The database schema (version {version}) is newer than this Kiwi (version {}). Update Kiwi.",
            MIGRATIONS.len()
        ));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let apply = |conn: &mut Connection| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()
        };
        apply(conn).map_err(|e| format!("Database migration {} failed: {e}", i + 1))?;
    }
    Ok(())
}

impl Db {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        Self::init(Connection::open_in_memory().map_err(|e| e.to_string())?)
    }

    fn init(mut conn: Connection) -> Result<Self, String> {
        // First, so switching to WAL waits for another connection (sqlite3 .backup)
        // instead of failing with "database is locked".
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| e.to_string())?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&conn)
    }

    /// Runs raw SQL (tests: triggers that make writes fail).
    #[cfg(test)]
    pub(crate) fn execute_batch(&self, sql: &str) -> rusqlite::Result<()> {
        self.with(|c| c.execute_batch(sql))
    }

    pub fn schema_version(&self) -> rusqlite::Result<i64> {
        self.with(|c| c.pragma_query_value(None, "user_version", |r| r.get(0)))
    }

    // ---- sources ----

    pub fn sources(&self) -> rusqlite::Result<Vec<SourceRow>> {
        self.with(|c| {
            c.prepare("SELECT * FROM sources ORDER BY position, added_at")?
                .query_map([], source_row)?
                .collect()
        })
    }

    pub fn save_source(&self, s: &SourceRow) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO sources (id, url, enabled, position, added_at, last_attempt_at, source,
                     browse, picked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (id) DO UPDATE SET url = ?2, enabled = ?3, position = ?4,
                     last_attempt_at = ?6, source = ?7, browse = ?8, picked_at = ?9",
                params![
                    s.id,
                    s.url,
                    s.enabled,
                    s.position,
                    s.added_at,
                    s.last_attempt_at,
                    to_json(&s.source),
                    s.browse.as_str(),
                    s.picked_at
                ],
            )
            .map(drop)
        })
    }

    /// Stores the result of a fetch (catalog, validators, error) without touching `enabled`,
    /// `position` or `url`. False when the source was removed.
    pub fn save_source_fetch(
        &self,
        id: &str,
        last_attempt_at: Time,
        source: &kiwi_source::Source,
    ) -> rusqlite::Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE sources SET last_attempt_at = ?2, source = ?3 WHERE id = ?1",
                params![id, last_attempt_at, to_json(source)],
            )? > 0)
        })
    }

    /// Records that a fetch is starting, before it runs. False when the source was removed.
    pub fn set_source_attempt(&self, id: &str, at: Time) -> rusqlite::Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE sources SET last_attempt_at = ?2 WHERE id = ?1",
                params![id, at],
            )? > 0)
        })
    }

    pub fn set_source_enabled(&self, id: &str, enabled: bool) -> rusqlite::Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE sources SET enabled = ?2 WHERE id = ?1",
                params![id, enabled],
            )? > 0)
        })
    }

    /// Sets which apps Browse shows by default; `picked_at` is when it became
    /// [`BrowseMode::Picked`].
    pub fn set_source_browse(
        &self,
        id: &str,
        browse: BrowseMode,
        picked_at: Option<Time>,
    ) -> rusqlite::Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE sources SET browse = ?2, picked_at = ?3 WHERE id = ?1",
                params![id, browse.as_str(), picked_at],
            )? > 0)
        })
    }

    /// Deletes a source and the choices made for its apps.
    pub fn delete_source(&self, id: &str) -> rusqlite::Result<bool> {
        self.with(|c| {
            c.execute("DELETE FROM app_prefs WHERE source_id = ?1", [id])?;
            Ok(c.execute("DELETE FROM sources WHERE id = ?1", [id])? > 0)
        })
    }

    // ---- app choices ----

    /// Records when apps were first seen in a source (rows that exist are kept).
    pub fn record_apps<'a>(
        &self,
        source_id: &str,
        bundle_ids: impl IntoIterator<Item = &'a str>,
        now: Time,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let mut insert = tx.prepare_cached(
                "INSERT OR IGNORE INTO app_prefs (source_id, bundle_id, first_seen)
                 VALUES (?1, ?2, ?3)",
            )?;
            for bundle_id in bundle_ids {
                insert.execute(params![source_id, bundle_id, now])?;
            }
            drop(insert);
            tx.commit()
        })
    }

    pub fn app_prefs(&self) -> rusqlite::Result<HashMap<(String, String), AppPrefs>> {
        self.with(|c| {
            c.prepare_cached("SELECT * FROM app_prefs")?
                .query_map([], app_prefs_row)?
                .collect()
        })
    }

    /// Shows an app in Browse, hides it, or (`None`) follows its source's mode again.
    pub fn set_app_shown(
        &self,
        source_id: &str,
        bundle_id: &str,
        shown: Option<bool>,
        now: Time,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO app_prefs (source_id, bundle_id, first_seen, shown)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (source_id, bundle_id) DO UPDATE SET shown = ?4",
                params![source_id, bundle_id, now, shown],
            )
            .map(drop)
        })
    }

    pub fn set_app_original(
        &self,
        source_id: &str,
        bundle_id: &str,
        original: &OriginalChoice,
        now: Time,
    ) -> rusqlite::Result<()> {
        let value = match original {
            OriginalChoice::Auto => None,
            OriginalChoice::None => Some(""),
            OriginalChoice::App(b) => Some(b.as_str()),
        };
        self.with(|c| {
            c.execute(
                "INSERT INTO app_prefs (source_id, bundle_id, first_seen, original)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (source_id, bundle_id) DO UPDATE SET original = ?4",
                params![source_id, bundle_id, now, value],
            )
            .map(drop)
        })
    }

    // ---- App Store ----

    /// Cached lookups in one storefront.
    pub fn store_rows(&self, country: &str) -> rusqlite::Result<Vec<StoreRow>> {
        self.with(|c| {
            c.prepare_cached("SELECT * FROM store_apps WHERE country = ?1")?
                .query_map([country], store_row)?
                .collect()
        })
    }

    /// Stores the result of looking up bundle ids: the apps found, and that the others are
    /// not in the App Store (`missing`).
    pub fn save_store_lookup(
        &self,
        country: &str,
        found: &[StoreApp],
        missing: &[String],
        now: Time,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let mut upsert = tx.prepare_cached(
                "INSERT OR REPLACE INTO store_apps (country, bundle_id, track_id, name, developer,
                     version, released_at, icon_url, store_url, checked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for a in found {
                upsert.execute(params![
                    country,
                    a.bundle_id,
                    a.track_id,
                    a.name,
                    a.developer,
                    a.version,
                    a.released_at,
                    a.icon_url,
                    a.store_url,
                    now
                ])?;
            }
            let mut miss = tx.prepare_cached(
                "INSERT OR REPLACE INTO store_apps (country, bundle_id, checked_at)
                 VALUES (?1, ?2, ?3)",
            )?;
            for bundle_id in missing {
                miss.execute(params![country, bundle_id, now])?;
            }
            drop(upsert);
            drop(miss);
            tx.commit()
        })
    }

    // ---- devices ----

    pub fn device(&self, id: &str) -> rusqlite::Result<Option<DeviceRow>> {
        self.with(|c| {
            c.query_row("SELECT * FROM devices WHERE id = ?1", [id], device_row)
                .optional()
        })
    }

    pub fn devices(&self) -> rusqlite::Result<Vec<DeviceRow>> {
        self.with(|c| {
            c.prepare("SELECT * FROM devices ORDER BY last_seen DESC")?
                .query_map([], device_row)?
                .collect()
        })
    }

    /// Inserts a new device; an existing row with the same id is left as is.
    pub fn insert_device(&self, d: &DeviceRow) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO devices (id, kind, name, tailscale_name, os_version,
                     os_version_source, model, devicectl_id, devicectl_name, devicectl_reachable,
                     last_sync_at, last_sync_error, udid, first_seen, last_seen, person_id,
                     platform, detected_platform, onboarded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19)",
                params![
                    d.id,
                    d.kind,
                    d.name,
                    d.tailscale_name,
                    d.os_version,
                    d.os_version_source,
                    d.model,
                    d.devicectl_id,
                    d.devicectl_name,
                    d.devicectl_reachable,
                    d.last_sync_at,
                    d.last_sync_error,
                    d.udid,
                    d.first_seen,
                    d.last_seen,
                    d.person_id,
                    d.platform,
                    d.detected_platform,
                    d.onboarded_at
                ],
            )
            .map(drop)
        })
    }

    // Device updates below each write only the columns their operation owns, so concurrent
    // requests, syncs and renames do not revert each other.

    pub fn touch_device(
        &self,
        id: &str,
        last_seen: Time,
        tailscale_name: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE devices SET last_seen = ?2, tailscale_name = COALESCE(?3, tailscale_name)
                 WHERE id = ?1",
                params![id, last_seen, tailscale_name],
            )
            .map(drop)
        })
    }

    pub fn set_device_name(&self, id: &str, name: Option<&str>) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE devices SET name = ?2 WHERE id = ?1",
                params![id, name],
            )
            .map(drop)
        })
    }

    pub fn set_device_model(&self, id: &str, model: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE devices SET model = ?2 WHERE id = ?1",
                params![id, model],
            )
            .map(drop)
        })
    }

    /// Records an observed OS version unless a higher-priority source (see
    /// `devices::observe_os`) provided the current one.
    pub fn observe_device_os(&self, id: &str, version: &str, source: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE devices SET os_version = ?2, os_version_source = ?3
                 WHERE id = ?1 AND (CASE os_version_source WHEN 'devicectl' THEN 5
                     WHEN 'installer' THEN 4 WHEN 'manual' THEN 3 WHEN 'android_app' THEN 2
                     WHEN 'safari' THEN 1 WHEN 'android' THEN 1 ELSE 0 END)
                     <= (CASE ?3 WHEN 'devicectl' THEN 5 WHEN 'installer' THEN 4 WHEN 'manual' THEN 3
                     WHEN 'android_app' THEN 2 WHEN 'safari' THEN 1 WHEN 'android' THEN 1 ELSE 0 END)",
                params![id, version, source],
            )
            .map(drop)
        })
    }

    /// Records the platform Tailscale or a User-Agent reported, unless one was recorded
    /// before (an iPhone never turns into an Android device).
    pub fn set_detected_platform(&self, id: &str, platform: Platform) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE devices SET detected_platform = ?2 WHERE id = ?1 AND detected_platform IS NULL",
                params![id, platform],
            )
            .map(drop)
        })
    }

    /// The owner's choice of platform; `None` goes back to detection.
    pub fn set_device_platform(
        &self,
        id: &str,
        platform: Option<Platform>,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE devices SET platform = ?2 WHERE id = ?1",
                params![id, platform],
            )
            .map(drop)
        })
    }

    /// Marks the first-run setup done (keeping the first time) or to be shown again.
    pub fn set_device_onboarded(
        &self,
        id: &str,
        onboarded: bool,
        now: Time,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            match onboarded {
                true => c.execute(
                    "UPDATE devices SET onboarded_at = COALESCE(onboarded_at, ?2) WHERE id = ?1",
                    params![id, now],
                ),
                false => c.execute("UPDATE devices SET onboarded_at = NULL WHERE id = ?1", [id]),
            }
            .map(drop)
        })
    }

    /// Sets a manual OS version, or clears it (`None`) when the current one is manual.
    pub fn set_manual_os(&self, id: &str, version: Option<&str>) -> rusqlite::Result<()> {
        self.with(|c| {
            match version {
                Some(v) => c.execute(
                    "UPDATE devices SET os_version = ?2, os_version_source = 'manual' WHERE id = ?1",
                    params![id, v],
                ),
                None => c.execute(
                    "UPDATE devices SET os_version = NULL, os_version_source = NULL
                     WHERE id = ?1 AND os_version_source = 'manual'",
                    [id],
                ),
            }
            .map(drop)
        })
    }

    /// Links a device to a CoreDevice (`Some((id, name, reachable, udid))`) or unlinks it.
    pub fn set_device_link(
        &self,
        id: &str,
        link: Option<(&str, &str, bool, Option<&str>)>,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            match link {
                Some((core, name, reachable, udid)) => c.execute(
                    "UPDATE devices SET devicectl_id = ?2, devicectl_name = ?3,
                         devicectl_reachable = ?4, udid = ?5
                     WHERE id = ?1",
                    params![id, core, name, reachable, udid],
                ),
                None => c.execute(
                    "UPDATE devices SET devicectl_id = NULL, devicectl_name = NULL,
                         devicectl_reachable = 0, last_sync_at = NULL, last_sync_error = NULL,
                         udid = NULL
                     WHERE id = ?1",
                    [id],
                ),
            }
            .map(drop)
        })
    }

    /// Records a devicectl sync of a device that is still linked to `core_id`; false when
    /// it was unlinked or relinked meanwhile.
    pub fn record_device_sync(
        &self,
        id: &str,
        core_id: &str,
        at: Time,
        reachable: bool,
        core_name: Option<&str>,
        error: Option<&str>,
    ) -> rusqlite::Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE devices SET last_sync_at = ?3, devicectl_reachable = ?4,
                     devicectl_name = COALESCE(?5, devicectl_name), last_sync_error = ?6
                 WHERE id = ?1 AND devicectl_id = ?2",
                params![id, core_id, at, reachable, core_name, error],
            )? > 0)
        })
    }

    pub fn set_device_person(&self, id: &str, person_id: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE devices SET person_id = ?2 WHERE id = ?1",
                params![id, person_id],
            )
            .map(drop)
        })
    }

    /// Deletes cookie devices (`local:`) last seen before `before` that have no installs,
    /// install attempts or push subscriptions (e.g. duplicates from concurrent first requests).
    pub fn delete_stale_local_devices(&self, before: Time) -> rusqlite::Result<usize> {
        self.with(|c| {
            let candidates: Vec<(String, Time)> = c
                .prepare(
                    "SELECT id, last_seen FROM devices d WHERE id LIKE 'local:%'
                     AND NOT EXISTS (SELECT 1 FROM installs WHERE device_id = d.id)
                     AND NOT EXISTS (SELECT 1 FROM attempts WHERE device_id = d.id)
                     AND NOT EXISTS (SELECT 1 FROM push_subscriptions WHERE device_id = d.id)",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut deleted = 0;
            for (id, last_seen) in candidates {
                if last_seen < before {
                    deleted += c.execute("DELETE FROM devices WHERE id = ?1", [&id])?;
                    c.execute("DELETE FROM foreign_packages WHERE device_id = ?1", [&id])?;
                }
            }
            Ok(deleted)
        })
    }

    // ---- builds ----

    pub fn save_build(&self, b: &BuildRow) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO builds (id, source_id, bundle_id, app_name, icon_url, version,
                     build, release, artifact_sha256, signed_bundle_id, url_scheme, identity_sha1,
                     profile_uuids, signing_key, expires_at, ipa_path, size, status, step, progress,
                     error, dropped_entitlements, created_at, finished_at, retryable, app_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
                params![
                    b.id,
                    b.source_id,
                    b.bundle_id,
                    b.app_name,
                    b.icon_url,
                    b.version,
                    b.build,
                    to_json(&b.release),
                    b.artifact_sha256,
                    b.signed_bundle_id,
                    b.url_scheme,
                    b.identity_sha1,
                    to_json(&b.profile_uuids),
                    b.signing_key,
                    b.expires_at,
                    b.ipa_path,
                    b.size,
                    b.status.as_str(),
                    b.step,
                    b.progress,
                    b.error,
                    to_json(&b.dropped_entitlements),
                    b.created_at,
                    b.finished_at,
                    b.retryable,
                    b.app_version
                ],
            )
            .map(drop)
        })
    }

    pub fn set_build_progress(
        &self,
        id: &str,
        status: BuildStatus,
        step: Option<&str>,
        progress: Option<f64>,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE builds SET status = ?2, step = ?3, progress = ?4
                 WHERE id = ?1 AND status NOT IN ('ready', 'failed')",
                params![id, status.as_str(), step, progress],
            )
            .map(drop)
        })
    }

    pub fn build(&self, id: &str) -> rusqlite::Result<Option<BuildRow>> {
        self.with(|c| {
            c.prepare_cached("SELECT * FROM builds WHERE id = ?1")?
                .query_row([id], build_row)
                .optional()
        })
    }

    /// All builds, newest first.
    pub fn builds(&self) -> rusqlite::Result<Vec<BuildRow>> {
        self.with(|c| {
            c.prepare_cached("SELECT * FROM builds ORDER BY created_at DESC, rowid DESC")?
                .query_map([], build_row)?
                .collect()
        })
    }

    /// Builds of one app, newest first.
    pub fn app_builds(&self, source_id: &str, bundle_id: &str) -> rusqlite::Result<Vec<BuildRow>> {
        self.with(|c| {
            c.prepare(
                "SELECT * FROM builds WHERE source_id = ?1 AND bundle_id = ?2
                 ORDER BY created_at DESC, rowid DESC",
            )?
            .query_map([source_id, bundle_id], build_row)?
            .collect()
        })
    }

    /// Points signed apps' files under `old` to the same files under `new` (both ending in
    /// `/`): the data directory moved to another computer. Returns how many it changed.
    pub fn move_build_files(&self, old: &str, new: &str) -> rusqlite::Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE builds SET ipa_path = ?2 || substr(ipa_path, length(?1) + 1)
                 WHERE substr(ipa_path, 1, length(?1)) = ?1",
                [old, new],
            )
        })
    }

    pub fn delete_build(&self, id: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM builds WHERE id = ?1", [id])
                .map(drop)
        })
    }

    /// Marks a build failed (retryable) with a small update, for when saving the whole row
    /// failed (for example on a full disk). False when the build does not exist.
    pub fn fail_build(&self, id: &str, error: &str, now: Time) -> rusqlite::Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "UPDATE builds SET status = 'failed', error = ?2, retryable = 1, step = NULL,
                     ipa_path = NULL, finished_at = ?3
                 WHERE id = ?1",
                params![id, error, now],
            )? > 0)
        })
    }

    pub fn clear_retryable(&self, id: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute("UPDATE builds SET retryable = 0 WHERE id = ?1", [id])
                .map(drop)
        })
    }

    /// Marks builds that were queued or running when the server stopped as failed (and
    /// retryable).
    pub fn fail_interrupted_builds(&self, now: Time) -> rusqlite::Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE builds SET status = 'failed', error = 'Interrupted by restart',
                     retryable = 1, step = NULL, finished_at = ?1
                 WHERE status NOT IN ('ready', 'failed')",
                [now],
            )
        })
    }

    // ---- attempts ----

    /// The token for (build, device): created on first use, reused while `keep` says so,
    /// else replaced (token and created_at). Returns the token and the one it replaced.
    pub fn attempt_token(
        &self,
        build_id: &str,
        device_id: &str,
        new_token: impl FnOnce() -> String,
        now: Time,
        keep: impl FnOnce(&AttemptRow) -> bool,
    ) -> rusqlite::Result<(String, Option<String>)> {
        self.with(|c| {
            let existing = c
                .prepare_cached("SELECT * FROM attempts WHERE build_id = ?1 AND device_id = ?2")?
                .query_row([build_id, device_id], attempt_row)
                .optional()?;
            if let Some(attempt) = existing {
                if keep(&attempt) {
                    return Ok((attempt.token, None));
                }
                let renewed = new_token();
                c.execute(
                    "UPDATE attempts SET token = ?1, created_at = ?2 WHERE token = ?3",
                    params![renewed, now, attempt.token],
                )?;
                return Ok((renewed, Some(attempt.token)));
            }
            let token = new_token();
            c.execute(
                "INSERT INTO attempts (token, build_id, device_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![token, build_id, device_id, now],
            )?;
            Ok((token, None))
        })
    }

    pub fn attempt(&self, token: &str) -> rusqlite::Result<Option<AttemptRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT * FROM attempts WHERE token = ?1",
                [token],
                attempt_row,
            )
            .optional()
        })
    }

    /// Attempts of a device with the app (source id, bundle id) of their build.
    pub fn device_attempts(
        &self,
        device_id: &str,
    ) -> rusqlite::Result<Vec<((String, String), AttemptRow)>> {
        self.with(|c| {
            c.prepare(
                "SELECT a.*, b.source_id, b.bundle_id FROM attempts a
                 JOIN builds b ON b.id = a.build_id
                 WHERE a.device_id = ?1",
            )?
            .query_map([device_id], |r| {
                Ok(((r.get("source_id")?, r.get("bundle_id")?), attempt_row(r)?))
            })?
            .collect()
        })
    }

    pub fn attempt_manifest(
        &self,
        token: &str,
        ua: Option<&str>,
        now: Time,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE attempts SET manifest_at = ?2, user_agent = COALESCE(?3, user_agent) WHERE token = ?1",
                params![token, now, ua],
            )
            .map(drop)
        })
    }

    pub fn attempt_download_started(&self, token: &str, now: Time) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE attempts SET download_started_at = ?2 WHERE token = ?1",
                params![token, now],
            )
            .map(drop)
        })
    }

    pub fn attempt_download_completed(
        &self,
        token: &str,
        ua: Option<&str>,
        now: Time,
    ) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE attempts SET download_completed_at = ?2, user_agent = COALESCE(?3, user_agent)
                 WHERE token = ?1",
                params![token, now, ua],
            )
            .map(drop)
        })
    }

    // ---- installs ----

    /// Installs of a device; removed ones only when `include_removed`.
    pub fn installs(
        &self,
        device_id: &str,
        include_removed: bool,
    ) -> rusqlite::Result<Vec<InstallRow>> {
        self.with(|c| installs_in(c, device_id, include_removed))
    }

    /// Reads a device's installs (removed ones too), lets `change` decide which rows to
    /// save, and saves those, in one transaction while holding the connection: a download
    /// completed meanwhile is never overwritten with a row read before it. Returns the saved
    /// rows.
    pub fn change_installs(
        &self,
        device_id: &str,
        change: impl FnOnce(&[InstallRow]) -> Vec<InstallRow>,
    ) -> rusqlite::Result<Vec<InstallRow>> {
        self.save_package_report_with(device_id, None, |installs| (change(installs), None))
    }

    pub fn all_installs(&self) -> rusqlite::Result<Vec<InstallRow>> {
        self.with(|c| {
            c.prepare("SELECT * FROM installs WHERE removed_at IS NULL")?
                .query_map([], install_row)?
                .collect()
        })
    }

    pub fn save_install(&self, i: &InstallRow) -> rusqlite::Result<()> {
        self.with(|c| save_install_in(c, i))
    }

    pub fn delete_install(
        &self,
        device_id: &str,
        signed_bundle_id: &str,
    ) -> rusqlite::Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM installs WHERE device_id = ?1 AND signed_bundle_id = ?2",
                [device_id, signed_bundle_id],
            )? > 0)
        })
    }

    /// Android packages a device has installed with another signature than this Kiwi's
    /// (`PUT /api/me/packages`).
    pub fn foreign_packages(&self, device_id: &str) -> rusqlite::Result<Vec<String>> {
        self.with(|c| {
            c.prepare("SELECT package FROM foreign_packages WHERE device_id = ?1 ORDER BY package")?
                .query_map([device_id], |r| r.get(0))?
                .collect()
        })
    }

    /// Saves a package report like [`Db::change_installs`]: `reconcile` turns the device's
    /// installs into the rows to save and the device's foreign packages, which replace the
    /// previous ones, all in one transaction. Returns the saved rows.
    pub fn save_package_report(
        &self,
        device_id: &str,
        now: Time,
        reconcile: impl FnOnce(&[InstallRow]) -> (Vec<InstallRow>, Vec<String>),
    ) -> rusqlite::Result<Vec<InstallRow>> {
        self.save_package_report_with(device_id, Some(now), |installs| {
            let (rows, foreign) = reconcile(installs);
            (rows, Some(foreign))
        })
    }

    /// [`Db::change_installs`], also replacing the foreign packages when `change` returns
    /// them (seen at `now`).
    fn save_package_report_with(
        &self,
        device_id: &str,
        now: Option<Time>,
        change: impl FnOnce(&[InstallRow]) -> (Vec<InstallRow>, Option<Vec<String>>),
    ) -> rusqlite::Result<Vec<InstallRow>> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let (rows, foreign) = change(&installs_in(&tx, device_id, true)?);
            for row in &rows {
                save_install_in(&tx, row)?;
            }
            if let (Some(foreign), Some(now)) = (foreign, now) {
                tx.execute(
                    "DELETE FROM foreign_packages WHERE device_id = ?1",
                    [device_id],
                )?;
                for package in foreign {
                    tx.execute(
                        "INSERT OR REPLACE INTO foreign_packages (device_id, package, seen_at)
                         VALUES (?1, ?2, ?3)",
                        params![device_id, package, now],
                    )?;
                }
            }
            tx.commit()?;
            Ok(rows)
        })
    }

    // ---- push ----

    pub fn save_subscription(&self, s: &SubscriptionRow) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO push_subscriptions (endpoint, device_id, p256dh, auth, created_at, failures)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)
                 ON CONFLICT (endpoint) DO UPDATE SET device_id = ?2, p256dh = ?3, auth = ?4, failures = 0",
                params![s.endpoint, s.device_id, s.p256dh, s.auth, s.created_at],
            )
            .map(drop)
        })
    }

    pub fn subscriptions(&self, device_id: &str) -> rusqlite::Result<Vec<SubscriptionRow>> {
        self.with(|c| {
            c.prepare("SELECT * FROM push_subscriptions WHERE device_id = ?1")?
                .query_map([device_id], subscription_row)?
                .collect()
        })
    }

    pub fn delete_subscription(
        &self,
        endpoint: &str,
        device_id: Option<&str>,
    ) -> rusqlite::Result<bool> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM push_subscriptions WHERE endpoint = ?1 AND (?2 IS NULL OR device_id = ?2)",
                params![endpoint, device_id],
            )? > 0)
        })
    }

    /// Deletes every subscription (they are bound to the VAPID key). Returns how many.
    pub fn delete_all_subscriptions(&self) -> rusqlite::Result<usize> {
        self.with(|c| c.execute("DELETE FROM push_subscriptions", []))
    }

    pub fn push_delivered(&self, endpoint: &str, now: Time) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE push_subscriptions SET failures = 0, last_success_at = ?2 WHERE endpoint = ?1",
                params![endpoint, now],
            )
            .map(drop)
        })
    }

    /// Increments the failure count and returns it.
    pub fn push_failed(&self, endpoint: &str) -> rusqlite::Result<i64> {
        self.with(|c| {
            c.query_row(
                "UPDATE push_subscriptions SET failures = failures + 1 WHERE endpoint = ?1 RETURNING failures",
                [endpoint],
                |r| r.get(0),
            )
            .optional()
            .map(Option::unwrap_or_default)
        })
    }

    // ---- notifications ----

    pub fn notification_sent(&self, key: &str) -> rusqlite::Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT 1 FROM notifications_sent WHERE key = ?1",
                [key],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
        })
    }

    pub fn mark_notification(&self, key: &str, now: Time) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO notifications_sent (key, sent_at) VALUES (?1, ?2)",
                params![key, now],
            )
            .map(drop)
        })
    }

    pub fn prune_notifications(&self, before: Time) -> rusqlite::Result<()> {
        self.with(|c| {
            let keys: Vec<(String, Time)> = c
                .prepare("SELECT key, sent_at FROM notifications_sent")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (key, sent) in keys {
                if sent < before {
                    c.execute("DELETE FROM notifications_sent WHERE key = ?1", [key])?;
                }
            }
            Ok(())
        })
    }

    // ---- settings and meta ----

    pub fn settings(&self) -> rusqlite::Result<Vec<(String, Value)>> {
        self.with(|c| {
            c.prepare("SELECT key, value FROM settings")?
                .query_map([], |r| {
                    let value: String = r.get(1)?;
                    Ok((
                        r.get(0)?,
                        serde_json::from_str(&value).unwrap_or(Value::Null),
                    ))
                })?
                .collect()
        })
    }

    pub fn save_settings(&self, pairs: &[(String, Value)]) -> rusqlite::Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let mut upsert =
                tx.prepare_cached("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)")?;
            for (key, value) in pairs {
                upsert.execute(params![key, value.to_string()])?;
            }
            drop(upsert);
            tx.commit()
        })
    }

    pub fn meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.with(|c| {
            c.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                .optional()
        })
    }

    pub fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                [key, value],
            )
            .map(drop)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn migration_5_drops_the_family_and_keeps_device_users() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kiwi.db");
        {
            // A database from a Kiwi with the family (schema 4).
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..4] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 4).unwrap();
            conn.execute_batch(
                "INSERT INTO people (id, name, created_at, last_seen)
                     VALUES ('alex@example.com', 'Alex', '2026-10-29T00:00:00Z', '2026-10-29T00:00:00Z');
                 INSERT INTO devices (id, kind, first_seen, last_seen, person_id)
                     VALUES ('ts:n1', 'tailscale', '2026-10-29T00:00:00Z', '2026-10-29T00:00:00Z',
                         'alex@example.com');",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i64);
        let gone = db
            .with(|c| {
                c.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name IN
                         ('people', 'app_access', 'registrations', 'enrollments')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
            })
            .unwrap();
        assert_eq!(gone, 0);
        let device = db.device("ts:n1").unwrap().unwrap();
        assert_eq!(device.person_id.as_deref(), Some("alex@example.com"));
        let now = Utc::now();
        db.save_build(&crate::testutil::build_row("b1", BuildStatus::Ready, now))
            .unwrap();
        assert_eq!(db.builds().unwrap().len(), 1);
    }

    #[test]
    fn migration_6_detects_platforms_and_keeps_devices_in_use_set_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kiwi.db");
        {
            // A database from a Kiwi before device platforms (schema 5).
            let conn = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..5] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 5).unwrap();
            conn.execute_batch(
                "INSERT INTO devices (id, kind, model, os_version, os_version_source, first_seen, last_seen)
                 VALUES
                     ('ts:iphone', 'tailscale', 'iPhone19,4', '27.0.1', 'installer',
                         '2026-10-01T00:00:00Z', '2026-11-01T00:00:00Z'),
                     ('ts:android', 'tailscale', 'Android', NULL, NULL,
                         '2026-10-02T00:00:00Z', '2026-11-01T00:00:00Z'),
                     ('local:desktop', 'browser', NULL, NULL, NULL,
                         '2026-10-03T00:00:00Z', '2026-11-01T00:00:00Z'),
                     ('ts:new', 'tailscale', NULL, '27.0', 'safari',
                         '2026-10-04T00:00:00Z', '2026-11-01T00:00:00Z'),
                     ('ts:blank', 'tailscale', NULL, NULL, NULL,
                         '2026-10-05T00:00:00Z', '2026-11-01T00:00:00Z');
                 INSERT INTO installs (device_id, signed_bundle_id, source_id, bundle_id, name,
                     version, origin, installed_at)
                 VALUES ('ts:iphone', 'com.example.app.A1B2C3D4E5', 'src', 'com.example.app',
                     'Example', '1.0', 'kiwi', '2026-10-10T00:00:00Z');
                 INSERT INTO builds (id, source_id, bundle_id, app_name, version, release,
                     signed_bundle_id, status, created_at)
                 VALUES ('b1', 'src', 'com.example.app', 'Example', '1.0', '{}',
                     'com.example.app', 'ready', '2026-10-10T00:00:00Z');
                 INSERT INTO attempts (token, build_id, device_id, created_at)
                 VALUES ('tok', 'b1', 'ts:android', '2026-10-10T00:00:00Z');
                 INSERT INTO push_subscriptions (endpoint, device_id, p256dh, auth, created_at)
                 VALUES ('https://web.push.apple.com/x', 'local:desktop', 'k', 'a',
                     '2026-10-10T00:00:00Z');",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i64);
        let state = |id: &str| {
            let d = db.device(id).unwrap().unwrap();
            (d.platform, d.detected_platform, d.onboarded_at)
        };
        let at = |day: u32| {
            Some("2026-10-0".to_owned() + &day.to_string() + "T00:00:00Z")
                .map(|t| t.parse::<Time>().unwrap())
        };
        assert_eq!(state("ts:iphone"), (None, Some(Platform::Ios), at(1)));
        assert_eq!(state("ts:android"), (None, Some(Platform::Android), at(2)));
        assert_eq!(state("local:desktop"), (None, None, at(3)));
        assert_eq!(state("ts:new"), (None, Some(Platform::Ios), None));
        assert_eq!(state("ts:blank"), (None, None, None));
        assert!(db.foreign_packages("ts:android").unwrap().is_empty());
    }

    #[test]
    fn device_platform_columns() {
        let db = Db::open_in_memory().unwrap();
        let now = Utc::now();
        db.insert_device(&DeviceRow::new("ts:n1".into(), "tailscale", now))
            .unwrap();
        // The first report wins.
        db.set_detected_platform("ts:n1", Platform::Ios).unwrap();
        db.set_detected_platform("ts:n1", Platform::Android)
            .unwrap();
        db.set_device_platform("ts:n1", Some(Platform::Android))
            .unwrap();
        let d = db.device("ts:n1").unwrap().unwrap();
        assert_eq!(
            (d.platform, d.detected_platform),
            (Some(Platform::Android), Some(Platform::Ios))
        );
        db.set_device_platform("ts:n1", None).unwrap();
        assert_eq!(db.device("ts:n1").unwrap().unwrap().platform, None);
        // Set up keeps its first time; showing the setup again clears it.
        let later = now + chrono::Duration::hours(1);
        db.set_device_onboarded("ts:n1", true, now).unwrap();
        db.set_device_onboarded("ts:n1", true, later).unwrap();
        let onboarded = db.device("ts:n1").unwrap().unwrap().onboarded_at;
        assert_eq!(onboarded, Some(now));
        db.set_device_onboarded("ts:n1", false, later).unwrap();
        assert_eq!(db.device("ts:n1").unwrap().unwrap().onboarded_at, None);
        // A package report replaces the foreign packages.
        let foreign = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        db.save_package_report("ts:n1", now, |_| (Vec::new(), foreign(&["b.b", "a.a"])))
            .unwrap();
        assert_eq!(db.foreign_packages("ts:n1").unwrap(), ["a.a", "b.b"]);
        db.save_package_report("ts:n1", now, |_| (Vec::new(), foreign(&["c.c"])))
            .unwrap();
        assert_eq!(db.foreign_packages("ts:n1").unwrap(), ["c.c"]);
    }

    #[test]
    fn a_download_completed_during_a_report_is_not_overwritten() {
        use std::sync::{Arc, mpsc};
        let db = Arc::new(Db::open_in_memory().unwrap());
        let now = Utc::now();
        let mut old = crate::testutil::install_of(
            &crate::testutil::release("1.0", None),
            "kiwi",
            now - Duration::hours(1),
        );
        old.verified = true;
        db.save_install(&old).unwrap();
        let new = InstallRow {
            version: "2.0".into(),
            verified: false,
            installed_at: now,
            ..old.clone()
        };
        // The download of 2.0 completes while a package report of the phone is reconciled
        // with the installs it read (1.0, now missing from the phone).
        let (started, completes) = mpsc::channel();
        let download = std::thread::spawn({
            let db = db.clone();
            let new = new.clone();
            move || {
                completes.recv().unwrap();
                db.save_install(&new).unwrap();
            }
        });
        let saved = db
            .change_installs("ts:test", |installs| {
                started.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(50));
                installs
                    .iter()
                    .map(|i| InstallRow {
                        removed_at: Some(now),
                        verified: false,
                        ..i.clone()
                    })
                    .collect()
            })
            .unwrap();
        assert_eq!(saved.len(), 1);
        download.join().unwrap();
        // The report's change came first; the completed download is what stays.
        assert_eq!(db.installs("ts:test", true).unwrap(), [new]);
    }

    #[test]
    fn catalog_and_store_batches_roll_back_on_failure() {
        let db = Db::open_in_memory().unwrap();
        let now = Utc::now();
        db.execute_batch(
            "CREATE TRIGGER reject_app BEFORE INSERT ON app_prefs
             WHEN NEW.bundle_id = 'bad' BEGIN SELECT RAISE(ABORT, 'rejected'); END;
             CREATE TRIGGER reject_store BEFORE INSERT ON store_apps
             WHEN NEW.bundle_id = 'bad' BEGIN SELECT RAISE(ABORT, 'rejected'); END;",
        )
        .unwrap();
        db.record_apps("src", ["existing"], now).unwrap();
        assert!(db.record_apps("src", ["new", "bad"], now).is_err());
        let prefs = db.app_prefs().unwrap();
        assert_eq!(prefs.len(), 1);
        assert!(prefs.contains_key(&("src".into(), "existing".into())));

        db.save_store_lookup("nl", &[], &["existing".into()], now)
            .unwrap();
        let app = StoreApp {
            bundle_id: "found".into(),
            track_id: 1,
            name: "Found".into(),
            developer: None,
            version: "1".into(),
            released_at: None,
            icon_url: None,
            store_url: "https://example.com".into(),
        };
        assert!(
            db.save_store_lookup("nl", &[app], &["missing".into(), "bad".into()], now)
                .is_err()
        );
        let rows = db.store_rows("nl").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bundle_id, "existing");
    }

    #[test]
    fn settings_batch_rolls_back_on_failure() {
        let db = Db::open_in_memory().unwrap();
        db.save_settings(&[("existing".into(), Value::Bool(true))])
            .unwrap();
        db.execute_batch(
            "CREATE TRIGGER reject_setting BEFORE INSERT ON settings
             WHEN NEW.key = 'bad' BEGIN SELECT RAISE(ABORT, 'rejected'); END;",
        )
        .unwrap();
        assert!(
            db.save_settings(&[
                ("existing".into(), Value::Bool(false)),
                ("bad".into(), Value::Null),
            ])
            .is_err()
        );
        assert_eq!(
            db.settings().unwrap(),
            [("existing".into(), Value::Bool(true))]
        );
    }

    #[test]
    fn migrates_fresh_and_existing_databases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kiwi.db");
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i64);
        db.set_meta("team_id", "TEAM").unwrap();
        drop(db);
        // Reopening applies nothing and keeps data.
        let db = Db::open(&path).unwrap();
        assert_eq!(db.meta("team_id").unwrap().as_deref(), Some("TEAM"));
        drop(db);
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        drop(conn);
        let err = Db::open(&path).err().unwrap();
        assert!(err.contains("newer than this Kiwi"), "{err}");
    }

    #[test]
    fn migration_2_marks_interrupted_builds_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kiwi.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        let release = r#"{"version":"1.0","download_url":"https://example.com/a.ipa"}"#;
        for (id, status, error) in [
            ("ready", "ready", None),
            ("interrupted", "failed", Some("Interrupted by restart")),
            ("signing", "failed", Some("codesign failed")),
        ] {
            conn.execute(
                "INSERT INTO builds (id, source_id, bundle_id, app_name, version, release,
                     signed_bundle_id, status, error, created_at)
                 VALUES (?1, 's', 'com.example.app', 'Example', '1.0', ?2, 'com.example.app.T',
                     ?3, ?4, '2026-10-29 09:47:15.161732+00:00')",
                params![id, release, status, error],
            )
            .unwrap();
        }
        drop(conn);
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i64);
        let retryable = |id: &str| db.build(id).unwrap().unwrap().retryable;
        assert!(retryable("interrupted"));
        assert!(!retryable("signing") && !retryable("ready"));
        assert_eq!(
            db.build("ready").unwrap().unwrap().status,
            BuildStatus::Ready
        );
    }

    #[test]
    fn migration_3_adds_browse_modes_and_app_versions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kiwi.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.execute_batch(MIGRATIONS[1]).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        let row = crate::testutil::source_row("src", Vec::new());
        conn.execute(
            "INSERT INTO sources (id, url, position, added_at, source) VALUES ('src', ?1, 0, ?2, ?3)",
            params![row.url, row.added_at, to_json(&row.source)],
        )
        .unwrap();
        drop(conn);
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i64);
        let source = &db.sources().unwrap()[0];
        assert_eq!((source.browse, source.picked_at), (BrowseMode::All, None));
        let mut build = crate::testutil::build_row("b1", BuildStatus::Ready, Utc::now());
        build.app_version = Some("21.43.5".into());
        db.save_build(&build).unwrap();
        assert_eq!(
            db.build("b1").unwrap().unwrap().app_version.as_deref(),
            Some("21.43.5")
        );
    }

    #[test]
    fn app_choices_follow_the_browse_mode() {
        let db = Db::open_in_memory().unwrap();
        let now = Utc::now();
        db.save_source(&crate::testutil::source_row("src", Vec::new()))
            .unwrap();
        db.record_apps("src", ["a", "b"], now).unwrap();
        db.set_app_shown("src", "a", Some(false), now).unwrap();
        // Recording again keeps choices and when apps were first seen.
        let later = now + Duration::hours(2);
        db.record_apps("src", ["a", "b", "c"], later).unwrap();
        let prefs = db.app_prefs().unwrap();
        let get = |b: &str| prefs.get(&("src".to_owned(), b.to_owned()));
        assert_eq!(get("a").unwrap().first_seen, now);
        assert_eq!(get("c").unwrap().first_seen, later);
        let source = db.sources().unwrap().remove(0);
        assert!(!source.shows(get("a")) && source.shows(get("b")) && source.shows(None));
        assert!(!source.is_new(get("c")));

        // Picking apps: only those turned on are shown; apps seen since are new.
        let picked = now + Duration::hours(1);
        db.set_source_browse("src", BrowseMode::Picked, Some(picked))
            .unwrap();
        let source = db.sources().unwrap().remove(0);
        assert_eq!(
            (source.browse, source.picked_at),
            (BrowseMode::Picked, Some(picked))
        );
        assert!(!source.shows(get("b")) && !source.shows(get("c")));
        assert!(source.is_new(get("c")) && !source.is_new(get("b")) && !source.is_new(get("a")));
        db.set_app_shown("src", "c", Some(true), later).unwrap();
        let prefs = db.app_prefs().unwrap();
        let c = prefs.get(&("src".to_owned(), "c".to_owned()));
        assert!(source.shows(c) && !source.is_new(c));

        for choice in [
            OriginalChoice::App("com.google.ios.youtube".into()),
            OriginalChoice::None,
            OriginalChoice::Auto,
        ] {
            db.set_app_original("src", "b", &choice, now).unwrap();
            let prefs = db.app_prefs().unwrap();
            assert_eq!(prefs[&("src".to_owned(), "b".to_owned())].original, choice);
        }
        // A choice for an app not recorded yet creates its row.
        db.set_app_shown("src", "d", Some(true), now).unwrap();
        assert_eq!(db.app_prefs().unwrap().len(), 4);

        assert!(db.delete_source("src").unwrap());
        assert!(db.app_prefs().unwrap().is_empty());
    }

    #[test]
    fn store_lookups_are_cached_per_country() {
        let db = Db::open_in_memory().unwrap();
        let now = Utc::now();
        let youtube = StoreApp {
            bundle_id: "com.google.ios.youtube".into(),
            track_id: 544007664,
            name: "YouTube".into(),
            developer: Some("Google".into()),
            version: "21.43.5".into(),
            released_at: Some(now),
            icon_url: None,
            store_url: "https://apps.apple.com/nl/app/youtube/id544007664".into(),
        };
        db.save_store_lookup(
            "nl",
            std::slice::from_ref(&youtube),
            &["kh.crysalis.feather".into()],
            now,
        )
        .unwrap();
        let mut rows = db.store_rows("nl").unwrap();
        rows.sort_by(|a, b| a.bundle_id.cmp(&b.bundle_id));
        assert_eq!(rows[0].app.as_ref(), Some(&youtube));
        assert_eq!(
            (rows[1].bundle_id.as_str(), rows[1].app.as_ref()),
            ("kh.crysalis.feather", None)
        );
        assert!(db.store_rows("us").unwrap().is_empty());
        // An app that left the App Store is recorded as missing.
        db.save_store_lookup("nl", &[], std::slice::from_ref(&youtube.bundle_id), now)
            .unwrap();
        assert!(db.store_rows("nl").unwrap().iter().all(|r| r.app.is_none()));
    }

    #[test]
    fn device_updates_write_only_their_columns() {
        let db = Db::open_in_memory().unwrap();
        let now = Utc::now();
        db.insert_device(&DeviceRow::new("ts:a".into(), "tailscale", now))
            .unwrap();
        let get = || db.device("ts:a").unwrap().unwrap();
        db.set_device_name("ts:a", Some("Phone")).unwrap();
        db.touch_device("ts:a", now, Some("iphone194")).unwrap();
        db.touch_device("ts:a", now, None).unwrap();
        assert_eq!(
            (get().name.as_deref(), get().tailscale_name.as_deref()),
            (Some("Phone"), Some("iphone194"))
        );
        // Inserting again keeps the row.
        db.insert_device(&DeviceRow::new("ts:a".into(), "tailscale", now))
            .unwrap();
        assert_eq!(get().name.as_deref(), Some("Phone"));

        // OS versions never downgrade their source.
        db.observe_device_os("ts:a", "27.0", "safari").unwrap();
        db.set_manual_os("ts:a", Some("27.0.1")).unwrap();
        db.observe_device_os("ts:a", "27.1", "safari").unwrap();
        assert_eq!(get().os_version.as_deref(), Some("27.0.1"));
        db.observe_device_os("ts:a", "27.0.2", "installer").unwrap();
        db.set_manual_os("ts:a", None).unwrap();
        assert_eq!(
            (
                get().os_version.as_deref(),
                get().os_version_source.as_deref()
            ),
            (Some("27.0.2"), Some("installer"))
        );

        // A sync result is only recorded while the device is still linked to that CoreDevice.
        db.set_device_link("ts:a", Some(("core1", "iPhone", true, Some("udid"))))
            .unwrap();
        assert!(
            db.record_device_sync("ts:a", "core1", now, false, None, Some("Not reachable"))
                .unwrap()
        );
        let d = get();
        assert_eq!(
            (
                d.devicectl_name.as_deref(),
                d.devicectl_reachable,
                d.last_sync_error.as_deref()
            ),
            (Some("iPhone"), false, Some("Not reachable"))
        );
        db.set_device_link("ts:a", None).unwrap();
        assert!(
            !db.record_device_sync("ts:a", "core1", now, true, Some("x"), None)
                .unwrap()
        );
        let d = get();
        assert_eq!((d.devicectl_id, d.last_sync_at, d.udid), (None, None, None));
        assert_eq!(d.name.as_deref(), Some("Phone"));
    }

    #[test]
    fn stale_unused_local_devices_are_deleted() {
        let db = Db::open_in_memory().unwrap();
        let now = Utc::now();
        let old = now - Duration::hours(25);
        for (id, seen) in [
            ("local:stale", old),
            ("local:recent", now - Duration::hours(1)),
            ("local:installed", old),
            ("local:attempted", old),
            ("local:subscribed", old),
            ("ts:phone", old),
        ] {
            let mut d = DeviceRow::new(id.into(), "browser", old);
            d.last_seen = seen;
            db.insert_device(&d).unwrap();
        }
        let mut install =
            crate::testutil::install_of(&crate::testutil::release("1.0", None), "kiwi", now);
        install.device_id = "local:installed".into();
        db.save_install(&install).unwrap();
        db.save_build(&crate::testutil::build_row("b1", BuildStatus::Ready, now))
            .unwrap();
        db.attempt_token("b1", "local:attempted", || "tok".into(), now, |_| true)
            .unwrap();
        db.save_subscription(&SubscriptionRow {
            endpoint: "https://web.push.apple.com/x".into(),
            device_id: "local:subscribed".into(),
            p256dh: "k".into(),
            auth: "a".into(),
            created_at: now,
            last_success_at: None,
            failures: 0,
        })
        .unwrap();
        assert_eq!(
            db.delete_stale_local_devices(now - Duration::hours(24))
                .unwrap(),
            1
        );
        let ids: Vec<String> = db.devices().unwrap().into_iter().map(|d| d.id).collect();
        assert_eq!(ids.len(), 5);
        assert!(!ids.contains(&"local:stale".to_owned()));
    }

    #[test]
    fn source_fetches_keep_enabled() {
        let db = Db::open_in_memory().unwrap();
        let mut row = crate::testutil::source_row("src", Vec::new());
        db.save_source(&row).unwrap();
        db.set_source_enabled("src", false).unwrap();
        row.source.catalog.name = "Renamed".into();
        assert!(
            db.save_source_fetch("src", Utc::now(), &row.source)
                .unwrap()
        );
        let saved = &db.sources().unwrap()[0];
        assert!(!saved.enabled && !saved.source.enabled);
        assert_eq!(saved.source.catalog.name, "Renamed");
        assert!(saved.last_attempt_at.is_some());
        assert!(
            !db.save_source_fetch("gone", Utc::now(), &row.source)
                .unwrap()
        );
    }

    #[test]
    fn attempts_are_reused_per_build_and_device() {
        let db = Db::open_in_memory().unwrap();
        let now = Utc::now();
        db.save_build(&crate::testutil::build_row("b1", BuildStatus::Ready, now))
            .unwrap();
        let keep = |_: &AttemptRow| true;
        let t1 = db
            .attempt_token("b1", "d1", || "tok1".into(), now, keep)
            .unwrap();
        let t2 = db
            .attempt_token("b1", "d1", || "tok2".into(), now, keep)
            .unwrap();
        let t3 = db
            .attempt_token("b1", "d2", || "tok3".into(), now, keep)
            .unwrap();
        assert_eq!(
            (t1.0.as_str(), t2.0.as_str(), t3.0.as_str()),
            ("tok1", "tok1", "tok3")
        );
        assert_eq!((t1.1, t2.1), (None, None));
        db.attempt_manifest("tok1", Some("ua"), now).unwrap();
        let attempts = db.device_attempts("d1").unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(
            attempts[0].0,
            ("src".to_owned(), "com.example.app".to_owned())
        );
        assert_eq!(attempts[0].1.user_agent.as_deref(), Some("ua"));
        // A token `keep` turns down is replaced, and the old one reported.
        let later = now + chrono::Duration::hours(13);
        assert_eq!(
            db.attempt_token("b1", "d1", || "tok5".into(), later, |a| a.token != "tok1")
                .unwrap(),
            ("tok5".to_owned(), Some("tok1".to_owned()))
        );
        assert!(db.attempt("tok1").unwrap().is_none());
        assert_eq!(db.attempt("tok5").unwrap().unwrap().created_at, later);
        // Deleting a build removes its attempts.
        db.delete_build("b1").unwrap();
        assert!(db.attempt("tok5").unwrap().is_none());
    }

    #[test]
    fn attempt_progress_window() {
        let now = Utc::now();
        let minutes = |m| now - Duration::minutes(m);
        let mut a = AttemptRow {
            token: "t".into(),
            build_id: "b".into(),
            device_id: "d".into(),
            created_at: minutes(30),
            manifest_at: None,
            download_started_at: None,
            download_completed_at: None,
            user_agent: None,
        };
        let window = Duration::minutes(10);
        assert!(!a.in_progress(now, window));
        a.manifest_at = Some(minutes(2));
        assert!(a.in_progress(now, window));
        a.download_started_at = Some(minutes(1));
        a.download_completed_at = Some(now);
        assert!(!a.in_progress(now, window));
        // A new manifest fetch after a completed install starts a new attempt.
        a.manifest_at = Some(now + Duration::seconds(1));
        assert!(a.in_progress(now + Duration::seconds(2), window));
        a.download_completed_at = None;
        a.manifest_at = Some(minutes(11));
        a.download_started_at = None;
        assert!(!a.in_progress(now, window));
    }

    #[test]
    fn interrupted_builds_fail_and_notifications_dedupe() {
        let db = Db::open_in_memory().unwrap();
        let now = Utc::now();
        for (id, status) in [
            ("a", BuildStatus::Queued),
            ("b", BuildStatus::Signing),
            ("c", BuildStatus::Ready),
        ] {
            db.save_build(&crate::testutil::build_row(id, status, now))
                .unwrap();
        }
        assert_eq!(db.fail_interrupted_builds(now).unwrap(), 2);
        let b = db.build("b").unwrap().unwrap();
        assert_eq!(b.status, BuildStatus::Failed);
        assert_eq!(b.error.as_deref(), Some("Interrupted by restart"));
        assert!(b.retryable);
        assert_eq!(db.build("c").unwrap().unwrap().status, BuildStatus::Ready);

        assert!(!db.notification_sent("k").unwrap());
        db.mark_notification("k", now - Duration::days(200))
            .unwrap();
        db.mark_notification("k2", now).unwrap();
        assert!(db.notification_sent("k").unwrap());
        db.prune_notifications(now - Duration::days(180)).unwrap();
        assert!(!db.notification_sent("k").unwrap());
        assert!(db.notification_sent("k2").unwrap());
    }
}
