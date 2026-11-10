//! Durable builds of patched apps: App Store apps from the recipes in
//! automation/recipes.json, Android apps from those in automation/android.json. One worker
//! builds one app at a time; other apps' jobs wait in a queue. The runners never receive
//! credentials over HTTP. iOS candidates enter the catalogue only after Kiwi's normal
//! signer has verified them; an Android runner verifies and signs its own.
use std::{
    collections::{BTreeMap, HashSet},
    io::Read,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, LazyLock, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use kiwi_source::{CancellationToken, Release, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::{
    Kiwi, builds,
    catalog::CatalogApp,
    db::{BrowseMode, BuildStatus, Db, SourceRow, Time},
    error::{ApiError, ApiResult},
};

/// The built-in Kiwi Builds source. Its id predates the other apps; installs refer to it.
pub const SOURCE_ID: &str = "kiwi-youtube";
const META: &str = "automation_v2";
/// State from when only YouTube was built; read once to migrate, then left untouched.
const LEGACY_META: &str = "youtube_automation_v1";
const RECIPES: &str = include_str!("../../../automation/recipes.json");
const RUNNER: &str = include_str!("../../../automation/runner.py");
const ANDROID_RECIPES: &str = include_str!("../../../automation/android.json");
const ANDROID_RUNNER: &str = include_str!("../../../automation/android.py");
const APK_READER: &str = include_str!("../../../automation/apk.py");

mod view;
pub use view::{AppView, Overview, app_view, overview};

#[derive(Debug, Deserialize)]
struct Recipes {
    apps: Vec<Recipe>,
}

/// What a recipe's app runs on, and so which runner builds it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    #[default]
    Ios,
    Android,
}

/// The parts of an app recipe the server uses; the runner reads the rest.
#[derive(Debug, Deserialize)]
pub struct Recipe {
    pub id: String,
    #[serde(default)]
    pub platform: Platform,
    pub name: String,
    /// The id the app has in Kiwi. For an Android app it is the package the patched app
    /// installs as, which can differ from the package it was downloaded as.
    pub bundle_id: String,
    /// The App Store app whose icon stands for this app, when `bundle_id` is not one.
    #[serde(default)]
    pub icon_bundle_id: Option<String>,
    pub tweak: String,
    pub summary: String,
    #[serde(default)]
    pub experimental: bool,
    #[serde(default)]
    pub warning: Option<String>,
    /// Home Screen names to choose from; the first is the default.
    #[serde(default)]
    pub names: Vec<String>,
    /// When set, build only this supported App Store version.
    #[serde(default)]
    pub app_version: Option<String>,
    pub interval_minutes: u32,
    /// Releases kept in the source (and their inputs on disk).
    pub retain: usize,
    pub catalog: CatalogInfo,
    pub packages: Vec<Package>,
    pub limitations: Vec<Limitation>,
}

#[derive(Debug, Deserialize)]
pub struct CatalogInfo {
    pub name: String,
    pub developer: String,
    pub subtitle: String,
    pub description: String,
    pub tint_color: String,
    pub category: String,
}

#[derive(Debug, Deserialize)]
pub struct Package {
    pub id: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limitation {
    pub icon: String,
    pub title: String,
    pub detail: String,
}

static PARSED: LazyLock<Recipes> = LazyLock::new(|| {
    let mut recipes: Recipes = serde_json::from_str(RECIPES).expect("valid embedded recipes");
    let android: Recipes =
        serde_json::from_str(ANDROID_RECIPES).expect("valid embedded Android recipes");
    assert!(
        android.apps.iter().all(|r| r.platform == Platform::Android),
        "android.json holds Android recipes only"
    );
    recipes.apps.extend(android.apps);
    recipes
});

/// Every app Kiwi can build, in display order.
pub fn recipes() -> &'static [Recipe] {
    &PARSED.apps
}

pub fn recipe(id: &str) -> ApiResult<&'static Recipe> {
    recipes()
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError::not_found("Kiwi doesn't build that app."))
}

impl Recipe {
    pub fn android(&self) -> bool {
        self.platform == Platform::Android
    }

    /// The bundle id Kiwi's App Store cache has this app's icon under.
    pub fn icon_id(&self) -> &str {
        self.icon_bundle_id.as_deref().unwrap_or(&self.bundle_id)
    }

    fn optional(&self) -> impl Iterator<Item = &Package> {
        self.packages.iter().filter(|p| p.optional)
    }

    pub fn default_preferences(&self) -> Preferences {
        let mut tweaks: Vec<String> = self
            .optional()
            .filter(|p| p.default)
            .map(|p| p.id.clone())
            .collect();
        tweaks.sort();
        Preferences {
            enabled: true,
            interval_minutes: self.interval_minutes,
            tweaks,
            name: self.names.first().cloned(),
        }
    }

