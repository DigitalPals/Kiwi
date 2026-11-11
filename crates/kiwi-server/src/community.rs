//! Community builds: Android apps patched with bundles from the Morphe community, listed in
//! the Morphe index (see docs/kiwi-builds.md, "Community builds").
//!
//! The index is discovery data, fetched and validated here ([`index`]). A build runs in the
//! Android runner (`android.py community`): it checks the top candidates' latest releases,
//! inspects their bundles in the sandbox, picks one, fetches the original from Google Play
//! and patches it in the sandbox, which is required: community bundles are third-party code
//! and never run unconfined. The first successful build pins the bundle's release, its
//! digest, the target version and the publisher's certificate; later builds must match the
//! pins. Community apps are never rebuilt on a schedule: a daily check offers newer
//! releases, and the owner approves each one.
//!
//! Published builds join the built-in Kiwi Builds source, so installing them works like
//! installing Kiwi's own Android builds.
pub mod index;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use kiwi_source::{CancellationToken, Release, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Kiwi,
    automation::{self, AppState, Failure, Invocation, Job, Limitation, Preferences},
    db::Time,
    error::{ApiError, ApiResult},
};
pub use index::{Index, valid_package, valid_repo};

/// A community app's id in Kiwi Builds: this prefix and its package.
pub const ID_PREFIX: &str = "community-";
pub const INDEX_URL: &str = "https://morphe-patches.software/data/bundles.json";
pub const GITHUB_API: &str = "https://api.github.com";
/// What macOS (and any system but Linux) shows instead of Install.
pub const NEEDS_LINUX: &str = "Community builds need Kiwi on Linux or in Docker, where Kiwi can lock the patcher in a sandbox.";
const LOADING: &str = "Kiwi is loading the community list…";
const LOAD_FAILED: &str =
    "Kiwi couldn't load the community list yet. It tries again in a few minutes.";
/// Largest index Kiwi downloads.
const MAX_INDEX_BYTES: u64 = 32 * 1024 * 1024;
/// How often Kiwi asks for a newer index; with no copy yet, how soon it retries.
const REFRESH_EVERY: chrono::Duration = chrono::Duration::hours(12);
const RETRY_EMPTY: chrono::Duration = chrono::Duration::minutes(15);
/// A copy older than this is shown as stale.
const STALE_AFTER: chrono::Duration = chrono::Duration::hours(24);
const UPDATE_CHECK_EVERY: chrono::Duration = chrono::Duration::hours(24);
/// Releases younger than this are not offered yet.
pub const UPDATE_MIN_AGE: chrono::Duration = chrono::Duration::hours(72);
/// Releases kept in the source per community app.
pub const RETAIN: usize = 2;
/// Candidates the runner gets for an automatic pick; it inspects at most three.
const CANDIDATES: usize = 6;
pub const POPULAR: usize = 30;

pub fn is_id(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

/// The package of a valid community id.
pub fn package_of(id: &str) -> Option<&str> {
    id.strip_prefix(ID_PREFIX).filter(|p| valid_package(p))
}

pub fn id_of(package: &str) -> String {
    format!("{ID_PREFIX}{package}")
}

/// A release tag Kiwi accepts from GitHub: it goes into URLs and names.
pub fn valid_tag(tag: &str) -> bool {
    (1..=128).contains(&tag.len())
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-' | b'/'))
        && tag
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && !tag.contains("..")
}

// --- persisted state ------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chosen {
    #[default]
    Auto,
    Manual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatchInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// What the first successful build fixed; later builds must match it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pins {
    pub repo: String,
    pub repo_id: u64,
    pub author: String,
    pub tag: String,
    #[serde(default)]
    pub published_at: Option<String>,
    pub asset: String,
    pub asset_sha256: String,
    pub bundle_name: String,
    /// The original's version name and code.
    pub version: String,
    pub version_code: u64,
    /// What the patched app installs as (learned from the first build).
    pub output_package: String,
    pub output_version_code: u64,
    pub patches: Vec<PatchInfo>,
    pub chosen: Chosen,
    pub pinned_at: Time,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Offered {
    pub tag: String,
    pub published_at: String,
}

/// What a build learned about one candidate bundle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Learned {
    pub tag: Option<String>,
    pub versions: Vec<String>,
    pub patches: Option<usize>,
    pub default_patches: Option<usize>,
    pub eligible: Option<bool>,
    /// Why it can't be used, at `tag`.
    pub failed: Option<String>,
}

/// A community app the owner installed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Added {
    pub package: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    /// The publisher's certificate (SHA-256), first seen on an original from Google Play.
    pub certificate: Option<String>,
    pub pins: Option<Pins>,
    pub update: Option<Offered>,
    pub update_checked_at: Option<Time>,
    /// By repository.
    pub learned: BTreeMap<String, Learned>,
}

/// What a community job was asked to do.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobIntent {
    /// The bundle the owner chose (manual override).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The release the owner approved as an update.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
}

/// Community apps' preferences: no schedule, no options.
pub fn preferences() -> Preferences {
    Preferences {
        enabled: false,
        interval_minutes: 1440,
        tweaks: Vec::new(),
        name: None,
    }
}

// --- the index --------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Status {
    etag: Option<String>,
    last_modified: Option<String>,
    /// When the copy on disk was last confirmed current.
    checked_at: Option<Time>,
    attempted_at: Option<Time>,
    failed: bool,
}

pub struct Loaded {
    pub index: Index,
}

/// The community index and the sandbox check, shared by the routes and the worker.
pub struct Community {
    /// Injectable for tests, like the runners' paths.
    pub index_url: RwLock<String>,
    pub github_api: RwLock<String>,
    dir: PathBuf,
    loaded: RwLock<Option<Arc<Loaded>>>,
    status: Mutex<Status>,
    /// `Err(reason)` where community builds can't run; `None` until checked.
    sandbox: RwLock<Option<Result<(), String>>>,
    busy: AtomicBool,
    /// Whether Kiwi refreshes the list and checks for updates by itself (tests turn it off
    /// and call [`refresh`] and [`check_updates`]).
    pub background: AtomicBool,
}

/// The packages Kiwi Builds builds itself: not offered as community apps.
pub fn official_packages() -> BTreeSet<String> {
    automation::recipes()
        .iter()
        .filter(|r| r.android())
        .flat_map(|r| [Some(r.bundle_id.clone()), r.original_package.clone()])
        .flatten()
        .collect()
}

impl Community {
    /// Loads the last good index from `<data>/community`, if any.
    pub fn load(data: &std::path::Path) -> Self {
        let dir = data.join("community");
        let status: Status = std::fs::read(dir.join("status.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let loaded = std::fs::read(dir.join("bundles.json"))
            .ok()
            .and_then(|bytes| match index::parse(&bytes, &official_packages()) {
                Ok(index) => Some(Arc::new(Loaded { index })),
                Err(e) => {
                    tracing::warn!("the saved community list is unusable: {e}");
                    None
                }
            });
        Self {
            index_url: RwLock::new(INDEX_URL.into()),
            github_api: RwLock::new(GITHUB_API.into()),
            dir,
            status: Mutex::new(if loaded.is_some() {
                status
            } else {
                Status::default()
            }),
            loaded: RwLock::new(loaded),
            sandbox: RwLock::new(None),
            busy: AtomicBool::new(false),
            background: AtomicBool::new(true),
        }
    }

    pub fn index(&self) -> Option<Arc<Loaded>> {
        self.loaded.read().unwrap().clone()
    }

    /// Sets the result of the sandbox check (tests stand in for other computers).
    pub fn set_sandbox(&self, result: Result<(), String>) {
        *self.sandbox.write().unwrap() = Some(result);
    }

    fn save_status(&self, status: &Status) {
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&self.dir)?;
            let tmp = self.dir.join("status.json.new");
            std::fs::write(&tmp, serde_json::to_vec(status).unwrap_or_default())?;
            std::fs::rename(tmp, self.dir.join("status.json"))
        };
        if let Err(e) = write() {
            tracing::warn!("could not save the community list status: {e}");
        }
    }

    fn due(&self, now: Time) -> bool {
        let status = self.status.lock().unwrap();
        let every = if self.index().is_some() {
            REFRESH_EVERY
        } else {
            RETRY_EMPTY
        };
        status.attempted_at.is_none_or(|t| now - t >= every)
    }
}

/// Fetches the index when it is due (or `force`), keeping the last good copy on failure.
pub async fn refresh(kiwi: &Kiwi, force: bool) -> Result<(), String> {
    let community = &kiwi.community;
    let now = Utc::now();
    if !force && !community.due(now) {
        return Ok(());
    }
    let url = Url::parse(&community.index_url.read().unwrap()).map_err(|e| e.to_string())?;
    let validators = {
        let mut status = community.status.lock().unwrap();
        status.attempted_at = Some(now);
        if community.index().is_some() {
            (status.etag.clone(), status.last_modified.clone())
        } else {
            (None, None)
        }
    };
    let fetched = kiwi
        .sources
        .json_document(&url, MAX_INDEX_BYTES, validators, &kiwi.cancel)
        .await;
    let result = match fetched {
        Ok(None) => Ok(None),
        Ok(Some(document)) => {
            let bytes = document.bytes.clone();
            match tokio::task::spawn_blocking(move || index::parse(&bytes, &official_packages()))
                .await
            {
                Ok(Ok(index)) => Ok(Some((index, document))),
                Ok(Err(e)) => Err(format!("the community list is not usable: {e}")),
                Err(e) => Err(e.to_string()),
            }
        }
        Err(e) => Err(e.to_string()),
    };
    let mut status = community.status.lock().unwrap().clone();
    let outcome = match result {
        Ok(None) => {
            status.checked_at = Some(now);
            status.failed = false;
            Ok(())
        }
        Ok(Some((index, document))) => {
            let saved = std::fs::create_dir_all(&community.dir).and_then(|()| {
                let tmp = community.dir.join("bundles.json.new");
                std::fs::write(&tmp, &document.bytes)?;
                std::fs::rename(tmp, community.dir.join("bundles.json"))
            });
            if let Err(e) = saved {
                tracing::warn!("could not keep the community list: {e}");
            }
            tracing::info!(
                "community list: {} apps from {} bundles",
                index.apps.len(),
                index.bundles.len()
            );
            *community.loaded.write().unwrap() = Some(Arc::new(Loaded { index }));
            status.etag = document.etag;
            status.last_modified = document.last_modified;
            status.checked_at = Some(now);
            status.failed = false;
            Ok(())
        }
        Err(e) => {
            tracing::warn!("community list refresh failed: {e}");
            status.failed = true;
            Err(e)
        }
    };
    status.attempted_at = Some(now);
    community.save_status(&status);
    *community.status.lock().unwrap() = status;
    outcome
}

/// Starts a background refresh of the index and the daily update checks, unless one runs.
pub fn kick(kiwi: &Arc<Kiwi>) {
    if automation::require_on(kiwi).is_err()
        || !kiwi.community.background.load(Ordering::Relaxed)
        || kiwi.community.busy.swap(true, Ordering::SeqCst)
    {
        return;
    }
    let kiwi = kiwi.clone();
    tokio::spawn(async move {
        let _ = refresh(&kiwi, false).await;
        check_updates(&kiwi).await;
        kiwi.community.busy.store(false, Ordering::SeqCst);
    });
}

// --- availability ---------------------------------------------------------------------------

/// Why community builds can't run on `os` at all (`std::env::consts::OS`).
pub fn platform_reason(os: &str) -> Option<&'static str> {
    (os != "linux").then_some(NEEDS_LINUX)
}

/// Whether this computer can confine the patcher: `sandbox.py --probe` must find Landlock
/// (ABI 3 or later) and the seccomp filter working. Checked once per process.
pub async fn sandbox(kiwi: &Kiwi) -> Result<(), String> {
    if let Some(known) = kiwi.community.sandbox.read().unwrap().clone() {
        return known;
    }
    let result = match platform_reason(std::env::consts::OS) {
        Some(reason) => Err(reason.to_owned()),
        None => probe(kiwi).await,
    };
    if let Err(e) = &result {
        tracing::info!("community builds unavailable: {e}");
    }
    kiwi.community.set_sandbox(result.clone());
    result
}