    /// The app's name in Kiwi, with the chosen Home Screen name where there is a choice.
    fn catalog_name(&self, name: Option<&str>) -> String {
        let name = name.or(self.names.first().map(String::as_str));
        self.catalog
            .name
            .replace("{name}", name.unwrap_or(&self.name))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub enabled: bool,
    pub interval_minutes: u32,
    pub tweaks: Vec<String>,
    /// The Home Screen name, for apps that offer a choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_minutes: 60,
            tweaks: Vec::new(),
            name: None,
        }
    }
}
impl Preferences {
    pub fn validate(&mut self, recipe: &Recipe) -> ApiResult<()> {
        if !(15..=1440).contains(&self.interval_minutes) {
            return Err(ApiError::bad_request(
                "Check interval must be between 15 and 1440 minutes.",
            ));
        }
        let optional: Vec<&str> = recipe.optional().map(|p| p.id.as_str()).collect();
        if self.tweaks.len() > optional.len()
            || self.tweaks.iter().any(|t| !optional.contains(&t.as_str()))
        {
            return Err(ApiError::bad_request(
                "Unknown or duplicate tweak selection.",
            ));
        }
        self.tweaks.sort();
        self.tweaks.dedup();
        match (&self.name, recipe.names.is_empty()) {
            (None, true) => {}
            (None, false) => self.name = recipe.names.first().cloned(),
            (Some(name), false) if recipe.names.contains(name) => {}
            _ => return Err(ApiError::bad_request("Unknown app name.")),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    #[serde(default = "legacy_app")]
    pub app: String,
    pub sequence: u64,
    pub status: String,
    pub stage: String,
    pub version: Option<String>,
    pub external_id: Option<String>,
    pub recipe_key: Option<String>,
    pub signed_build_id: Option<String>,
    pub error: Option<String>,
    pub created_at: Time,
    pub finished_at: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferences: Option<Preferences>,
    /// Rebuild even when the recipe already succeeded.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
}
fn legacy_app() -> String {
    "youtube".into()
}
impl Job {
    /// Queued or running.
    fn active(&self) -> bool {
        self.finished_at.is_none()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    /// Shown in Kiwi Builds and built on schedule.
    pub added: bool,
    pub preferences: Preferences,
    pub last_checked_at: Option<Time>,
    pub last_success_key: Option<String>,
    pub last_success_tweaks: Option<Vec<String>>,
    pub last_success_name: Option<String>,
    pub failed_recipe: Option<String>,
    pub failed_attempts: u32,
    /// The last Kiwi revision handed out; never reused, also after removing the app.
    pub sequence: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub apps: BTreeMap<String, AppState>,
    /// Newest first, at most [`JOBS_PER_APP`] per app.
    pub jobs: Vec<Job>,
    pub checks: Vec<Value>,
}
const JOBS_PER_APP: usize = 30;

/// `youtube_automation_v1`: one app's preferences and history at the top level.
#[derive(Default, Deserialize)]
#[serde(default)]
struct LegacyState {
    preferences: Option<Preferences>,
    jobs: Vec<Job>,
    checks: Vec<Value>,
    last_checked_at: Option<Time>,
    last_success_key: Option<String>,
    last_success_tweaks: Option<Vec<String>>,
    failed_recipe: Option<String>,
    failed_attempts: u32,
    sequence: u64,
}

impl From<LegacyState> for State {
    fn from(old: LegacyState) -> Self {
        let youtube = AppState {
            added: true,
            preferences: old.preferences.unwrap_or_default(),
            last_checked_at: old.last_checked_at,
            last_success_key: old.last_success_key,
            last_success_tweaks: old.last_success_tweaks,
            last_success_name: None,
            failed_recipe: old.failed_recipe,
            failed_attempts: old.failed_attempts,
            sequence: old.sequence,
        };
        Self {
            apps: BTreeMap::from([("youtube".into(), youtube)]),
            jobs: old.jobs,
            checks: old.checks,
        }
    }
}

pub struct Automation {
    state: Mutex<State>,
    /// The job the worker is running, and how to cancel it.
    running: Mutex<Option<(String, CancellationToken)>>,
    /// Injectable runner for integration tests. Only local code can set this path.
    pub runner: RwLock<PathBuf>,
    /// The runner of Android recipes; injectable like `runner`.
    pub android_runner: RwLock<PathBuf>,
    /// Whether iOS apps can be built here: their runner needs macOS (Xcode's tools and
    /// decryption on the Mac). Tests elsewhere turn it on along with a fake runner.
    pub builds_ios: AtomicBool,
}
impl Automation {
    pub fn load(db: &Db, data: &std::path::Path) -> Result<Self, String> {
        let parse = |key: &str| db.meta(key).map_err(|e| e.to_string());
        let mut state: State = match parse(META)? {
            Some(text) => {
                serde_json::from_str(&text).map_err(|e| format!("automation state: {e}"))?
            }
            None => match parse(LEGACY_META)? {
                Some(text) => serde_json::from_str::<LegacyState>(&text)
                    .map_err(|e| format!("automation state: {e}"))?
                    .into(),
                None => State::default(),
            },
        };
        // Apps whose recipe Kiwi no longer ships (Rambler PoC and Gboard Rambler).
        state.apps.retain(|id, _| recipe(id).is_ok());
        state.jobs.retain(|j| recipe(&j.app).is_ok());
        for recipe in recipes() {
            state
                .apps
                .entry(recipe.id.clone())
                .or_insert_with(|| AppState {
                    preferences: recipe.default_preferences(),
                    ..AppState::default()
                });
        }
        for job in &mut state.jobs {
            if job.active() {
                job.error = Some(if job.status == "queued" {
                    "Kiwi restarted before this build started. Check again to retry.".into()
                } else {
                    "Kiwi restarted during this build. Check again to retry.".into()
                });
                job.status = "failed".into();
                job.stage = "Interrupted by restart".into();
                job.finished_at = Some(Utc::now());
            }
        }
        db.set_meta(
            META,
            &serde_json::to_string(&state).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let root = data.join("automation");
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        for (name, text) in [
            ("runner.py", RUNNER),
            ("recipes.json", RECIPES),
            ("android.py", ANDROID_RUNNER),
            ("apk.py", APK_READER),
            ("android.json", ANDROID_RECIPES),
        ] {
            let path = root.join(name);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
                let temporary = root.join(format!("{name}.new"));
                std::fs::write(&temporary, text).map_err(|e| e.to_string())?;
                std::fs::rename(temporary, path).map_err(|e| e.to_string())?;
            }
        }
        // The single-app recipe this file replaced, and the retired Rambler apps' sources
        // and their cached caller-metadata download.
        let _ = std::fs::remove_file(root.join("recipe.json"));
        let _ = std::fs::remove_dir_all(root.join("native"));
        let _ = std::fs::remove_dir_all(root.join("tools/native-inputs"));
        Ok(Self {
            state: Mutex::new(state),
            running: Mutex::new(None),
            runner: RwLock::new(root.join("runner.py")),
            android_runner: RwLock::new(root.join("android.py")),
            builds_ios: AtomicBool::new(cfg!(target_os = "macos")),
        })
    }
    pub fn snapshot(&self) -> State {
        self.state.lock().unwrap().clone()
    }
    /// Whether this Kiwi can build the recipe's app (iOS apps only on a Mac).
    pub fn can_build(&self, recipe: &Recipe) -> bool {
        recipe.android() || self.builds_ios.load(Ordering::Relaxed)
    }
    /// `409 needs_mac` for an iOS app on a Kiwi that does not run on macOS.
    fn require_buildable(&self, recipe: &Recipe) -> ApiResult<()> {
        if self.can_build(recipe) {
            Ok(())
        } else {
            Err(ApiError::needs_mac())
        }
    }
    /// The shared iOS setup checks: what the last iOS job found, or, where iOS apps cannot
    /// be built, only the missing Mac.
    pub fn checks(&self, state: &State) -> Vec<Value> {
        if self.builds_ios.load(Ordering::Relaxed) {
            state.checks.clone()
        } else {
            vec![json!({"name": NEEDS_MAC_CHECK, "ready": false})]
        }
    }
    fn change<T>(&self, db: &Db, edit: impl FnOnce(&mut State) -> ApiResult<T>) -> ApiResult<T> {
        let mut current = self.state.lock().unwrap();
        let mut next = current.clone();
        let result = edit(&mut next)?;
        db.set_meta(
            META,
            &serde_json::to_string(&next).map_err(ApiError::internal)?,
        )?;
        *current = next;
        Ok(result)
    }
    fn app<T>(
        &self,
        db: &Db,
        id: &str,
        edit: impl FnOnce(&mut AppState) -> ApiResult<T>,
    ) -> ApiResult<T> {
        self.change(db, |s| {
            edit(
                s.apps
                    .get_mut(id)
                    .ok_or_else(|| ApiError::not_found("Kiwi doesn't build that app."))?,
            )
        })
    }
    pub fn preferences(
        &self,
        kiwi: &Kiwi,
        id: &str,
        mut preferences: Preferences,
    ) -> ApiResult<()> {
        preferences.validate(recipe(id)?)?;
        self.app(&kiwi.db, id, |a| {
            a.preferences = preferences;
            Ok(())
        })?;
        kiwi.kick.notify_one();
        Ok(())
    }
    fn job(&self, db: &Db, id: &str, edit: impl FnOnce(&mut Job)) -> ApiResult<()> {
        self.change(db, |s| {
            let job = s
                .jobs
                .iter_mut()
                .find(|j| j.id == id)
                .ok_or_else(|| ApiError::not_found("Build job not found."))?;
            edit(job);
            Ok(())
        })
    }
}

/// The name of the setup check that is never ready where iOS apps cannot be built.
pub const NEEDS_MAC_CHECK: &str = "macOS";

/// Adds an app to Kiwi Builds with these preferences and queues its first build.
pub fn add(kiwi: &Arc<Kiwi>, id: &str, mut preferences: Preferences) -> ApiResult<()> {
    let recipe = recipe(id)?;
    kiwi.automation.require_buildable(recipe)?;
    preferences.validate(recipe)?;
    kiwi.automation.app(&kiwi.db, id, |a| {
        a.added = true;
        a.preferences = preferences;
        Ok(())
    })?;
    queue(kiwi, id, false)
}

/// Removes an app from Kiwi Builds: stops its builds and takes it out of the source.
/// Installed copies keep working until they expire; retained inputs age out.
pub fn remove(kiwi: &Kiwi, id: &str) -> ApiResult<()> {
    recipe(id)?;
    cancel(kiwi, id);
    kiwi.automation.app(&kiwi.db, id, |a| {
        a.added = false;
        a.last_success_key = None;
        a.failed_recipe = None;
        a.failed_attempts = 0;
        Ok(())
    })?;
    publish(kiwi, None)
}

pub fn cancel(kiwi: &Kiwi, id: &str) {
    let now = Utc::now();
    let _ = kiwi.automation.change(&kiwi.db, |s| {
        for job in s
            .jobs
            .iter_mut()
            .filter(|j| j.app == id && j.active() && j.status == "queued")
        {
            job.status = "cancelled".into();
            job.stage = "Cancelled".into();
            job.error = Some("The build was cancelled before it started.".into());
            job.finished_at = Some(now);
        }
        Ok(())
    });
    let running = kiwi.automation.running.lock().unwrap();
    if let Some((job, token)) = running.as_ref()
        && kiwi
            .automation
            .snapshot()
            .jobs
            .iter()
            .any(|j| &j.id == job && j.app == id)
    {
        token.cancel();
    }
}

/// Queues a check (and build, when anything changed) of an added app.
pub fn queue(kiwi: &Arc<Kiwi>, id: &str, force: bool) -> ApiResult<()> {
    let recipe = recipe(id)?;
    kiwi.automation.require_buildable(recipe)?;
    let mut bytes = [0u8; 12];
    getrandom::fill(&mut bytes).map_err(ApiError::internal)?;
    let job_id = hex::encode(bytes);
    kiwi.automation.change(&kiwi.db, |s| {
        if s.jobs.iter().any(|j| j.app == id && j.active()) {
            return Err(ApiError::conflict(format!(
                "A {} build job is already running.",
                recipe.name
            )));
        }
        let waiting_for = s
            .jobs
            .iter()
            .find(|j| j.active())
            .and_then(|j| self::recipe(&j.app).ok())
            .map(|r| r.name.as_str());
        let app = s
            .apps
            .get_mut(id)
            .ok_or_else(|| ApiError::not_found("Kiwi doesn't build that app."))?;
        if !app.added {
            return Err(ApiError::bad_request(format!(
                "Add {} to Kiwi Builds first.",
                recipe.name
            )));
        }
        app.sequence += 1;
        app.last_checked_at = Some(Utc::now());
        let job = Job {
            id: job_id.clone(),
            app: id.into(),
            sequence: app.sequence,
            status: "queued".into(),
            stage: waiting_for.map_or_else(
                || "Checking build setup".into(),
                |other| format!("Waiting for {other}"),
            ),
            version: None,
            external_id: None,
            recipe_key: None,
            signed_build_id: None,
            error: None,
            created_at: Utc::now(),
            finished_at: None,
            preferences: Some(app.preferences.clone()),
            force,
        };
        s.jobs.insert(0, job);
        let mut kept = BTreeMap::<String, usize>::new();
        s.jobs.retain(|j| {
            let count = kept.entry(j.app.clone()).or_default();
            *count += 1;
            *count <= JOBS_PER_APP
        });
        Ok(())
    })?;
    start_next(kiwi);
    Ok(())
}

/// Starts the oldest queued job unless one is running.
fn start_next(kiwi: &Arc<Kiwi>) {
    let mut running = kiwi.automation.running.lock().unwrap();
    if running.is_some() {
        return;
    }
    let next = kiwi.automation.change(&kiwi.db, |s| {
        let Some(job) = s
            .jobs
            .iter_mut()
            .rev()
            .find(|j| j.active() && j.status == "queued")
        else {
            return Ok(None);
        };
        job.status = "running".into();
        job.stage = "Checking build setup".into();
        Ok(Some(job.clone()))
    });
    match next {
        Ok(Some(job)) => {
            let token = kiwi.cancel.child_token();
            *running = Some((job.id.clone(), token.clone()));
            kiwi.jobs.tracker.spawn(work(kiwi.clone(), job, token));
        }
        Ok(None) => {}
        Err(e) => tracing::error!("could not start the next build job: {e}"),
    }
}

/// Kiwi Builds is a setting; with it off nothing builds and its API answers `builds_off`.
pub fn require_on(kiwi: &Kiwi) -> ApiResult<()> {
    if kiwi.settings().kiwi_builds {
        Ok(())
    } else {
        Err(ApiError::builds_off())
    }
}

/// Stops every app's queued and running builds (Kiwi Builds was turned off).
pub fn cancel_all(kiwi: &Kiwi) {
    for recipe in recipes() {
        cancel(kiwi, &recipe.id);
    }
}

pub fn tick(kiwi: &Arc<Kiwi>) {
    if require_on(kiwi).is_err() {
        return;
    }
    let state = kiwi.automation.snapshot();
    for (id, app) in &state.apps {
        let due = app.last_checked_at.is_none_or(|t| {
            Utc::now() - t >= chrono::Duration::minutes(i64::from(app.preferences.interval_minutes))
        });
        // An iOS app added on a Mac whose data moved to Linux waits for a Mac quietly.
        if app.added
            && app.preferences.enabled
            && due
            && !state.jobs.iter().any(|j| &j.app == id && j.active())
            && recipe(id).is_ok_and(|r| kiwi.automation.can_build(r))
            && let Err(e) = queue(kiwi, id, false)
        {
            tracing::warn!(app = %id, "could not queue a build: {e}");
        }
    }
}

async fn work(kiwi: Arc<Kiwi>, job: Job, token: CancellationToken) {
    let result = match recipe(&job.app) {
        Ok(recipe) => execute(&kiwi, recipe, &job, &token).await,
        Err(e) => Err(failed(e)),
    };
    let (status, stage, error) = match result {
        Ok(true) if recipe(&job.app).is_ok_and(Recipe::android) => {
            ("ready", "Available in Kiwi", None)
        }
        Ok(true) => ("ready", "Signed and available in Kiwi", None),
        Ok(false) => ("unchanged", "Already up to date", None),
        Err((_, _)) if token.is_cancelled() => (
            "cancelled",
            "Cancelled",
            Some("The build was cancelled. The previous release is still available.".into()),
        ),
        Err((message, blocked)) => (
            if blocked { "blocked" } else { "failed" },
            "Needs attention",
            Some(message),
        ),
    };
    if let Err(e) = kiwi.automation.change(&kiwi.db, |s| {
        let job = s
            .jobs
            .iter_mut()
            .find(|j| j.id == job.id)
            .ok_or_else(|| ApiError::not_found("Build job not found."))?;
        job.status = status.into();
        job.stage = stage.into();
        job.error = error;
        job.finished_at = Some(Utc::now());
        if status == "failed"
            && let Some(key) = job.recipe_key.clone()
            && let Some(app) = s.apps.get_mut(&job.app)
        {
            app.failed_attempts = if app.failed_recipe.as_ref() == Some(&key) {
                app.failed_attempts.saturating_add(1)
            } else {
                1
            };
            app.failed_recipe = Some(key);
        }
        Ok(())
    }) {
        tracing::error!("could not finish automation job: {e}");
    }
    let _ = tokio::fs::remove_file(
        kiwi.config
            .data_dir
            .join("automation")
            .join(format!("{}.request.json", job.id)),
    )
    .await;
    {
        let mut running = kiwi.automation.running.lock().unwrap();
        if running.as_ref().is_some_and(|(id, _)| id == &job.id) {
            *running = None;
        }
    }
    kiwi.kick.notify_one();
    start_next(&kiwi);
}

type Failure = (String, bool);
fn failed(e: impl std::fmt::Display) -> Failure {
    (e.to_string(), false)
}

/// Terminate the runner and all commands it supervises. The Python SIGTERM handler
/// reaps its own process groups; escalation also bounds a broken runner's shutdown.
#[cfg(unix)]
async fn stop(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .is_err()
        {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.wait().await;
        }
    }
}
#[cfg(not(unix))]
async fn stop(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
}

async fn runner(
    kiwi: &Kiwi,
    job: &Job,
    command: &str,
    request: &Value,
    token: &CancellationToken,
) -> Result<Vec<Value>, Failure> {
    let id = &job.id;
    let root = kiwi.config.data_dir.join("automation");
    let path = root.join(format!("{id}.request.json"));
    tokio::fs::write(&path, serde_json::to_vec(request).map_err(failed)?)
        .await
        .map_err(failed)?;
    let android = recipe(&job.app).is_ok_and(Recipe::android);
    let program = if android {
        &kiwi.automation.android_runner
    } else {
        &kiwi.automation.runner
    }
    .read()
    .unwrap()
    .clone();
    let builder = if android {
        "The Android builder"
    } else {
        "The Mac builder"
    };
    let python = &kiwi.config.python;
    let mut cmd = tokio::process::Command::new(python);
    cmd.arg(program)
        .arg(command)
        .arg("--data-dir")
        .arg(&kiwi.config.data_dir)
        .arg("--request")
        .arg(&path)
        .arg("--job-id")
        .arg(id)
        .arg("--recipe")
        .arg(&job.app)
        .arg("--supervisor-pid")
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut().process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| {
        (
            format!(
                "{builder} could not start Python ({}): {e}. Install Python 3.12 or newer, \
                 or name it with --python (KIWI_PYTHON).",
                python.display()
            ),
            true,
        )
    })?;
    let mut lines = BufReader::new(child.stdout.take().unwrap().take(2 * 1024 * 1024)).lines();
    let mut events = Vec::new();
    let mut runner_error = None;
    let deadline = tokio::time::sleep(Duration::from_secs(if command == "plan" {
        600
    } else {
        7200
    }));
    tokio::pin!(deadline);
    let read = async {
        while let Some(line) = lines.next_line().await.map_err(failed)? {
            let event: Value = serde_json::from_str(&line).map_err(failed)?;
            match event["event"].as_str() {
                Some("stage") => {
                    let stage = event["stage"]
                        .as_str()
                        .ok_or_else(|| failed("Invalid build stage"))?;
                    if stage.len() > 256 {
                        return Err(failed("Build stage exceeds its limit"));
                    }
                    kiwi.automation
                        .job(&kiwi.db, id, |j| j.stage = stage.into())
                        .map_err(failed)?;
                }
                Some("health") => {
                    let checks = event["checks"]
                        .as_array()
                        .ok_or_else(|| failed("Invalid setup checks"))?
                        .clone();
                    kiwi.automation
                        .change(&kiwi.db, |s| {
                            s.checks = checks;
                            Ok(())
                        })
                        .map_err(failed)?;
                }
                Some("error") => {
                    runner_error = Some((
                        event["message"]
                            .as_str()
                            .unwrap_or("The builder failed")
                            .chars()
                            .take(1024)
                            .collect(),
                        event["blocked"].as_bool().unwrap_or(false),
                    ))
                }
                Some("plan" | "artifact") => events.push(event),
                _ => return Err(failed("Unknown builder event")),
            }
        }
        let status = child.wait().await.map_err(failed)?;
        if let Some(error) = runner_error.take() {
            return Err(error);
        }
        if !status.success() {
            return Err(failed(format!(
                "{builder} stopped unexpectedly. Check its private log."
            )));
        }
        Ok(())
    };
    let result = tokio::select! {
        result = read => result,
        _ = token.cancelled() => Err(failed("Cancelled")),
        _ = &mut deadline => Err(failed(format!("{builder} exceeded its time limit"))),
    };
    if result.is_err() {
        stop(&mut child).await;
    }
    result?;
    Ok(events)
}

/// The newest release of a recipe's app in the published source.
fn published_release(kiwi: &Kiwi, recipe: &Recipe) -> Option<Release> {
    kiwi.catalog.source(SOURCE_ID).and_then(|s| {
        s.source
            .catalog
            .apps
            .iter()
            .find(|a| a.bundle_id == recipe.bundle_id)
            .and_then(|a| a.releases.first().cloned())
    })
}

async fn execute(
    kiwi: &Arc<Kiwi>,
    recipe: &Recipe,
    job: &Job,
    token: &CancellationToken,
) -> Result<bool, Failure> {
    let id = &job.id;
    let preferences = job.preferences.clone().unwrap_or_default();
    let events = runner(
        kiwi,
        job,
        "plan",
        &json!({"app": recipe.id, "tweaks": preferences.tweaks, "name": preferences.name}),
        token,
    )
    .await?;
    let plan = events
        .iter()
        .find(|e| e["event"] == "plan")
        .map(|e| e["plan"].clone())
        .ok_or_else(|| failed("The builder returned no build plan"))?;
    let key = plan["key"]
        .as_str()
        .filter(|s| valid_sha(s))
        .ok_or_else(|| failed("Invalid recipe key"))?;
    let version = plan["version"]
        .as_str()
        .ok_or_else(|| failed(format!("Missing {} version", recipe.name)))?;
    let external = plan["external_id"]
        .as_str()
        .ok_or_else(|| failed("Missing store build identifier"))?;
    kiwi.automation
        .job(&kiwi.db, id, |j| {
            j.version = Some(version.into());
            j.external_id = Some(external.into());
            j.recipe_key = Some(key.into());
        })
        .map_err(failed)?;
    let app = kiwi
        .automation
        .snapshot()
        .apps
        .remove(&recipe.id)
        .unwrap_or_default();
    if !job.force
        && app.last_success_key.as_deref() == Some(key)
        && published_release(kiwi, recipe).is_some()
    {
        return Ok(false);
    }
    if !job.force && app.failed_recipe.as_deref() == Some(key) && app.failed_attempts >= 3 {
        return Err(("This recipe failed three times. Fix its compatibility issue, then use Rebuild to retry.".into(), true));
    }
    let events = runner(
        kiwi,
        job,
        "build",
        &json!({"plan": plan, "sequence": job.sequence}),
        token,
    )
    .await?;
    let artifact = events
        .iter()
        .find(|e| e["event"] == "artifact")
        .ok_or_else(|| failed("The builder returned no patched app"))?;
    let release = validate_artifact(kiwi, recipe, artifact, version, job.sequence, token).await?;
    let update = (recipe, preferences.name.as_deref(), release.clone());
    let row = source_row(kiwi, Some(update.clone()))
        .map_err(failed)?
        .ok_or_else(|| failed(format!("{} is no longer in Kiwi Builds", recipe.name)))?;
    kiwi.automation
        .job(&kiwi.db, id, |j| {
            j.stage = if recipe.android() {
                "Adding to Kiwi".into()
            } else {
                "Signing and verifying with Kiwi".into()
            }
        })
        .map_err(failed)?;
    let index = row
        .source
        .catalog
        .apps
        .iter()
        .position(|a| a.bundle_id == recipe.bundle_id)
        .ok_or_else(|| failed(format!("{} is no longer in Kiwi Builds", recipe.name)))?;
    let candidate = CatalogApp {
        source: Arc::new(row),
        index,
    };
    let build = builds::prepare_release(kiwi, &candidate, 0)
        .await
        .map_err(failed)?;
    kiwi.automation
        .job(&kiwi.db, id, |j| j.signed_build_id = Some(build.id.clone()))
        .map_err(failed)?;
    // Signing has its own deadline and cleanup. Cancelling this job stops publication;
    // any already-running signing task can finish without exposing the candidate.
    loop {
        tokio::select! {
            _ = token.cancelled() => return Err(failed("Cancelled")),
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
        let current = kiwi
            .db
            .build(&build.id)
            .map_err(failed)?
            .ok_or_else(|| failed("The signing build disappeared"))?;
        match current.status {
            BuildStatus::Ready => break,
            BuildStatus::Failed => {
                return Err(failed(current.error.as_deref().unwrap_or("Signing failed")));
            }
            _ => {}
        }
    }
    if token.is_cancelled() {
        return Err(failed("Cancelled"));
    }
    // Re-read the source: visibility and other apps may have changed while signing.
    publish(kiwi, Some(update)).map_err(failed)?;
    kiwi.automation
        .app(&kiwi.db, &recipe.id, |a| {
            a.last_success_key = Some(key.into());
            a.last_success_tweaks = Some(preferences.tweaks.clone());
            a.last_success_name = preferences.name.clone();
            a.failed_recipe = None;
            a.failed_attempts = 0;
            Ok(())
        })
        .map_err(failed)?;
    kiwi.kick.notify_one();
    Ok(true)
}

pub fn valid_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Retained catalogue/install inputs survive cache eviction. Only unreferenced,
/// recognizably owned files age out, and never while a candidate is being built.
pub async fn cleanup(kiwi: &Kiwi) -> ApiResult<()> {
    if kiwi.automation.snapshot().jobs.iter().any(Job::active) {
        return Ok(());
    }
    let mut keep: HashSet<String> = HashSet::new();
    if let Some(source) = kiwi.catalog.source(SOURCE_ID) {
        for app in &source.source.catalog.apps {
            keep.extend(app.releases.iter().filter_map(|r| r.sha256.clone()));
        }
    }
    for build in kiwi
        .db
        .builds()?
        .into_iter()
        .filter(|b| b.source_id == SOURCE_ID)
    {
        keep.extend(build.release.sha256);
    }
    for install in kiwi
        .db
        .all_installs()?
        .into_iter()
        .filter(|i| i.source_id == SOURCE_ID)
    {
        keep.extend(install.release.and_then(|r| r.sha256));
    }
    let root = kiwi.config.data_dir.join("automation");
    tokio::task::spawn_blocking(move || {
        let old = |path: &std::path::Path, days: u64| {
            path.symlink_metadata()
                .ok()
                .filter(|m| !m.file_type().is_symlink())
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(days * 86400))
        };
        if let Ok(entries) = std::fs::read_dir(root.join("artifacts")) {
            for entry in entries.flatten() {
                let path = entry.path();
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if valid_sha(stem)
                    && matches!(
                        path.extension().and_then(|s| s.to_str()),
                        Some("ipa" | "apk" | "json")
                    )
                    && !keep.contains(stem)
                    && old(&path, 7)
                {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        for (directory, days) in [(root.join("tools/jobs"), 1), (root.join("jobs"), 30)] {
            if let Ok(entries) = std::fs::read_dir(directory) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.len() == 24
                        && name.bytes().all(|b| b.is_ascii_hexdigit())
                        && old(&entry.path(), days)
                    {
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
    })
    .await
    .map_err(ApiError::internal)?;
    Ok(())
}

pub fn artifact_path(kiwi: &Kiwi, release: &Release) -> ApiResult<PathBuf> {
    let sha = release
        .sha256
        .as_deref()
        .filter(|s| valid_sha(s))
        .ok_or_else(|| ApiError::bad_request("Invalid local artifact digest."))?;
    Ok(kiwi
        .config
        .data_dir
        .join("automation/artifacts")
        .join(format!("{sha}.{}", extension(builds::android(release)))))
}

fn extension(android: bool) -> &'static str {
    if android { "apk" } else { "ipa" }
}

pub async fn restore_artifact(
    kiwi: &Kiwi,
    release: &Release,
    cancel: &CancellationToken,
) -> Result<(PathBuf, String), String> {
    let path = artifact_path(kiwi, release).map_err(|e| e.to_string())?;
    let expected = release.sha256.clone().unwrap();
    let checked = path.clone();
    let token = cancel.clone();
    let kind = if builds::android(release) {
        "app"
    } else {
        "IPA"
    };
    let actual = tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&checked).map_err(|_| {
            format!("The local patched {kind} is missing. Rebuild it in Kiwi Builds.")
        })?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            if token.is_cancelled() {
                return Err("Cancelled".into());
            }
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        Ok::<_, String>(hex::encode(hash.finalize()))
    })
    .await
    .map_err(|e| e.to_string())??;
    if actual != expected {
        return Err(format!(
            "The local patched {kind} failed its integrity check. Rebuild it in Kiwi Builds."
        ));
    }
    Ok((path, actual))
}

async fn validate_artifact(
    kiwi: &Kiwi,
    recipe: &Recipe,
    artifact: &Value,
    version: &str,
    sequence: u64,
    token: &CancellationToken,
) -> Result<Release, Failure> {
    let sha = artifact["sha256"]
        .as_str()
        .filter(|s| valid_sha(s))
        .ok_or_else(|| failed("Invalid artifact SHA-256"))?;
    let release = Release {
        version: artifact["version"]
            .as_str()
            .filter(|v| *v == version)
            .ok_or_else(|| failed("Patched version does not match the plan"))?
            .into(),
        build: Some(
            artifact["build"]
                .as_str()
                .ok_or_else(|| failed("Missing patched build number"))?
                .into(),
        ),
        revision: Some(
            artifact["revision"]
                .as_u64()
                .filter(|r| *r == sequence)
                .ok_or_else(|| failed("Patched revision does not match the job"))?,
        ),
        date: Some(Utc::now().to_rfc3339()),
        notes: artifact["notes"].as_str().map(str::to_owned),
        download_url: Url::parse(&format!(
            "https://kiwi.invalid/automation/{sha}.{}",
            extension(recipe.android())
        ))
        .unwrap(),
        size: artifact["size"].as_u64(),
        sha256: Some(sha.into()),
        min_os: artifact["min_os"].as_str().map(str::to_owned),
        max_os: None,
        platforms: vec![if recipe.android() { "Android" } else { "iOS" }.into()],
    };
    let (path, _) = restore_artifact(kiwi, &release, token)
        .await
        .map_err(failed)?;
    if recipe.android() {
        // The Android runner checked the patched app's manifest and signature itself; Kiwi
        // serves the file as built, so its digest and declared identity are what count.
        if artifact["package"].as_str() != Some(recipe.bundle_id.as_str()) {
            return Err(failed("The patched app is not the one in the recipe"));
        }
        if release.min_os.is_some() {
            return Err(failed("Android builds declare no minimum OS version"));
        }
        return Ok(release);
    }
    let expected = release.clone();
    let token = token.clone();
    let bundle_id = recipe.bundle_id.clone();
    tokio::task::spawn_blocking(move || {
        let metadata = kiwi_source::inspect_ipa(&path, &token)?;
        kiwi_source::validate_metadata(&metadata, &expected, &bundle_id, None)
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    Ok(release)
}

/// The App Store icon of a recipe's app, from Kiwi's storefront cache.
fn store_icon(kiwi: &Kiwi, bundle_id: &str) -> Option<String> {
    kiwi.db
        .store_rows(&kiwi.settings().app_store_country)
        .ok()?
        .into_iter()
        .find(|r| r.bundle_id == bundle_id)
        .and_then(|r| r.app)
        .and_then(|a| a.icon_url)
}

/// A new release of one app, with the Home Screen name it was built with.
type Update<'a> = (&'a Recipe, Option<&'a str>, Release);

/// The Kiwi Builds source: every added app with its retained releases, plus `update`.
/// `None` when nothing was ever published.
fn source_row(kiwi: &Kiwi, update: Option<Update<'_>>) -> ApiResult<Option<SourceRow>> {
    let now = Utc::now();
    let old = kiwi.catalog.source(SOURCE_ID);
    let state = kiwi.automation.snapshot();
    let mut apps = Vec::new();
    for recipe in recipes() {
        if !state.apps.get(&recipe.id).is_some_and(|a| a.added) {
            continue;
        }
        let previous = old.as_ref().and_then(|s| {
            s.source
                .catalog
                .apps
                .iter()
                .find(|a| a.bundle_id == recipe.bundle_id)
        });
        let mut releases = Vec::new();
        let mut name = previous.map(|a| a.name.clone());
        if let Some((updated, chosen, release)) = &update
            && updated.id == recipe.id
        {
            releases.push(release.clone());
            name = Some(recipe.catalog_name(*chosen));
        }
        let kept = recipe.retain.saturating_sub(releases.len());
        releases.extend(
            previous
                .into_iter()
                .flat_map(|a| a.releases.iter().take(kept).cloned()),
        );
        if releases.is_empty() {
            continue;
        }
        let icon = store_icon(kiwi, recipe.icon_id())
            .or_else(|| previous.and_then(|a| a.icon_url.as_ref().map(Url::to_string)));
        let c = &recipe.catalog;
        apps.push(json!({
            "name": name.unwrap_or_else(|| recipe.catalog_name(None)), "bundle_id": recipe.bundle_id,
            "developer": c.developer, "subtitle": c.subtitle, "description": c.description,
            "tint_color": c.tint_color, "category": c.category, "icon_url": icon, "releases": releases,
        }));
    }
    if apps.is_empty() && old.is_none() {
        return Ok(None);
    }
    let url = Url::parse("https://kiwi.invalid/automation/builds").unwrap();
    let catalog = serde_json::from_value(json!({
        "name": "Kiwi Builds", "identifier": "local.kiwi.builds", "subtitle": "Built on your Mac",
        "apps": apps,
    }))
    .map_err(ApiError::internal)?;
    Ok(Some(SourceRow {
        id: SOURCE_ID.into(),
        url: url.to_string(),
        enabled: old.as_ref().is_none_or(|s| s.enabled),
        position: old
            .as_ref()
            .map_or_else(|| kiwi.catalog.next_position(), |s| s.position),
        added_at: old.as_ref().map_or(now, |s| s.added_at),
        last_attempt_at: Some(now),
        browse: old.as_ref().map_or(BrowseMode::All, |s| s.browse),
        picked_at: old.as_ref().and_then(|s| s.picked_at),
        source: kiwi_source::Source {
            key: SOURCE_ID.into(),
            url,
            enabled: old.as_ref().is_none_or(|s| s.enabled),
            catalog,
            etag: None,
            last_modified: None,
            last_success: Some(now),
            error: None,
            link: None,
        },
    }))
}

/// Takes apps Kiwi no longer builds out of the Kiwi Builds source. Installed copies keep
/// working until they expire, as after removing an app.
pub fn retire(kiwi: &Kiwi) -> ApiResult<()> {
    let Some(old) = kiwi.catalog.source(SOURCE_ID) else {
        return Ok(());
    };
    let retired = old
        .source
        .catalog
        .apps
        .iter()
        .any(|a| !recipes().iter().any(|r| r.bundle_id == a.bundle_id));
    if retired { publish(kiwi, None) } else { Ok(()) }
}

/// Saves and shows the Kiwi Builds source with `update` applied.
fn publish(kiwi: &Kiwi, update: Option<Update<'_>>) -> ApiResult<()> {
    if let Some(row) = source_row(kiwi, update)? {
        kiwi.db.save_source(&row)?;
        crate::sources::record_apps(kiwi, &row);
        kiwi.catalog.upsert(row);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_preferences_and_digest_paths() {
        let youtube = recipe("youtube").unwrap();
        let mut p = youtube.default_preferences();
        assert!(p.validate(youtube).is_ok());
        assert_eq!(p.tweaks, ["ryd", "youpip", "youquality", "ytabconfig"]);
        p.interval_minutes = 1;
        assert!(p.validate(youtube).is_err());
        p.interval_minutes = 60;
        p.tweaks.push("shell-command".into());
        assert!(p.validate(youtube).is_err());
        assert!(!valid_sha("../../private"));
        assert!(valid_sha(&"a".repeat(64)));
    }

    #[test]
    fn app_names_are_offered_only_where_the_recipe_has_a_choice() {
        let x = recipe("x").unwrap();
        let mut p = x.default_preferences();
        assert_eq!(p.name.as_deref(), Some("Twitter"));
        p.name = None;
        p.validate(x).unwrap();
        assert_eq!(p.name.as_deref(), Some("Twitter"));
        p.name = Some("Bird".into());
        assert!(p.validate(x).is_err());
        let twitch = recipe("twitch").unwrap();
        let mut p = twitch.default_preferences();
        p.name = Some("Twitter".into());
        assert!(p.validate(twitch).is_err());
        assert_eq!(x.catalog_name(Some("X")), "X");
        assert_eq!(x.catalog_name(None), "Twitter");
        assert_eq!(recipe("youtube").unwrap().catalog_name(None), "YouTube");
        assert!(recipe("instagram").is_err());
    }

    #[test]
    fn interrupted_jobs_are_durable_and_never_reappear_as_active() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("test.db")).unwrap();
        let automation = Automation::load(&db, tmp.path()).unwrap();
        let job = |id: char, status: &str| Job {
            id: id.to_string().repeat(24),
            app: "youtube".into(),
            sequence: 8,
            status: status.into(),
            stage: "Decrypting".into(),
            version: None,
            external_id: None,
            recipe_key: None,
            signed_build_id: None,
            error: None,
            created_at: Utc::now(),
            finished_at: None,
            preferences: None,
            force: false,
        };
        automation
            .change(&db, |s| {
                let app = s.apps.get_mut("youtube").unwrap();
                app.sequence = 8;
                app.failed_recipe = Some("a".repeat(64));
                app.failed_attempts = 3;
                app.preferences.enabled = false;
                s.jobs.push(job('a', "running"));
                s.jobs.push(job('b', "queued"));
                Ok(())
            })
            .unwrap();
        let restored = Automation::load(&db, tmp.path()).unwrap().snapshot();
        assert!(
            restored
                .jobs
                .iter()
                .all(|j| j.status == "failed" && !j.active())
        );
        let app = &restored.apps["youtube"];
        assert_eq!(app.sequence, 8);
        assert_eq!(app.failed_attempts, 3);
        assert_eq!(app.failed_recipe, Some("a".repeat(64)));
        assert!(!app.preferences.enabled);
    }

    #[test]
    fn single_app_state_migrates_to_youtube_and_new_apps_start_unadded() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("test.db")).unwrap();
        let legacy = json!({
            "preferences": {"enabled": false, "interval_minutes": 120, "tweaks": ["ryd"]},
            "jobs": [{"id": "c".repeat(24), "sequence": 4, "status": "ready", "stage": "Signed",
                "version": "21.43.5", "external_id": "1", "recipe_key": null, "signed_build_id": null,
                "error": null, "created_at": "2026-10-29T10:00:00Z", "finished_at": "2026-10-29T10:10:00Z"}],
            "checks": [{"name": "Mac build tools", "ready": true}],
            "last_success_key": "k", "last_success_tweaks": ["ryd"], "failed_attempts": 0, "sequence": 4
        });
        db.set_meta(LEGACY_META, &legacy.to_string()).unwrap();
        let state = Automation::load(&db, tmp.path()).unwrap().snapshot();
        let youtube = &state.apps["youtube"];
        assert!(youtube.added);
        assert_eq!(youtube.sequence, 4);
        assert_eq!(youtube.preferences.interval_minutes, 120);
        assert_eq!(
            youtube.last_success_tweaks.as_deref(),
            Some(&["ryd".to_owned()][..])
        );
        assert_eq!(state.jobs[0].app, "youtube");
        assert_eq!(state.checks.len(), 1);
        for id in ["youtube-music", "twitch", "x", "reddit"] {
            assert!(!state.apps[id].added, "{id}");
            assert_eq!(
                state.apps[id].preferences,
                recipe(id).unwrap().default_preferences()
            );
        }
        // The legacy key stays readable by an older Kiwi after a rollback.
        assert!(db.meta(LEGACY_META).unwrap().is_some());
        assert!(db.meta(META).unwrap().is_some());
    }
}