async fn probe(kiwi: &Kiwi) -> Result<(), String> {
    let script = kiwi.config.data_dir.join("automation/sandbox.py");
    let run = tokio::process::Command::new(&kiwi.config.python)
        .arg("-I")
        .arg("-S")
        .arg(&script)
        .arg("--probe")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let reasons = match tokio::time::timeout(Duration::from_secs(60), run).await {
        Ok(Ok(output)) => match serde_json::from_slice::<Value>(&output.stdout) {
            Ok(found)
                if found["landlock"].as_u64().is_some_and(|abi| abi >= 3)
                    && found["seccomp"] == true =>
            {
                return Ok(());
            }
            Ok(found) => found["reasons"]
                .as_array()
                .map(|r| {
                    r.iter()
                        .filter_map(Value::as_str)
                        .take(3)
                        .map(|s| index::clean_text(s, 160))
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "the sandbox check found no reason".into()),
            Err(_) => "the sandbox check failed".into(),
        },
        Ok(Err(e)) => format!("Python could not run the sandbox check: {e}"),
        Err(_) => "the sandbox check timed out".into(),
    };
    Err(format!(
        "Community builds need a sandbox for the patcher, and this computer can't provide one \
         ({reasons}). Use Linux 6.2 or later, or Kiwi's Docker image."
    ))
}

/// `Ok` when community apps can be installed now; else the one-sentence reason.
pub async fn availability(kiwi: &Kiwi) -> Result<(), String> {
    sandbox(kiwi).await?;
    if kiwi.community.index().is_none() {
        return Err(if kiwi.community.status.lock().unwrap().failed {
            LOAD_FAILED.into()
        } else {
            LOADING.into()
        });
    }
    Ok(())
}

// --- catalog views --------------------------------------------------------------------------

#[derive(Serialize)]
pub struct AppJson {
    id: String,
    package: String,
    name: String,
    icon_url: Option<String>,
    category: Option<String>,
    installs: u64,
    description: Option<String>,
    bundles: usize,
    patches: usize,
    featured: bool,
    added: bool,
}

fn app_json(app: &index::App, added: &BTreeSet<String>) -> AppJson {
    AppJson {
        id: id_of(&app.package),
        package: app.package.clone(),
        name: app.name.clone(),
        icon_url: app.icon_url.as_deref().map(crate::linked::client_icon),
        category: app.category.clone(),
        installs: app.installs,
        description: app.description.clone(),
        bundles: app.bundles.len(),
        patches: app.patches,
        featured: app.featured,
        added: added.contains(&app.package),
    }
}

fn added_packages(kiwi: &Kiwi) -> BTreeSet<String> {
    let state = kiwi.automation.snapshot();
    state
        .community
        .iter()
        .filter(|(id, _)| state.apps.get(*id).is_some_and(|a| a.added))
        .map(|(_, c)| c.package.clone())
        .collect()
}

/// `GET /api/community`.
pub async fn overview(kiwi: &Arc<Kiwi>) -> Value {
    kick(kiwi);
    let available = availability(kiwi).await;
    let added = added_packages(kiwi);
    let loaded = kiwi.community.index();
    let status = kiwi.community.status.lock().unwrap().clone();
    let index = loaded.as_ref().map(|l| {
        json!({
            "updated_at": status.checked_at,
            "apps": l.index.apps.len(),
            "bundles": l.index.bundles.len(),
            "stale": status.failed || status.checked_at.is_none_or(|t| Utc::now() - t > STALE_AFTER),
        })
    });
    let popular: Vec<AppJson> = loaded
        .as_ref()
        .map(|l| {
            l.index
                .popular(POPULAR)
                .into_iter()
                .map(|a| app_json(a, &added))
                .collect()
        })
        .unwrap_or_default();
    let state = kiwi.automation.snapshot();
    let installed: Vec<Value> = state
        .community
        .keys()
        .filter(|id| state.apps.get(*id).is_some_and(|a| a.added))
        .filter_map(|id| automation::app_view(kiwi, id).ok())
        .filter_map(|view| serde_json::to_value(view).ok())
        .collect();
    json!({
        "available": available.is_ok(),
        "reason": available.err(),
        "index": index,
        "popular": popular,
        "added": installed,
    })
}

/// `GET /api/community/search`.
pub fn search(kiwi: &Arc<Kiwi>, query: &str, offset: usize, limit: usize) -> Value {
    kick(kiwi);
    let added = added_packages(kiwi);
    let Some(loaded) = kiwi.community.index() else {
        return json!({"results": [], "total": 0});
    };
    let hits = loaded.index.search(query);
    let results: Vec<AppJson> = hits
        .iter()
        .skip(offset)
        .take(limit)
        .map(|a| app_json(a, &added))
        .collect();
    json!({"results": results, "total": hits.len()})
}

/// `GET /api/community/:id/candidates`: what the index says and what Kiwi learned, without
/// network.
pub fn candidates(kiwi: &Kiwi, id: &str) -> ApiResult<Value> {
    let package = package_of(id).ok_or_else(unknown)?;
    let state = kiwi.automation.snapshot();
    let added = state.community.get(id);
    let loaded = kiwi.community.index();
    let ranked = loaded
        .as_ref()
        .map(|l| l.index.candidates(package))
        .unwrap_or_default();
    if ranked.is_empty() && added.is_none() {
        return Err(unknown());
    }
    let pins = added.and_then(|a| a.pins.as_ref());
    let learned = |repo: &str| {
        added.and_then(|a| {
            a.learned
                .iter()
                .find(|(r, _)| r.eq_ignore_ascii_case(repo))
                .map(|(_, l)| l)
        })
    };
    // Kiwi's own pick: the automatic pin, else the best candidate not known to fail.
    let recommended = pins
        .filter(|p| p.chosen == Chosen::Auto)
        .map(|p| p.repo.clone())
        .or_else(|| {
            ranked
                .iter()
                .find(|c| {
                    learned(&c.repo).is_none_or(|l| l.failed.is_none() && l.eligible != Some(false))
                })
                .map(|c| c.repo.clone())
        });
    let mut out: Vec<Value> = ranked
        .iter()
        .map(|c| {
            let learned = learned(&c.repo);
            json!({
                "repo": c.repo,
                "author": c.author,
                "stars": c.stars,
                "updated_at": c.updated_at.and_then(chrono::DateTime::from_timestamp_millis),
                "patches": learned.and_then(|l| l.patches).unwrap_or(c.patches),
                "versions": learned
                    .filter(|l| !l.versions.is_empty())
                    .map_or_else(|| c.versions.clone(), |l| l.versions.clone()),
                "recommended": recommended.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(&c.repo)),
                "chosen": pins.is_some_and(|p| p.repo.eq_ignore_ascii_case(&c.repo)),
                "failed": learned.and_then(|l| l.failed.clone()),
            })
        })
        .collect();
    // A pinned bundle the index no longer lists.
    if let Some(p) = pins
        && !ranked.iter().any(|c| c.repo.eq_ignore_ascii_case(&p.repo))
    {
        out.insert(
            0,
            json!({"repo": p.repo, "author": p.author, "stars": 0, "updated_at": p.published_at,
                "patches": p.patches.len(), "versions": [p.version], "recommended": p.chosen == Chosen::Auto,
                "chosen": true, "failed": null}),
        );
    }
    Ok(Value::Array(out))
}

fn unknown() -> ApiError {
    ApiError::not_found("Kiwi doesn't know that community app.")
}

/// The `community` part of an added app's state.
#[derive(Serialize)]
pub struct View {
    package: String,
    repo: Option<String>,
    author: Option<String>,
    tag: Option<String>,
    chosen: Chosen,
    update: Option<Offered>,
    resolving: bool,
}

/// The parts of the automation state a community app shows (see `automation::app_view`).
pub struct Shown {
    pub name: String,
    pub bundle_id: String,
    pub icon_url: Option<String>,
    pub tweak: String,
    pub summary: String,
    pub warning: String,
    pub app_version: Option<String>,
    pub limitations: Vec<Limitation>,
    pub included: Vec<PatchInfo>,
    pub view: View,
}

pub fn shown(added: &Added, jobs: &[Job]) -> Shown {
    let pending = jobs.iter().find(|j| j.finished_at.is_none());
    let manual = pending
        .and_then(|j| j.community.as_ref())
        .and_then(|i| i.repo.clone());
    let pins = added.pins.as_ref();
    let repo = pins.map(|p| p.repo.clone()).or(manual.clone());
    let replaces = pins.is_none_or(|p| p.output_package == added.package);
    let name = &added.name;
    let mut limitations = vec![
        Limitation {
            icon: "warning".into(),
            title: "Community · Not Reviewed".into(),
            detail: format!(
                "The patches come from {} on GitHub. Kiwi hasn't reviewed them; it runs them \
                 locked in a sandbox, without network or Kiwi's signing key.",
                repo.as_deref().unwrap_or("a community bundle")
            ),
        },
        if replaces {
            Limitation {
                icon: "warning".into(),
                title: "Replaces the Play Store App".into(),
                detail: format!(
                    "This build keeps {name}'s own package name. Uninstall the Play Store \
                     version first; Android refuses to install over an app signed by someone else."
                ),
            }
        } else {
            Limitation {
                icon: "phone".into(),
                title: "Installs Beside the Play Store App".into(),
                detail: format!("This build installs as its own app, next to {name}."),
            }
        },
        Limitation {
            icon: "warning".into(),
            title: "Google Sign-In and Notifications".into(),
            detail: "Signing in with Google, push notifications and some logins may not work: \
                     the app is no longer signed by its publisher."
                .into(),
        },
        Limitation {
            icon: "phone".into(),
            title: "Android 12L or Later".into(),
            detail: "For 64-bit ARM phones and tablets. Builds aren't tested on a device; \
                     updates come only when you approve them."
                .into(),
        },
    ];
    limitations.truncate(4);
    Shown {
        name: name.clone(),
        bundle_id: pins.map_or_else(|| added.package.clone(), |p| p.output_package.clone()),
        icon_url: added.icon_url.as_deref().map(crate::linked::client_icon),
        tweak: pins.map_or_else(
            || "Community patches".into(),
            |p| format!("{} {}", p.bundle_name, p.tag),
        ),
        summary: pins.map_or_else(
            || match &repo {
                Some(r) => format!("Community patches from {r}"),
                None => "Community patches, picked by Kiwi".into(),
            },
            |p| format!("{} patches from {}", p.patches.len(), p.repo),
        ),
        warning: "Community patches aren't reviewed by Kiwi.".into(),
        app_version: pins.map(|p| p.version.clone()),
        limitations,
        included: pins.map(|p| p.patches.clone()).unwrap_or_default(),
        view: View {
            package: added.package.clone(),
            author: pins.map(|p| p.author.clone()).or_else(|| {
                repo.as_ref()
                    .and_then(|r| r.split('/').next().map(str::to_owned))
            }),
            repo,
            tag: pins.map(|p| p.tag.clone()),
            chosen: match (pins, &manual) {
                (_, Some(_)) => Chosen::Manual,
                (Some(p), None) => p.chosen,
                (None, None) => Chosen::Auto,
            },
            update: added.update.clone(),
            resolving: pending.is_some() && (pins.is_none() || manual.is_some()),
        },
    }
}

// --- install and update ---------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallBody {
    #[serde(default)]
    pub repo: Option<String>,
}

/// The newest published release of a community app, by its output package.
fn published(kiwi: &Kiwi, added: &Added) -> Option<Release> {
    let pins = added.pins.as_ref()?;
    automation::published_release(kiwi, &pins.output_package)
}

async fn require_available(kiwi: &Kiwi) -> ApiResult<()> {
    availability(kiwi)
        .await
        .map_err(ApiError::community_unavailable)
}

/// `POST /api/community/:id/install`: adds the app if needed and queues an interactive
/// build. `Ok(false)` when the published build is already what was asked for.
pub async fn install(kiwi: &Arc<Kiwi>, id: &str, body: InstallBody) -> ApiResult<bool> {
    let package = package_of(id).ok_or_else(unknown)?.to_owned();
    require_available(kiwi).await?;
    let loaded = kiwi.community.index();
    let index_app = loaded
        .as_ref()
        .and_then(|l| l.index.apps.get(&package).cloned());
    let state = kiwi.automation.snapshot();
    let existing = state.community.get(id).cloned();
    if index_app.is_none() && existing.is_none() {
        return Err(unknown());
    }
    // A chosen bundle must be one the index lists for this app (or the pinned one), spelt
    // as the index spells it.
    let repo = match body.repo {
        None => None,
        Some(repo) => {
            if !valid_repo(&repo) {
                return Err(ApiError::bad_request(
                    "Name the bundle as owner/repository.",
                ));
            }
            let listed = loaded
                .as_ref()
                .map(|l| l.index.candidates(&package))
                .unwrap_or_default()
                .into_iter()
                .map(|c| c.repo)
                .chain(
                    existing
                        .as_ref()
                        .and_then(|a| a.pins.as_ref())
                        .map(|p| p.repo.clone()),
                )
                .find(|r| r.eq_ignore_ascii_case(&repo));
            Some(listed.ok_or_else(|| {
                ApiError::bad_request(format!(
                    "{repo} has no patches for {} in the community list.",
                    index_app
                        .as_ref()
                        .map_or(package.as_str(), |a| a.name.as_str())
                ))
            })?)
        }
    };
    let unchanged = existing.as_ref().is_some_and(|a| {
        state.apps.get(id).is_some_and(|s| s.added)
            && published(kiwi, a).is_some()
            && repo.as_ref().is_none_or(|r| {
                a.pins
                    .as_ref()
                    .is_some_and(|p| p.repo.eq_ignore_ascii_case(r))
            })
    });
    let job_id = automation::new_job_id()?;
    let queued = kiwi.automation.change(&kiwi.db, |s| {
        let added = s.community.entry(id.to_owned()).or_insert_with(|| Added {
            package: package.clone(),
            ..Added::default()
        });
        if let Some(app) = &index_app {
            added.name = app.name.clone();
            added.icon_url = app.icon_url.clone();
            added.category = app.category.clone();
            added.description = app.description.clone();
        }
        if added.name.is_empty() {
            added.name = package.clone();
        }
        let name = added.name.clone();
        let app = s.apps.entry(id.to_owned()).or_insert_with(|| AppState {
            preferences: preferences(),
            ..AppState::default()
        });
        app.added = true;
        if let Some(job) = s.jobs.iter_mut().find(|j| j.app == id && j.finished_at.is_none()) {
            let pending = job.community.as_ref().and_then(|i| i.repo.as_ref());
            if repo.is_some() && !pending.zip(repo.as_ref()).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)) {
                return Err(ApiError::conflict(format!(
                    "A {name} build with another bundle is already on its way. Cancel it or wait for it first."
                )));
            }
            job.interactive = true;
            return Ok(true);
        }
        if unchanged {
            return Ok(false);
        }
        automation::push_job(
            s,
            &job_id,
            id,
            Some(JobIntent { repo: repo.clone(), update: None }),
        )?;
        Ok(true)
    })?;
    if queued {
        automation::start_next(kiwi);
    }
    Ok(queued)
}

/// `POST /api/community/:id/update`: approves the offered update and builds it.
pub async fn approve_update(kiwi: &Arc<Kiwi>, id: &str) -> ApiResult<()> {
    package_of(id).ok_or_else(unknown)?;
    require_available(kiwi).await?;
    let job_id = automation::new_job_id()?;
    kiwi.automation.change(&kiwi.db, |s| {
        let added = s.community.get(id).ok_or_else(unknown)?;
        let offered = added
            .update
            .clone()
            .filter(|_| s.apps.get(id).is_some_and(|a| a.added))
            .ok_or_else(|| ApiError::conflict("No update is offered for this app."))?;
        if let Some(job) = s
            .jobs
            .iter_mut()
            .find(|j| j.app == id && j.finished_at.is_none())
        {
            if job.community.as_ref().and_then(|i| i.update.as_ref()) == Some(&offered.tag) {
                job.interactive = true;
                return Ok(());
            }
            return Err(ApiError::conflict(
                "A build of this app is already on its way. Wait for it first.",
            ));
        }
        automation::push_job(
            s,
            &job_id,
            id,
            Some(JobIntent {
                repo: None,
                update: Some(offered.tag),
            }),
        )
    })?;
    automation::start_next(kiwi);
    Ok(())
}

/// Takes a removed app's bundle pins away (the next install picks again); the publisher's
/// certificate stays pinned.
pub fn forget_pins(added: &mut Added) {
    added.pins = None;
    added.update = None;
    added.update_checked_at = None;
}

// --- daily update check ---------------------------------------------------------------------

/// The newest stable release of a pinned repository: `(tag, published_at)`, when it has a
/// patch bundle.
pub fn latest_release(body: &Value) -> Option<(String, chrono::DateTime<Utc>)> {
    if body["draft"] == true || body["prerelease"] == true {
        return None;
    }
    let tag = body["tag_name"].as_str().filter(|t| valid_tag(t))?;
    let published = body["published_at"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())?
        .with_timezone(&Utc);
    let bundle = body["assets"].as_array()?.iter().any(|a| {
        a["name"]
            .as_str()
            .is_some_and(|n| n.to_ascii_lowercase().ends_with(".mpp"))
    });
    bundle.then(|| (tag.to_owned(), published))
}

/// What to offer after a check found `latest`: nothing when it is the pinned release, the
/// release once it is old enough, else the earlier offer.
pub fn offer(
    pins: &Pins,
    previous: Option<Offered>,
    latest: Option<(String, chrono::DateTime<Utc>)>,
    now: Time,
) -> Option<Offered> {
    match latest {
        Some((tag, _)) if tag == pins.tag => None,
        Some((tag, published)) if now - published >= UPDATE_MIN_AGE => Some(Offered {
            tag,
            published_at: published.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        }),
        _ => previous,
    }
}

pub async fn check_updates(kiwi: &Arc<Kiwi>) {
    let now = Utc::now();
    let state = kiwi.automation.snapshot();
    let due: Vec<(String, Pins)> = state
        .community
        .iter()
        .filter(|(id, _)| state.apps.get(*id).is_some_and(|a| a.added))
        .filter(|(_, a)| {
            a.update_checked_at
                .is_none_or(|t| now - t >= UPDATE_CHECK_EVERY)
        })
        .filter_map(|(id, a)| Some((id.clone(), a.pins.clone()?)))
        .collect();
    let api = kiwi.community.github_api.read().unwrap().clone();
    for (id, pins) in due {
        let Ok(url) = Url::parse(&format!(
            "{}/repositories/{}/releases/latest",
            api.trim_end_matches('/'),
            pins.repo_id
        )) else {
            continue;
        };
        let fetched = kiwi
            .sources
            .json_bytes("GitHub", &url, 2 * 1024 * 1024, &kiwi.cancel)
            .await;
        let (latest, checked) = match fetched
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| e.to_string()))
        {
            Ok(body) => (Some(latest_release(&body)), now),
            Err(e) => {
                tracing::info!(app = %id, "update check failed: {e}");
                // Retried in a few hours rather than tomorrow.
                (None, now - UPDATE_CHECK_EVERY + chrono::Duration::hours(4))
            }
        };
        let _ = kiwi.automation.change(&kiwi.db, |s| {
            if let Some(added) = s.community.get_mut(&id)
                && added.pins.as_ref() == Some(&pins)
            {
                if let Some(latest) = latest {
                    added.update = offer(&pins, added.update.take(), latest, now);
                }
                added.update_checked_at = Some(checked);
            }
            Ok(())
        });
    }
}

// --- building ---------------------------------------------------------------------------------

/// A failed community build: the message, whether it needs the owner, and what the app
/// can offer (`retry`, `other_bundle`, `unavailable`).
pub type Failed = (Failure, Option<&'static str>);

fn fail(message: impl Into<String>, blocked: bool, kind: Option<&'static str>) -> Failed {
    ((message.into(), blocked), kind)
}

pub fn error_kind(value: Option<&str>) -> Option<&'static str> {
    match value? {
        "retry" => Some("retry"),
        "other_bundle" => Some("other_bundle"),
        "unavailable" => Some("unavailable"),
        _ => None,
    }
}

/// What the runner gets for one job.
fn request(
    added: &Added,
    intent: &JobIntent,
    index: Option<&Index>,
    sequence: u64,
    api: &str,
) -> Result<Value, Failed> {
    let pins = added.pins.as_ref();
    let named = |repo: &str| {
        index.and_then(|i| {
            i.candidates(&added.package)
                .into_iter()
                .find(|c| c.repo.eq_ignore_ascii_case(repo))
        })
    };
    let candidate = |c: &index::Candidate| {
        json!({"repo": c.repo, "author": c.author, "stars": c.stars, "updated_at": c.updated_at,
            "targets": c.targets, "versions": c.versions})
    };
    let pinned = |p: &Pins| json!({"repo": p.repo, "author": p.author, "stars": 0, "updated_at": null, "targets": 0, "versions": [p.version]});
    let (mode, candidates) = match (&intent.update, &intent.repo, pins) {
        (Some(_), _, Some(p)) => ("update", vec![pinned(p)]),
        (Some(_), _, None) => return Err(fail("The update no longer applies; install the app again.", true, None)),
        (None, Some(repo), p) if p.is_none_or(|p| !p.repo.eq_ignore_ascii_case(repo)) => (
            "manual",
            vec![named(repo).as_ref().map_or_else(
                || json!({"repo": repo, "author": repo.split('/').next(), "stars": 0, "updated_at": null, "targets": 0, "versions": []}),
                candidate,
            )],
        ),
        (None, _, Some(p)) => ("pinned", vec![pinned(p)]),
        (None, _, None) => {
            let ranked = index.map(|i| i.candidates(&added.package)).unwrap_or_default();
            if ranked.is_empty() {
                return Err(fail(
                    format!("No community bundle offers patches for {} right now.", added.name),
                    false,
                    Some("unavailable"),
                ));
            }
            ("auto", ranked.iter().take(CANDIDATES).map(candidate).collect())
        }
    };
    // Bundles that failed at a release are skipped while that release is the latest.
    let skip: BTreeMap<&String, &String> = added
        .learned
        .iter()
        .filter(|_| mode == "auto")
        .filter_map(|(repo, l)| Some((repo, l.tag.as_ref().filter(|_| l.failed.is_some())?)))
        .collect();
    Ok(json!({
        "package": added.package,
        "name": added.name,
        "sequence": sequence,
        "mode": mode,
        "candidates": candidates,
        "pins": pins,
        "update_tag": intent.update,
        "certificate": added.certificate,
        "skip": skip,
        "github_api": api,
    }))
}

/// Records one `community` event of the runner as it arrives.
fn learn(kiwi: &Kiwi, id: &str, event: &Value) -> Result<(), Failure> {
    let repo = event["repo"].as_str().filter(|r| valid_repo(r));
    let tag = event["tag"]
        .as_str()
        .filter(|t| valid_tag(t))
        .map(str::to_owned);
    let text = |key: &str| event[key].as_str().map(|s| index::clean_text(s, 300));
    kiwi.automation
        .change(&kiwi.db, |s| {
            let Some(added) = s.community.get_mut(id) else {
                return Ok(());
            };
            match (event["kind"].as_str(), repo) {
                (Some("candidate"), Some(repo)) => {
                    let entry = added.learned.entry(repo.to_owned()).or_default();
                    if entry.tag != tag {
                        entry.failed = None;
                    }
                    entry.tag = tag;
                    entry.versions = event["versions"]
                        .as_array()
                        .map(|v| {
                            v.iter()
                                .filter_map(Value::as_str)
                                .map(|s| index::clean_text(s, 100))
                                .take(20)
                                .collect()
                        })
                        .unwrap_or_default();
                    entry.patches = event["patches"].as_u64().map(|n| n as usize);
                    entry.default_patches = event["default_patches"].as_u64().map(|n| n as usize);
                    entry.eligible = event["eligible"].as_bool();
                    if entry.eligible == Some(false) {
                        entry.failed = text("reason");
                    }
                }
                (Some("failed"), Some(repo)) => {
                    let entry = added.learned.entry(repo.to_owned()).or_default();
                    if tag.is_some() {
                        entry.tag = tag;
                    }
                    entry.failed =
                        Some(text("reason").unwrap_or_else(|| "It didn't build.".into()));
                }
                (Some("certificate"), _) => {
                    let sha = event["sha256"]
                        .as_str()
                        .filter(|s| automation::valid_sha(s));
                    if added.certificate.is_none() {
                        added.certificate = sha.map(str::to_owned);
                    }
                }
                _ => {}
            }
            Ok(())
        })
        .map_err(|e| (e.to_string(), false))
}

/// The pins in an artifact event, checked against what was asked and what was pinned.
fn artifact_pins(
    artifact: &Value,
    added: &Added,
    mode: &str,
    other_outputs: &BTreeSet<String>,
) -> Result<Pins, Failed> {
    let bad = |what: &str| fail(format!("The builder reported {what}"), false, None);
    let mut reported = artifact["community"]
        .as_object()
        .cloned()
        .ok_or_else(|| bad("no pins for the community build"))?;
    reported.insert("pinned_at".into(), json!(Utc::now()));
    let chosen = match mode {
        "manual" => Chosen::Manual,
        "auto" => Chosen::Auto,
        _ => added.pins.as_ref().map_or(Chosen::Auto, |p| p.chosen),
    };
    reported.insert("chosen".into(), json!(chosen));
    let mut pins: Pins =
        serde_json::from_value(Value::Object(reported)).map_err(|_| bad("unreadable pins"))?;
    if !valid_repo(&pins.repo)
        || !valid_tag(&pins.tag)
        || !automation::valid_sha(&pins.asset_sha256)
        || !valid_package(&pins.output_package)
        || pins.patches.is_empty()
    {
        return Err(bad("unreadable pins"));
    }
    pins.author = index::clean_text(&pins.author, 120);
    pins.bundle_name = index::clean_text(&pins.bundle_name, 120);
    if pins.bundle_name.is_empty() {
        pins.bundle_name = pins.repo.split('/').nth(1).unwrap_or_default().to_owned();
    }
    pins.patches.truncate(2000);
    for p in &mut pins.patches {
        p.name = index::clean_text(&p.name, 120);
        p.description = index::clean_text(&p.description, 1000);
    }
    if artifact["version"].as_str() != Some(pins.version.as_str())
        || artifact["build"].as_str() != Some(pins.output_version_code.to_string().as_str())
        || artifact["package"].as_str() != Some(pins.output_package.as_str())
    {
        return Err(bad("a patched app that doesn't match its pins"));
    }
    if official_packages().contains(&pins.output_package) {
        return Err(fail(
            "The patched app would replace one of Kiwi's own builds; nothing was published.",
            true,
            Some("other_bundle"),
        ));
    }
    if other_outputs.contains(&pins.output_package) {
        return Err(fail(
            "The patched app would replace another community app in Kiwi; nothing was published.",
            true,
            Some("other_bundle"),
        ));
    }
    if let Some(old) = &added.pins {
        let same_release = old.repo.eq_ignore_ascii_case(&pins.repo) && old.tag == pins.tag;
        let mismatch = if mode == "pinned" {
            !same_release
                || old.asset_sha256 != pins.asset_sha256
                || old.version_code != pins.version_code
                || old.output_package != pins.output_package
                || old.output_version_code != pins.output_version_code
                || old.repo_id != pins.repo_id
        } else if mode == "update" {
            old.repo_id != pins.repo_id || old.output_package != pins.output_package
        } else {
            false
        };
        if mismatch {
            return Err(fail(
                "The patched app doesn't match what Kiwi pinned for this app; nothing was published.",
                true,
                Some("other_bundle"),
            ));
        }
    }
    Ok(pins)
}

/// Runs one community job: resolve (or follow the pins), build, publish.
pub(crate) async fn execute(
    kiwi: &Arc<Kiwi>,
    job: &Job,
    token: &CancellationToken,
) -> Result<bool, Failed> {
    let id = job.app.as_str();
    if sandbox(kiwi).await.is_err() {
        return Err(fail(
            NEEDS_LINUX.replace("Community builds need", "This build needs"),
            true,
            Some("unavailable"),
        ));
    }
    let state = kiwi.automation.snapshot();
    let added = state
        .community
        .get(id)
        .cloned()
        .ok_or_else(|| fail("This community app is no longer in Kiwi.", false, None))?;
    let intent = job.community.clone().unwrap_or_default();
    let loaded = kiwi.community.index();
    let api = kiwi.community.github_api.read().unwrap().clone();
    let request = request(
        &added,
        &intent,
        loaded.as_ref().map(|l| &l.index),
        job.sequence,
        &api,
    )?;
    let mode = request["mode"].as_str().unwrap_or("auto").to_owned();
    let program = kiwi.automation.android_runner.read().unwrap().clone();
    let call = Invocation {
        program,
        command: "community",
        id: &job.id,
        recipe: None,
        builder: "The Android builder",
        timeout: Duration::from_secs(4 * 3600),
    };
    let mut kind = None;
    let events = automation::invoke(
        kiwi,
        call,
        &request,
        token,
        |stage| {
            kiwi.automation
                .job(&kiwi.db, &job.id, |j| j.stage = stage.into())
                .map_err(|e| (e.to_string(), false))
        },
        |event| {
            if event["event"] == "error" {
                kind = error_kind(event["kind"].as_str());
                return Ok(());
            }
            learn(kiwi, id, event)
        },
    )
    .await
    .map_err(|f| (f, kind))?;
    let artifact = events
        .iter()
        .find(|e| e["event"] == "artifact")
        .ok_or_else(|| fail("The builder returned no patched app", false, None))?;
    let others: BTreeSet<String> = kiwi
        .automation
        .snapshot()
        .community
        .iter()
        .filter(|(other, _)| other.as_str() != id)
        .filter_map(|(_, a)| a.pins.as_ref().map(|p| p.output_package.clone()))
        .collect();
    let pins = artifact_pins(artifact, &added, &mode, &others)?;
    let release = automation::community_release(kiwi, artifact, job.sequence, token)
        .await
        .map_err(|f| (f, None))?;
    kiwi.automation
        .job(&kiwi.db, &job.id, |j| {
            j.version = Some(pins.version.clone());
            j.external_id = Some(pins.version_code.to_string());
            j.stage = "Adding to Kiwi".into();
        })
        .map_err(|e| fail(e.to_string(), false, None))?;
    // The pins become the app's before publication, so the source lists the new build
    // under its output package.
    let previous = added.pins.clone();
    kiwi.automation
        .change(&kiwi.db, |s| {
            let a = s
                .community
                .get_mut(id)
                .ok_or_else(|| ApiError::not_found("This community app is no longer in Kiwi."))?;
            if a.update.as_ref().is_some_and(|u| u.tag == pins.tag) {
                a.update = None;
            }
            if let Some(l) = a.learned.get_mut(&pins.repo) {
                l.failed = None;
            }
            a.pins = Some(pins.clone());
            Ok(())
        })
        .map_err(|e| fail(e.to_string(), false, None))?;
    let published = automation::publish_community(kiwi, id, release, token).await;
    if let Err(e) = published {
        // Back to what was published before.
        let _ = kiwi.automation.change(&kiwi.db, |s| {
            if let Some(a) = s.community.get_mut(id) {
                a.pins = previous;
            }
            Ok(())
        });
        return Err((e, None));
    }
    kiwi.automation
        .app(&kiwi.db, id, |a| {
            a.last_success_key = artifact["sha256"].as_str().map(str::to_owned);
            a.failed_recipe = None;
            a.failed_attempts = 0;
            Ok(())
        })
        .map_err(|e| fail(e.to_string(), false, None))?;
    kiwi.kick.notify_one();
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pins() -> Pins {
        Pins {
            repo: "VenusIsJaded/Venus-Patches".into(),
            repo_id: 42,
            author: "VenusIsJaded".into(),
            tag: "v1.21.1".into(),
            published_at: None,
            asset: "patches-1.21.1.mpp".into(),
            asset_sha256: "a".repeat(64),
            bundle_name: "Venus-Patches".into(),
            version: "351.11 - Stable".into(),
            version_code: 351011,
            output_package: "com.discord".into(),
            output_version_code: 351011,
            patches: vec![PatchInfo {
                name: "Venus settings".into(),
                description: String::new(),
            }],
            chosen: Chosen::Auto,
            pinned_at: Utc::now(),
        }
    }

    #[test]
    fn ids_and_tags() {
        assert_eq!(package_of("community-com.discord"), Some("com.discord"));
        assert_eq!(package_of("community-../x"), None);
        assert_eq!(package_of("youtube"), None);
        assert!(is_id("community-anything"));
        assert_eq!(id_of("com.discord"), "community-com.discord");
        assert!(valid_tag("v1.21.1"));
        assert!(valid_tag("release/2026-11"));
        for bad in ["", "../x", "-v1", "v1 2", "v1?x", &"v".repeat(129)] {
            assert!(!valid_tag(bad), "{bad}");
        }
    }

    #[test]
    fn only_linux_can_build_community_apps() {
        assert_eq!(platform_reason("macos"), Some(NEEDS_LINUX));
        assert_eq!(
            NEEDS_LINUX,
            "Community builds need Kiwi on Linux or in Docker, where Kiwi can lock the patcher in a sandbox."
        );
        assert_eq!(platform_reason("windows"), Some(NEEDS_LINUX));
        assert_eq!(platform_reason("linux"), None);
    }

    #[test]
    fn updates_are_offered_after_three_days() {
        let now = Utc::now();
        let release = |tag: &str, hours: i64| {
            json!({"tag_name": tag, "draft": false, "prerelease": false,
                "published_at": (now - chrono::Duration::hours(hours)).to_rfc3339(),
                "assets": [{"name": "patches-1.22.0.mpp"}]})
        };
        let p = pins();
        let old = latest_release(&release("v1.22.0", 80));
        assert_eq!(offer(&p, None, old, now).unwrap().tag, "v1.22.0");
        let young = latest_release(&release("v1.22.0", 10));
        assert_eq!(offer(&p, None, young.clone(), now), None);
        let earlier = Offered {
            tag: "v1.21.9".into(),
            published_at: "x".into(),
        };
        assert_eq!(
            offer(&p, Some(earlier.clone()), young, now),
            Some(earlier.clone()),
            "an earlier offer stays while a newer release is young"
        );
        let same = latest_release(&release("v1.21.1", 500));
        assert_eq!(offer(&p, Some(earlier), same, now), None);
        let mut pre = release("v2.0.0", 500);
        pre["prerelease"] = json!(true);
        assert_eq!(latest_release(&pre), None);
        let mut nobundle = release("v2.0.0", 500);
        nobundle["assets"] = json!([{"name": "notes.txt"}]);
        assert_eq!(latest_release(&nobundle), None);
        let mut hostile = release("../../x", 500);
        hostile["assets"] = json!([{"name": "a.mpp"}]);
        assert_eq!(latest_release(&hostile), None);
    }

    fn artifact(p: &Pins) -> Value {
        json!({"event": "artifact", "sha256": "b".repeat(64), "version": p.version,
            "build": p.output_version_code.to_string(), "package": p.output_package,
            "community": p})
    }

    #[test]
    fn a_pinned_rebuild_must_match_its_pins() {
        let p = pins();
        let added = Added {
            package: "com.discord".into(),
            name: "Discord".into(),
            pins: Some(p.clone()),
            ..Added::default()
        };
        let none = BTreeSet::new();
        assert_eq!(
            artifact_pins(&artifact(&p), &added, "pinned", &none)
                .unwrap()
                .asset_sha256,
            p.asset_sha256
        );
        // The same release with other bytes, another version code or package: a hard stop.
        for change in [
            |p: &mut Pins| p.asset_sha256 = "c".repeat(64),
            |p: &mut Pins| p.output_version_code = 2147483647,
            |p: &mut Pins| p.repo_id = 7,
            |p: &mut Pins| p.tag = "v9".into(),
        ] {
            let mut other = p.clone();
            change(&mut other);
            let ((message, blocked), kind) =
                artifact_pins(&artifact(&other), &added, "pinned", &none).unwrap_err();
            assert!(blocked, "{message}");
            assert_eq!(kind, Some("other_bundle"));
        }
        // An update moves the tag and digest, but not the repository or the app.
        let mut newer = p.clone();
        newer.tag = "v1.22.0".into();
        newer.asset_sha256 = "d".repeat(64);
        newer.output_version_code = 352000;
        assert!(artifact_pins(&artifact(&newer), &added, "update", &none).is_ok());
        newer.output_package = "com.discord.venus".into();
        assert!(artifact_pins(&artifact(&newer), &added, "update", &none).is_err());
        // Never over Kiwi's own builds or another community app.
        let mut official = p.clone();
        official.output_package = "app.morphe.android.youtube".into();
        assert!(artifact_pins(&artifact(&official), &Added::default(), "auto", &none).is_err());
        let taken = BTreeSet::from(["com.discord".to_owned()]);
        assert!(artifact_pins(&artifact(&p), &Added::default(), "auto", &taken).is_err());
        // The artifact must be the app the pins describe.
        let mut wrong = artifact(&p);
        wrong["package"] = json!("com.other");
        assert!(artifact_pins(&wrong, &added, "auto", &none).is_err());
        // A manual pick is recorded as one.
        assert_eq!(
            artifact_pins(&artifact(&p), &Added::default(), "manual", &none)
                .unwrap()
                .chosen,
            Chosen::Manual
        );
    }
}
